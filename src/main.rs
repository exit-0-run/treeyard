use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

use treeyard::audit::Record;
use treeyard::cow::Strategy;
use treeyard::workspace::{CreateOptions, GcOptions, LayerSpec, Node};
use treeyard::{client, connect, daemon, githttp, mcp, replay, repo::Repo, server};

const DEFAULT_URL: &str = "http://127.0.0.1:7420";

#[derive(Parser)]
#[command(name = "treeyard", version, about = "Version control server for agent swarms, git underneath")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a data directory from the tip of a branch of an existing git repo.
    Init {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        from: PathBuf,
        #[arg(long, default_value = "main")]
        branch: String,
        /// Start from this revision instead of the branch tip.
        #[arg(long)]
        rev: Option<String>,
        /// Also import the last N first-parent commits as changesets (blame, log, bisect).
        #[arg(long, default_value_t = 0)]
        history: usize,
    },
    /// API tokens (the first one turns auth on): create, list, revoke. Works on the data dir.
    Token {
        /// The data directory (also accepted after the subcommand).
        #[arg(long, global = true)]
        data: Option<PathBuf>,
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// Serve the HTTP API.
    Serve {
        #[arg(long)]
        data: PathBuf,
        #[arg(long, default_value = "127.0.0.1:7420")]
        addr: String,
        /// Serve a non-loopback address although no API token exists (anyone could write).
        #[arg(long)]
        insecure_open: bool,
        /// API calls that may run at once; more get `503 busy` (clients retry).
        #[arg(long, default_value_t = server::MAX_INFLIGHT)]
        max_inflight: usize,
        /// Write the bound port to this file once listening (useful with port 0).
        #[arg(long)]
        port_file: Option<PathBuf>,
    },
    /// MCP server on stdio that forwards tool calls to a running treeyard.
    Mcp {
        #[arg(long, env = "TREEYARD_URL", default_value = DEFAULT_URL)]
        url: String,
        /// Default agent name for open_change and revert.
        #[arg(long, env = "TREEYARD_AGENT")]
        agent: Option<String>,
        /// Take the first free name of this list (comma-separated) as the agent name; a name is
        /// held by a lock file in --pool-dir while this bridge runs.
        #[arg(long, value_delimiter = ',', conflicts_with = "agent", requires = "pool_dir")]
        agent_pool: Vec<String>,
        #[arg(long)]
        pool_dir: Option<PathBuf>,
        /// Tools to list: `agent` (the ones a coding agent needs, short descriptions), `all`, or
        /// `native` (start/sync/land: the agent edits a directory with its own tools).
        #[arg(long, default_value = "all")]
        tools: mcp::ToolSet,
    },
    /// Attach TreeYard to the agent tools of the git repository here: import it, serve it on
    /// loopback, register the MCP server for Claude Code, Codex and OpenCode in project config,
    /// and add an instruction block to AGENTS.md / CLAUDE.md. `treeyard disconnect` undoes it.
    #[command(args_conflicts_with_subcommands = true)]
    Connect {
        #[command(subcommand)]
        cmd: Option<ConnectCmd>,
        /// Harnesses to attach: claude, codex, opencode (comma-separated) or all. Default: the
        /// ones on PATH.
        #[arg(long)]
        harness: Option<String>,
        /// Agent names per harness: <harness>-1 .. <harness>-N (default 1).
        #[arg(long)]
        agents: Option<usize>,
        /// Port on 127.0.0.1 (default: a free port, then the same one again).
        #[arg(long)]
        port: Option<u16>,
        /// Push landed changes to the `origin` remote (`treeyard mirror`).
        #[arg(long, conflicts_with = "no_mirror")]
        mirror: bool,
        /// Do not start the mirror and do not ask.
        #[arg(long)]
        no_mirror: bool,
        /// Let the MCP bridge list every operation (`treeyard mcp --tools all`), not only the
        /// agent tool set.
        #[arg(long)]
        all_tools: bool,
        /// Native mode (`treeyard mcp --tools native`): each agent edits and tests a directory of
        /// its own with its normal tools; `sync` and `land` send the result. Its own instruction block.
        #[arg(long, conflicts_with = "all_tools")]
        native: bool,
    },
    /// Undo `treeyard connect`: restore every file it touched, stop its server and mirror.
    Disconnect {
        /// Also delete the TreeYard data (open changes included).
        #[arg(long)]
        purge: bool,
    },
    /// Replay real history: waves of N agents apply consecutive commits from stale bases.
    Replay {
        /// Git repo whose history is replayed.
        #[arg(long)]
        src: PathBuf,
        /// Start revision (the import); its first-parent descendants up to --to are replayed.
        #[arg(long)]
        from: String,
        #[arg(long, default_value = "HEAD")]
        to: String,
        #[arg(long, default_value_t = 16)]
        agents: usize,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Data directory to create (must not exist).
        #[arg(long)]
        data: PathBuf,
        /// Skip the git merge-tree check of every merge.
        #[arg(long)]
        no_git_check: bool,
        /// Each agent stacks on the previous agent's change, so landing waits for it.
        #[arg(long)]
        stacked: bool,
        /// treeyard.toml for the replayed repo (merge drivers, continuous rebase).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Agents declare their commit's paths; an overlapping agent starts in a later round.
        #[arg(long)]
        intents: bool,
        /// Agents start on main at the start of their wave (3-way apply), not on the commit's parent.
        #[arg(long)]
        wave_base: bool,
    },
    /// Push the branch to a read-only mirror remote (any git URL), once or every N seconds.
    Mirror {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        remote: String,
        /// Keep running and push whenever the branch moved, checking every N seconds.
        #[arg(long)]
        every: Option<u64>,
    },
    /// Workspaces on this node: real directories (copy-on-write clones), each bound to one change.
    Workspace {
        #[command(flatten)]
        node: NodeArgs,
        #[command(subcommand)]
        cmd: WsCmd,
    },
    /// For orchestrators: run on workspace creation and removal. Only the path goes to stdout.
    /// Exit 2: bad name or no server address (TREEYARD_URL or `connect` state); 3: server down.
    WorkspaceHook {
        #[command(subcommand)]
        cmd: HookCmd,
    },
    /// Watch every workspace of this node and sync each one after its edits settle.
    Daemon {
        #[command(flatten)]
        node: NodeArgs,
        /// Quiet time after the last edit before a sync.
        #[arg(long, default_value_t = 300)]
        debounce_ms: u64,
        /// Also sync every workspace this often, in seconds (0 = only on events).
        #[arg(long, default_value_t = 30)]
        rescan_s: u64,
        /// Keep this many spare directories for current main (a warm pool for `workspace create`).
        #[arg(long, default_value_t = 0)]
        spares: usize,
        /// Run `workspace gc` at start and then every HOURS hours (0 = never).
        #[arg(long, default_value_t = 0.0)]
        gc_every: f64,
        /// `gc --max` for --gc-every.
        #[arg(long, default_value_t = 25)]
        gc_max: usize,
        /// `gc --max-gb` for --gc-every.
        #[arg(long, default_value_t = 50.0)]
        gc_max_gb: f64,
    },
    /// Agents that edit one shared directory (no workspaces): each hunk goes to one agent's change.
    Share {
        #[arg(long, env = "TREEYARD_URL", default_value = DEFAULT_URL, global = true)]
        url: String,
        #[command(subcommand)]
        cmd: ShareCmd,
    },
    /// Print the API reference (Markdown, docs/api.md) generated from the operation table.
    ApiDoc {
        /// Print the raw tool table as JSON instead (the MCP `tools/list` reply).
        #[arg(long)]
        json: bool,
    },
    /// Write the files of a change into a directory to run its tests there (the `checkout` tool
    /// of `treeyard mcp`). A second run writes only what changed. Prints JSON.
    Checkout {
        /// Change id, ch_...
        change: String,
        /// A new, empty or earlier checkout directory. Default: one per change under the temp dir.
        #[arg(long)]
        into: Option<PathBuf>,
        #[arg(long, env = "TREEYARD_URL", default_value = DEFAULT_URL)]
        url: String,
    },
    /// Call one operation, e.g. `treeyard call read '{"path":"README.md"}'`.
    Call {
        op: String,
        params: Option<String>,
        #[arg(long, env = "TREEYARD_URL", default_value = DEFAULT_URL)]
        url: String,
    },
}

#[derive(clap::Args)]
struct HookArgs {
    /// Server URL. Default: the `treeyard connect` state of the repository here (never a fixed port).
    #[arg(long, env = "TREEYARD_URL")]
    url: Option<String>,
    /// Node home (default ~/.treeyard).
    #[arg(long, env = "TREEYARD_HOME")]
    home: Option<PathBuf>,
}

impl HookArgs {
    fn common(&self) -> Result<treeyard::hook::Common> {
        Ok(treeyard::hook::Common {
            url: self.url.clone(),
            home: self.home.clone().unwrap_or_else(Node::default_home),
            cwd: std::env::current_dir()?,
        })
    }
}

#[derive(Subcommand)]
enum HookCmd {
    /// Open a change and make its workspace; print the workspace path. A ready workspace of the
    /// same name is reused (no second change).
    Create {
        /// Workspace name (letters, digits, . _ -). Default: the base name of --from-dir or of $PWD.
        #[arg(long, env = "TREEYARD_HOOK_NAME")]
        name: Option<String>,
        /// Agent name of the change (default hook-<name>).
        #[arg(long, env = "TREEYARD_AGENT")]
        agent: Option<String>,
        /// Declared intent of the change.
        #[arg(long, env = "TREEYARD_INTENT")]
        intent: Option<String>,
        /// Shell command run once in the new workspace (for example `npm ci`).
        #[arg(long, env = "TREEYARD_SETUP")]
        setup: Option<String>,
        /// No `.git` in the workspace (default: one, so git status and diff work).
        #[arg(long)]
        no_git: bool,
        /// The orchestrator's checkout: its uncommitted edits seed the change, its gitignored
        /// files (`.worktreeinclude`, `copy`) are copied, and its base name is the default name.
        #[arg(long, env = "TREEYARD_HOOK_DIR", value_name = "PATH")]
        from_dir: Option<PathBuf>,
        #[command(flatten)]
        hook: HookArgs,
    },
    /// Send the last edits to the change, delete the workspace, print its former path. The change
    /// stays open. A workspace that is not there is not an error (exit 0).
    Remove {
        /// Workspace name or path. Default: the name that `create` derives from --from-dir or $PWD.
        target: Option<String>,
        #[arg(long, env = "TREEYARD_HOOK_DIR", value_name = "PATH")]
        from_dir: Option<PathBuf>,
        /// Drop the open change instead of keeping it.
        #[arg(long)]
        abandon: bool,
        /// Delete even when the last edits cannot be sent.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        hook: HookArgs,
    },
}

fn workspace_hook(cmd: HookCmd) -> Result<()> {
    use treeyard::hook;
    let out = match cmd {
        HookCmd::Create { name, agent, intent, setup, no_git, from_dir, hook: h } => {
            let a = hook::CreateArgs { name, agent, intent, setup, git: !no_git, from_dir };
            hook::create(&h.common()?, &a).map(Some)
        }
        HookCmd::Remove { target, from_dir, abandon, force, hook: h } => {
            hook::remove(&h.common()?, &hook::RemoveArgs { target, from_dir, abandon, force })
        }
    };
    match out {
        Ok(Some(dir)) => println!("{}", dir.display()),
        Ok(None) => {}
        Err(e) => {
            eprintln!("workspace-hook: {e}");
            std::process::exit(e.code);
        }
    }
    Ok(())
}

#[derive(Subcommand)]
enum ConnectCmd {
    /// Server address, attached harnesses, open changes and overlap or conflict warnings.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    /// Create a token; prints it once (the server keeps only a hash).
    Create {
        /// The agent (or person) name; a non-admin token always acts as this agent.
        #[arg(long)]
        name: String,
        #[arg(long)]
        admin: bool,
        /// Path globs this token may read (repeat it); default `**`.
        #[arg(long = "read")]
        read: Vec<String>,
        /// Path globs this token may write (repeat it); none = read-only.
        #[arg(long = "write")]
        write: Vec<String>,
    },
    List,
    Revoke {
        id: String,
    },
}

#[derive(clap::Args)]
struct NodeArgs {
    /// Node home: bases, layers, workspaces (default ~/.treeyard).
    #[arg(long, env = "TREEYARD_HOME", global = true)]
    home: Option<PathBuf>,
    #[arg(long, env = "TREEYARD_URL", default_value = DEFAULT_URL, global = true)]
    url: String,
}

impl NodeArgs {
    fn node(&self) -> Node {
        Node::new(self.home.clone().unwrap_or_else(Node::default_home), self.url.clone())
    }
}

// Parsed once per run, so the size of `Create` does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum WsCmd {
    /// Open a change and make its workspace directory.
    Create {
        name: String,
        #[arg(long, env = "TREEYARD_AGENT", default_value = "agent")]
        agent: String,
        #[arg(long, default_value = "work in a workspace")]
        intent: String,
        /// Base revision (default main); a change id stacks on that change.
        #[arg(long)]
        base: Option<String>,
        /// auto, clone, reflink, overlay, hardlink or copy.
        #[arg(long, default_value = "auto")]
        strategy: Strategy,
        /// Dependency layer DIR=LOCKFILE[,LOCKFILE]; repeat it. Default: detected from lockfiles.
        #[arg(long = "layer")]
        layers: Vec<LayerSpec>,
        /// No dependency layers.
        #[arg(long, conflicts_with = "layers")]
        no_layers: bool,
        /// Shell command run in the new workspace (missing layers are saved after it).
        #[arg(long)]
        setup: Option<String>,
        /// Add a `.git` (HEAD = base commit) so git status, diff and log work in the workspace.
        #[arg(long)]
        git: bool,
        /// Shell command `remove` runs before it deletes the directory (a failure is reported only).
        #[arg(long)]
        teardown: Option<String>,
        /// Default command of `workspace exec NAME` with no command.
        #[arg(long)]
        run: Option<String>,
        /// Ports in the workspace's block (default 10).
        #[arg(long)]
        ports: Option<u16>,
        /// Directory to copy gitignored files from (globs: `copy` in .treeyard/workspace.toml, .worktreeinclude).
        #[arg(long)]
        copy_from: Option<PathBuf>,
        /// Sparse workspace: only these directories (comma-separated, like `src/,docs/`) plus root files.
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        /// Seed the change with the uncommitted edits of the git checkout at PATH (first changeset).
        #[arg(long, value_name = "PATH")]
        from_dir: Option<PathBuf>,
    },
    /// Keep COUNT spare directories (base clone + layers) for current main; create takes one.
    Spare {
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[arg(long = "layer")]
        layers: Vec<LayerSpec>,
        #[arg(long, conflicts_with = "layers")]
        no_layers: bool,
        /// Spares with a `.git` (for `create --git`).
        #[arg(long)]
        git: bool,
    },
    /// List the workspaces of this node.
    List,
    /// Send the workspace's edits to the server now.
    Sync { name: String },
    /// Sync, then land the change on main.
    Land {
        name: String,
        /// Seconds to wait for the tests of queue or optimistic landing (0 = do not wait).
        #[arg(long, default_value_t = 600)]
        wait_s: u64,
        /// Land although the change removes lines another agent landed (status `undoes`), with
        /// this reason (why the task removes them); recorded as `undid`.
        #[arg(long, value_name = "REASON")]
        undo: Option<String>,
    },
    /// Sync, restore the change to a checkpoint, and rewrite only the files that differ.
    Restore {
        name: String,
        /// Checkpoint number (0 = no edits), checkpoint id cp_..., or a changeset id of the change.
        #[arg(long)]
        to: String,
    },
    /// Print the workspace's environment as `export` lines.
    Env { name: String },
    /// Print the workspace directory.
    Path { name: String },
    /// Run a command in the workspace with its environment, then sync. No command: the `run`
    /// command of the workspace config.
    Exec {
        name: String,
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
    },
    /// After every sync of NAME, write the change's files into DIR (only files that differ), for
    /// apps that need one fixed path or port. --off stops it.
    Mirror {
        name: String,
        #[arg(long, required_unless_present = "off", conflicts_with = "off")]
        into: Option<PathBuf>,
        #[arg(long)]
        off: bool,
    },
    /// Remove old workspaces until at most --max are left and they use at most --max-gb: landed or
    /// abandoned first, then the least recently synced. Never one younger than 10 minutes, one
    /// being created, or one with edits not synced yet.
    Gc {
        #[arg(long, default_value_t = 25)]
        max: usize,
        #[arg(long, default_value_t = 50.0)]
        max_gb: f64,
        /// Print what would be removed; remove nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Sync, then show this change's collisions with other agents' open changes and its land status.
    Status { name: String },
    /// Build on another agent's open change (after a landing_order collision): its edits appear
    /// here now, merged with yours; land waits until it has landed.
    Stack {
        name: String,
        #[arg(long)]
        on: String,
    },
    /// Write the change as the server holds it now into the workspace (after a land that hit a
    /// conflict, or a continuous rebase): the work that landed first plus conflict markers.
    Update { name: String },
    /// Send the last edits to the change, then delete the workspace directory. With --abandon the
    /// open change is dropped instead. Refuses when the edits cannot be sent, unless --force.
    Remove {
        name: String,
        #[arg(long)]
        abandon: bool,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ShareCmd {
    /// Open a change for an agent in the directory (base: the share base, main for the first agent).
    Join {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, env = "TREEYARD_AGENT")]
        agent: String,
        #[arg(long)]
        intent: String,
        /// Path globs the agent will edit (repeat it); its hunks there go to its change.
        #[arg(long = "paths", num_args = 1..)]
        paths: Vec<String>,
        /// Symbols (`name` or `path#name`) the agent will edit (repeat it).
        #[arg(long = "symbols", num_args = 1..)]
        symbols: Vec<String>,
    },
    /// Send the directory's edits: assign hunks, then write each agent's own hunks into its change.
    Sync {
        #[arg(long)]
        dir: PathBuf,
        /// First rebase every agent's change onto main and move the share there.
        #[arg(long)]
        rebase: bool,
    },
    /// Give hunks (ids from sync) to an agent's change.
    Claim {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, env = "TREEYARD_AGENT")]
        agent: String,
        #[arg(required = true)]
        hunks: Vec<String>,
    },
}

fn share(url: &str, cmd: ShareCmd) -> Result<Value> {
    match cmd {
        ShareCmd::Join { dir, agent, intent, paths, symbols } => {
            treeyard::share::join(url, &dir, &agent, &intent, &paths, &symbols)
        }
        ShareCmd::Sync { dir, rebase } => treeyard::share::sync(url, &dir, rebase),
        ShareCmd::Claim { dir, agent, hunks } => treeyard::share::claim(url, &dir, &agent, &hunks),
    }
}

fn gb(g: f64) -> u64 {
    (g.max(0.0) * (1u64 << 30) as f64) as u64
}

fn print(v: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn workspace(node: Node, cmd: WsCmd) -> Result<()> {
    match cmd {
        WsCmd::Create {
            name,
            agent,
            intent,
            base,
            strategy,
            layers,
            no_layers,
            setup,
            git,
            teardown,
            run,
            ports,
            copy_from,
            paths,
            from_dir,
        } => {
            let layers = if no_layers {
                Some(vec![])
            } else if layers.is_empty() {
                None
            } else {
                Some(layers)
            };
            let o = CreateOptions {
                name,
                agent,
                intent,
                base,
                strategy,
                layers,
                setup,
                git,
                teardown,
                run,
                ports,
                copy_from,
                paths,
                from_dir,
            };
            let (ws, timing) = node.create(&o)?;
            let mut v = serde_json::to_value(&ws)?;
            v["timing"] = timing;
            print(&v)?;
        }
        WsCmd::Spare { count, layers, no_layers, git } => {
            let layers = if no_layers {
                Some(vec![])
            } else if layers.is_empty() {
                None
            } else {
                Some(layers)
            };
            print(&node.fill_spares(count, layers.as_deref(), git)?)?
        }
        WsCmd::List => print(&serde_json::to_value(node.list()?)?)?,
        WsCmd::Sync { name } => print(&node.sync(&name)?)?,
        WsCmd::Land { name, wait_s, undo } => {
            print(&node.land_undo(&name, std::time::Duration::from_secs(wait_s), undo.as_deref())?)?
        }
        WsCmd::Restore { name, to } => print(&node.restore(&name, &to)?)?,
        WsCmd::Env { name } => {
            for (k, v) in node.get(&name)?.env() {
                println!("export {k}='{}'", v.replace('\'', "'\\''"));
            }
        }
        WsCmd::Path { name } => println!("{}", node.get(&name)?.dir.display()),
        WsCmd::Exec { name, cmd } => {
            let ws = node.get(&name)?;
            let cmd = match (cmd.is_empty(), &ws.run) {
                (false, _) => cmd,
                (true, Some(run)) => vec!["sh".into(), "-c".into(), run.clone()],
                (true, None) => anyhow::bail!(
                    "no command: write one after `--`, or set `run` in .treeyard/workspace.toml (or create --run)"
                ),
            };
            let status = std::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .current_dir(&ws.dir)
                .envs(ws.env())
                .status()
                .with_context(|| format!("run {}", cmd[0]))?;
            let sync = node.sync(&name)?;
            eprintln!("{}", serde_json::to_string(&sync)?);
            std::process::exit(status.code().unwrap_or(1));
        }
        WsCmd::Mirror { name, into, off: _ } => print(&node.mirror(&name, into.as_deref())?)?,
        WsCmd::Gc { max, max_gb, dry_run } => print(&node.gc(&GcOptions {
            max: Some(max),
            max_bytes: Some(gb(max_gb)),
            dry_run,
            ..Default::default()
        })?)?,
        WsCmd::Update { name } => print(&node.update(&name)?)?,
        WsCmd::Status { name } => print(&node.status(&name)?)?,
        WsCmd::Stack { name, on } => print(&node.stack(&name, &on)?)?,
        WsCmd::Remove { name, abandon, force } => print(&node.remove(&name, abandon, force)?)?,
    }
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Init { data, from, branch, rev, history } => {
            let root = Repo::init_with_history(&data, &from, &branch, rev.as_deref(), history)?;
            println!("imported {branch} from {} as changeset {}", from.display(), &root[..12]);
        }
        Cmd::Token { data, cmd } => {
            let repo = Repo::open(&data.context("token needs --data DIR")?)?;
            let audit = |op: &str, detail: String| {
                let rec = Record {
                    actor: "cli".into(),
                    op: op.into(),
                    status: "ok".into(),
                    detail: Some(detail),
                    ..Record::default()
                };
                repo.with(|r| r.audit_add(rec))
            };
            match cmd {
                TokenCmd::Create { name, admin, read, write } => {
                    let read = if read.is_empty() { vec!["**".to_string()] } else { read };
                    let (id, token) = repo.auth.create(&name, admin, &read, &write)?;
                    audit("token_create", format!("token {id} ({name})"))?;
                    eprintln!("token {id} for {name}: store it now (it is shown once), e.g. in TREEYARD_TOKEN");
                    println!("{token}");
                }
                TokenCmd::List => print(&repo.auth.list()?)?,
                TokenCmd::Revoke { id } => {
                    if !repo.auth.revoke(&id)? {
                        anyhow::bail!("no active token {id}");
                    }
                    audit("token_revoke", format!("token {id}"))?;
                    println!("revoked {id}");
                }
            }
        }
        Cmd::Serve { data, addr, insecure_open, max_inflight, port_file } => {
            let _lock = server::lock_data(&data)?;
            let repo = Arc::new(Repo::open(&data)?);
            let opts = server::ServeOptions { limits: server::Limits::new(max_inflight), insecure_open, port_file };
            tokio::runtime::Runtime::new()?.block_on(server::serve_with(repo, &addr, opts))?;
        }
        Cmd::Replay { src, from, to, agents, limit, seed, data, no_git_check, stacked, config, intents, wave_base } => {
            let o = replay::Options {
                src,
                from,
                to,
                agents,
                limit,
                seed,
                data,
                check_git: !no_git_check,
                stacked,
                config,
                intents,
                wave_base,
            };
            println!("{}", serde_json::to_string_pretty(&replay::run(&o)?)?);
        }
        Cmd::Mirror { data, remote, every } => {
            let mut last = String::new();
            loop {
                let commit = githttp::mirror(&data, &remote)?;
                if commit != last {
                    println!("mirrored {commit} to {remote}");
                    last = commit;
                }
                match every {
                    Some(s) => std::thread::sleep(std::time::Duration::from_secs(s.max(1))),
                    None => break,
                }
            }
        }
        Cmd::Mcp { url, agent, agent_pool, pool_dir, tools } => {
            let held = match pool_dir {
                Some(dir) if !agent_pool.is_empty() => Some(connect::claim_name(&agent_pool, &dir)?),
                _ => None,
            };
            let agent = held.as_ref().map(|h| h.name.clone()).or(agent);
            mcp::run(&url, agent, tools)?;
            drop(held);
        }
        Cmd::Connect { cmd, harness, agents, port, mirror, no_mirror, all_tools, native } => {
            let here = std::env::current_dir()?;
            match cmd {
                Some(ConnectCmd::Status { json }) => {
                    let v = connect::status_json(&here)?;
                    if json {
                        print(&v)?;
                    } else {
                        print!("{}", connect::status_text(&v));
                    }
                }
                None => {
                    let o = connect::Options {
                        harnesses: harness.as_deref().map(connect::Harness::parse_list).transpose()?,
                        agents,
                        port,
                        mirror: (mirror || no_mirror).then_some(mirror),
                        all_tools,
                        native,
                    };
                    connect::connect(&here, &o)?;
                }
            }
        }
        Cmd::Disconnect { purge } => connect::disconnect(&std::env::current_dir()?, purge)?,
        Cmd::Workspace { node, cmd } => workspace(node.node(), cmd)?,
        Cmd::WorkspaceHook { cmd } => workspace_hook(cmd)?,
        Cmd::Daemon { node, debounce_ms, rescan_s, spares, gc_every, gc_max, gc_max_gb } => {
            let gc = GcOptions { max: Some(gc_max), max_bytes: Some(gb(gc_max_gb)), ..Default::default() };
            let o = daemon::Options {
                debounce: std::time::Duration::from_millis(debounce_ms),
                rescan: std::time::Duration::from_secs(rescan_s),
                spares,
                gc: (gc_every > 0.0).then(|| (std::time::Duration::from_secs_f64(gc_every * 3600.0), gc)),
                ..Default::default()
            };
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            daemon::run(&node.node(), &o, stop, &mut |line| println!("{line}"))?;
        }
        Cmd::Share { url, cmd } => print(&share(&url, cmd)?)?,
        Cmd::ApiDoc { json } => {
            let tools = treeyard::rpc::tools();
            if json {
                println!("{}", serde_json::to_string_pretty(&json!({"tools": tools}))?);
            } else {
                print!("{}", treeyard::apidoc::markdown(&tools));
            }
        }
        Cmd::Checkout { change, into, url } => {
            let dir = match into {
                Some(d) => d,
                None => treeyard::workspace::checkout_dir(&change)?,
            };
            print(&treeyard::workspace::checkout(&url, &change, &dir)?)?;
        }
        Cmd::Call { op, params, url } => {
            let params: Value = match params {
                Some(p) => serde_json::from_str(&p).context("params must be JSON")?,
                None => json!({}),
            };
            let v = client::call(&url, &op, params)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
    }
    Ok(())
}
