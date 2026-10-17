use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

use treeyard::audit::Record;
use treeyard::cow::Strategy;
use treeyard::workspace::{CreateOptions, LayerSpec, Node};
use treeyard::{client, daemon, githttp, mcp, replay, repo::Repo, server};

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
    },
    /// MCP server on stdio that forwards tool calls to a running treeyard.
    Mcp {
        #[arg(long, env = "TREEYARD_URL", default_value = DEFAULT_URL)]
        url: String,
        /// Default agent name for open_change and revert.
        #[arg(long, env = "TREEYARD_AGENT")]
        agent: Option<String>,
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
    },
    /// Print the API reference (Markdown, docs/api.md) generated from the operation table.
    ApiDoc {
        /// Print the raw tool table as JSON instead (the MCP `tools/list` reply).
        #[arg(long)]
        json: bool,
    },
    /// Call one operation, e.g. `treeyard call read '{"path":"README.md"}'`.
    Call {
        op: String,
        params: Option<String>,
        #[arg(long, env = "TREEYARD_URL", default_value = DEFAULT_URL)]
        url: String,
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
    Land { name: String },
    /// Print the workspace's environment as `export` lines.
    Env { name: String },
    /// Print the workspace directory.
    Path { name: String },
    /// Run a command in the workspace with its environment, then sync.
    Exec {
        name: String,
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
    /// Delete the workspace directory (and with --abandon, its open change).
    Remove {
        name: String,
        #[arg(long)]
        abandon: bool,
    },
}

fn print(v: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn workspace(node: Node, cmd: WsCmd) -> Result<()> {
    match cmd {
        WsCmd::Create { name, agent, intent, base, strategy, layers, no_layers, setup, git } => {
            let layers = if no_layers {
                Some(vec![])
            } else if layers.is_empty() {
                None
            } else {
                Some(layers)
            };
            let (ws, timing) =
                node.create(&CreateOptions { name, agent, intent, base, strategy, layers, setup, git })?;
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
        WsCmd::Land { name } => print(&node.land(&name)?)?,
        WsCmd::Env { name } => {
            for (k, v) in node.get(&name)?.env() {
                println!("export {k}='{}'", v.replace('\'', "'\\''"));
            }
        }
        WsCmd::Path { name } => println!("{}", node.get(&name)?.dir.display()),
        WsCmd::Exec { name, cmd } => {
            let ws = node.get(&name)?;
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
        WsCmd::Remove { name, abandon } => print(&node.remove(&name, abandon)?)?,
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
        Cmd::Serve { data, addr, insecure_open, max_inflight } => {
            let _lock = server::lock_data(&data)?;
            let repo = Arc::new(Repo::open(&data)?);
            let opts = server::ServeOptions { limits: server::Limits::new(max_inflight), insecure_open };
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
        Cmd::Mcp { url, agent } => mcp::run(&url, agent)?,
        Cmd::Workspace { node, cmd } => workspace(node.node(), cmd)?,
        Cmd::Daemon { node, debounce_ms, rescan_s, spares } => {
            let o = daemon::Options {
                debounce: std::time::Duration::from_millis(debounce_ms),
                rescan: std::time::Duration::from_secs(rescan_s),
                spares,
            };
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            daemon::run(&node.node(), &o, stop, &mut |line| println!("{line}"))?;
        }
        Cmd::ApiDoc { json } => {
            let tools = treeyard::rpc::tools();
            if json {
                println!("{}", serde_json::to_string_pretty(&json!({"tools": tools}))?);
            } else {
                print!("{}", treeyard::apidoc::markdown(&tools));
            }
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
