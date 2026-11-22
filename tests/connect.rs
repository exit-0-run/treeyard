//! `treeyard connect`, `connect status` and `disconnect` on throwaway repositories, through the
//! built binary. Every run gets empty HOME, CODEX_HOME and XDG_CONFIG_HOME directories, and the
//! tests check that they stay empty: connect writes project-scoped config only. Ports are free
//! ports picked by the OS. No agent runs here.

mod common;

use common::{fixture_repo, git};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_treeyard"))
}

/// A repository plus the empty user-level config dirs that every command runs with.
struct Env {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    user: Vec<PathBuf>,
}

impl Env {
    fn new(extra: &[(&str, &str)]) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(fixture_repo(tmp.path(), 2, 5, extra)).unwrap();
        let user: Vec<PathBuf> = ["home", "codex-home", "xdg-config"].iter().map(|d| tmp.path().join(d)).collect();
        for d in &user {
            std::fs::create_dir_all(d).unwrap();
        }
        Env { _tmp: tmp, repo, user }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(bin());
        c.args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.user[0])
            .env("CODEX_HOME", &self.user[1])
            .env("XDG_CONFIG_HOME", &self.user[2])
            .env_remove("TREEYARD_URL")
            .env_remove("TREEYARD_TOKEN")
            .env_remove("TREEYARD_AGENT")
            .stdin(Stdio::null());
        c
    }

    /// Runs the binary; panics on failure; returns stdout.
    fn run(&self, args: &[&str]) -> String {
        let out = self.cmd(args).output().unwrap();
        let (so, se) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "treeyard {} failed:\n{so}\n{se}", args.join(" "));
        so.into_owned()
    }

    fn state_dir(&self) -> PathBuf {
        self.repo.join(".git/treeyard")
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.state_dir().join("connect.json")).unwrap()).unwrap()
    }

    fn user_dirs_empty(&self) -> bool {
        self.user.iter().all(|d| std::fs::read_dir(d).unwrap().next().is_none())
    }
}

impl Drop for Env {
    /// A failed test must not leave a server running.
    fn drop(&mut self) {
        if self.state_dir().join("connect.json").exists() {
            let _ = self.cmd(&["disconnect", "--purge"]).output();
        }
    }
}

/// Hash of every path, file mode and file body under `root`, minus `skip` (relative paths).
fn tree_hash(root: &Path, skip: &[&str]) -> String {
    fn walk(root: &Path, dir: &Path, skip: &[&str], out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for p in entries {
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if skip.iter().any(|s| rel == *s || rel.starts_with(&format!("{s}/"))) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&p).unwrap();
            if meta.is_dir() {
                out.push((rel + "/", String::new()));
                walk(root, &p, skip, out);
            } else {
                use std::os::unix::fs::PermissionsExt;
                let body = std::fs::read(&p).unwrap();
                out.push((rel, format!("{:o} {}", meta.permissions().mode(), hex::encode(Sha256::digest(&body)))));
            }
        }
    }
    let mut all = Vec::new();
    walk(root, root, skip, &mut all);
    let mut h = Sha256::new();
    for (p, v) in &all {
        h.update(format!("{p}\0{v}\n").as_bytes());
    }
    format!("{} entries, {}", all.len(), hex::encode(h.finalize()))
}

fn pid_alive(pid: u64) -> bool {
    treeyard::connect::pid_alive(pid as u32)
}

fn status(url: &str) -> Value {
    treeyard::client::call(url, "status", json!({})).unwrap()
}

/// Replaces the machine-specific parts of a generated config with fixed words.
fn normalise(text: &str, env: &Env, port: u64) -> String {
    text.replace(&env.state_dir().to_string_lossy().into_owned(), "STATE")
        .replace(&bin().to_string_lossy().into_owned(), "TREEYARD")
        .replace(&format!("127.0.0.1:{port}"), "127.0.0.1:PORT")
}

#[test]
fn connect_initialises_serves_on_loopback_and_runs_twice_without_change() {
    let env = Env::new(&[]);
    assert!(!env.state_dir().exists());
    let out = env.run(&["connect", "--harness", "claude"]);
    assert!(out.contains("imported main"), "{out}");
    assert!(env.state_dir().join("data/treeyard.db").is_file(), "connect ran init");
    let st = env.state();
    let (pid, port) = (st["pid"].as_u64().unwrap(), st["port"].as_u64().unwrap());
    assert!(pid_alive(pid), "the server runs");
    assert!(port != 0 && port != 7420, "a free port, not the default one: {port}");
    assert_eq!(st["url"], format!("http://127.0.0.1:{port}"), "loopback only");
    assert_eq!(std::fs::read_to_string(env.state_dir().join("port")).unwrap().trim(), port.to_string());
    assert_eq!(status(st["url"].as_str().unwrap())["branch"], "main");
    assert!(env.user_dirs_empty(), "no user-level config");

    let before = tree_hash(&env.repo, &[".git/treeyard/data"]);
    let again = env.run(&["connect", "--harness", "claude"]);
    assert!(again.starts_with("already connected"), "{again}");
    assert_eq!(tree_hash(&env.repo, &[".git/treeyard/data"]), before, "a second connect writes nothing");
    assert_eq!(env.state()["pid"].as_u64(), Some(pid), "same server");

    // A dead server is started again on the recorded port; the configs stay as they are.
    let cfg = std::fs::read(env.repo.join(".mcp.json")).unwrap();
    // SAFETY: the pid is the server this test started.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    for _ in 0..100 {
        if !pid_alive(pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    env.run(&["connect"]);
    let st = env.state();
    assert_ne!(st["pid"].as_u64(), Some(pid));
    assert_eq!(st["port"].as_u64(), Some(port), "the recorded port");
    assert_eq!(std::fs::read(env.repo.join(".mcp.json")).unwrap(), cfg);
    env.run(&["disconnect"]);
}

#[test]
fn connect_writes_project_scoped_config_for_three_harnesses() {
    let env = Env::new(&[]);
    env.run(&["connect", "--api", "--harness", "all", "--agents", "3"]);
    let port = env.state()["port"].as_u64().unwrap();
    let read = |p: &str| normalise(&std::fs::read_to_string(env.repo.join(p)).unwrap(), &env, port);
    assert_eq!(
        read(".mcp.json"),
        r#"{
  "mcpServers": {
    "treeyard": {
      "args": [
        "mcp",
        "--agent-pool",
        "claude-1,claude-2,claude-3",
        "--pool-dir",
        "STATE/agents",
        "--tools",
        "agent"
      ],
      "command": "TREEYARD",
      "env": {
        "TREEYARD_URL": "http://127.0.0.1:PORT"
      }
    }
  }
}
"#
    );
    assert_eq!(
        read(".codex/config.toml"),
        r#"# treeyard:connect:start
[mcp_servers.treeyard]
command = "TREEYARD"
args = ["mcp", "--agent-pool", "codex-1,codex-2,codex-3", "--pool-dir", "STATE/agents", "--tools", "agent"]
default_tools_approval_mode = "approve"

[mcp_servers.treeyard.env]
TREEYARD_URL = "http://127.0.0.1:PORT"
# treeyard:connect:end
"#
    );
    assert_eq!(
        read("opencode.json"),
        r#"{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "treeyard": {
      "command": [
        "TREEYARD",
        "mcp",
        "--agent-pool",
        "opencode-1,opencode-2,opencode-3",
        "--pool-dir",
        "STATE/agents",
        "--tools",
        "agent"
      ],
      "enabled": true,
      "environment": {
        "TREEYARD_URL": "http://127.0.0.1:PORT"
      },
      "type": "local"
    }
  }
}
"#
    );
    let exclude = std::fs::read_to_string(env.repo.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.ends_with(
            "# treeyard:connect:start\n/.mcp.json\n/.codex/config.toml\n/opencode.json\n/AGENTS.md\n/CLAUDE.md\n# treeyard:connect:end\n"
        ),
        "{exclude}"
    );
    assert_eq!(git(&env.repo, &["status", "--porcelain"]), "", "created files are excluded from git");
    assert!(env.user_dirs_empty(), "no user-level config");

    // One agent: a plain --agent name. --all-tools: the bridge lists every operation.
    let one = Env::new(&[]);
    one.run(&["connect", "--harness", "codex"]);
    let p1 = one.state()["port"].as_u64().unwrap();
    let toml = normalise(&std::fs::read_to_string(one.repo.join(".codex/config.toml")).unwrap(), &one, p1);
    assert!(toml.contains(r#"args = ["mcp", "--agent", "codex-1", "--tools", "native"]"#), "{toml}");
    one.run(&["connect", "--harness", "codex", "--api"]);
    let toml = normalise(&std::fs::read_to_string(one.repo.join(".codex/config.toml")).unwrap(), &one, p1);
    assert!(toml.contains(r#"args = ["mcp", "--agent", "codex-1", "--tools", "agent"]"#), "{toml}");
    one.run(&["connect", "--harness", "codex", "--all-tools"]);
    let toml = normalise(&std::fs::read_to_string(one.repo.join(".codex/config.toml")).unwrap(), &one, p1);
    assert!(toml.contains(r#"args = ["mcp", "--agent", "codex-1"]"#), "{toml}");
    assert_eq!(one.state()["all_tools"], true);
    assert!(!one.repo.join(".mcp.json").exists() && !one.repo.join("CLAUDE.md").exists(), "only codex files");
    assert!(one.repo.join("AGENTS.md").exists());
    one.run(&["disconnect"]);
    env.run(&["disconnect"]);
}

#[test]
fn instruction_block_leaves_the_rest_of_the_file_byte_identical() {
    let agents = "# Agents\n\nRun `make test`.";
    let claude = "# Claude\n\n@AGENTS.md\n";
    let env = Env::new(&[("AGENTS.md", agents), ("CLAUDE.md", claude)]);
    env.run(&["connect", "--api", "--harness", "codex"]);
    for (f, orig) in [("AGENTS.md", agents), ("CLAUDE.md", claude)] {
        let now = std::fs::read_to_string(env.repo.join(f)).unwrap();
        assert!(now.starts_with(orig), "{f} keeps its bytes before the block:\n{now}");
        assert_eq!(now.matches(treeyard::connect::MD_START).count(), 1, "{f}: one block");
        let block = &now[now.find(treeyard::connect::MD_START).unwrap()..];
        assert!(block.trim_end().ends_with(treeyard::connect::MD_END), "{f}: the block is last");
        for word in ["intent", "apply", "overlap", "conflict", "land", "checkout"] {
            assert!(block.contains(word), "{f}: the block names {word}");
        }
        let body = &block[treeyard::connect::MD_START.len() + 1..block.find(treeyard::connect::MD_END).unwrap()];
        assert_eq!(body, treeyard::connect::INSTRUCTIONS, "{f}: the block is the constant");
    }
    // More agents and one more harness: still one block per file.
    env.run(&["connect", "--api", "--harness", "codex,claude", "--agents", "2"]);
    for f in ["AGENTS.md", "CLAUDE.md"] {
        let now = std::fs::read_to_string(env.repo.join(f)).unwrap();
        assert_eq!(now.matches(treeyard::connect::MD_START).count(), 1, "{f}: one block after a second connect");
    }
    env.run(&["disconnect"]);
    assert_eq!(std::fs::read_to_string(env.repo.join("AGENTS.md")).unwrap(), agents);
    assert_eq!(std::fs::read_to_string(env.repo.join("CLAUDE.md")).unwrap(), claude);
}

/// `bench/t2/<file>` is the block between the markers, as connect writes it (the kit refuses any other).
fn bench_block(file: &str, body: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/t2").join(file);
    let want = format!("{}\n{body}{}\n", treeyard::connect::MD_START, treeyard::connect::MD_END);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), want, "{file} is the constant");
}

#[test]
fn the_bench_blocks_are_the_constants() {
    bench_block("block-b.md", treeyard::connect::INSTRUCTIONS);
    bench_block("block-bn.md", treeyard::connect::NATIVE_INSTRUCTIONS);
    let points = treeyard::connect::NATIVE_INSTRUCTIONS.lines().filter(|l| l.starts_with(char::is_numeric)).count();
    assert!(points <= 6, "the native block is short: {points} points");
}

#[test]
fn connect_is_native_by_default_and_switches_to_api_and_back() {
    let agents = "# Agents\n";
    let env = Env::new(&[("AGENTS.md", agents)]);
    env.run(&["connect", "--harness", "claude,codex", "--no-mirror"]);
    let native_cfg = std::fs::read(env.repo.join(".mcp.json")).unwrap();
    // `--native` is accepted and changes nothing.
    env.run(&["connect", "--native"]);
    assert_eq!(std::fs::read(env.repo.join(".mcp.json")).unwrap(), native_cfg);
    let mcp: Value = serde_json::from_slice(&std::fs::read(env.repo.join(".mcp.json")).unwrap()).unwrap();
    let args: Vec<&str> =
        mcp["mcpServers"]["treeyard"]["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert!(args.windows(2).any(|w| w == ["--tools", "native"]), "{args:?}");
    let codex = std::fs::read_to_string(env.repo.join(".codex/config.toml")).unwrap();
    assert!(codex.contains("\"native\""), "{codex}");
    for f in ["AGENTS.md", "CLAUDE.md"] {
        let now = std::fs::read_to_string(env.repo.join(f)).unwrap();
        let start = now.find(treeyard::connect::MD_START).unwrap() + treeyard::connect::MD_START.len() + 1;
        let body = &now[start..now.find(treeyard::connect::MD_END).unwrap()];
        assert_eq!(body, treeyard::connect::NATIVE_INSTRUCTIONS, "{f}");
    }
    assert_eq!(env.state()["native"], true);
    assert!(env.run(&["connect", "status"]).contains("mode: native"));
    assert!(env.cmd(&["connect", "--native", "--all-tools"]).output().unwrap().status.code() != Some(0));
    assert!(env.cmd(&["connect", "--native", "--api"]).output().unwrap().status.code() != Some(0));
    // `--api`: the API tool set and block, byte for byte.
    env.run(&["connect", "--api"]);
    assert!(env.run(&["connect", "status"]).contains("mode: api"));
    let now = std::fs::read_to_string(env.repo.join("AGENTS.md")).unwrap();
    assert!(now.contains(treeyard::connect::INSTRUCTIONS), "{now}");
    let mcp: Value = serde_json::from_slice(&std::fs::read(env.repo.join(".mcp.json")).unwrap()).unwrap();
    assert!(mcp["mcpServers"]["treeyard"]["args"].as_array().unwrap().iter().any(|a| a == "agent"));
    // And back: native again.
    env.run(&["connect"]);
    assert_eq!(std::fs::read(env.repo.join(".mcp.json")).unwrap(), native_cfg);
    env.run(&["disconnect"]);
    assert_eq!(std::fs::read_to_string(env.repo.join("AGENTS.md")).unwrap(), agents);
    assert!(env.user_dirs_empty());
}

#[test]
fn connect_sets_and_clears_the_land_test() {
    let env = Env::new(&[]);
    env.run(&[
        "connect",
        "--harness",
        "claude",
        "--no-mirror",
        "--land-test",
        "make test",
        "--land-test-timeout",
        "120",
    ]);
    let pid = env.state()["pid"].as_u64().unwrap();
    let st = env.run(&["connect", "status", "--json"]);
    let st: Value = serde_json::from_str(&st).unwrap();
    assert_eq!(st["land_test"], "make test");
    let url = st["url"].as_str().unwrap().to_string();
    let ch = treeyard::client::call(&url, "open_change", json!({"agent": "a", "intent": "x"})).unwrap();
    let ch = treeyard::client::call(&url, "change", json!({"change": ch["id"]})).unwrap();
    assert_eq!(ch["land_test"], json!({"command": "make test", "timeout_s": 120}), "the server reads it");
    // The same flag again: nothing to restart.
    env.run(&["connect", "--land-test", "make test"]);
    assert_eq!(env.state()["pid"].as_u64(), Some(pid));
    env.run(&["connect", "--no-land-test"]);
    assert!(env.run(&["connect", "status"]).contains("land test: none"));
    env.run(&["disconnect"]);
}

#[test]
fn connect_sets_and_clears_the_land_queue() {
    let env = Env::new(&[]);
    let mode = || treeyard::config::Config::load(&env.state_dir().join("data")).unwrap().landing.mode;
    let queue = treeyard::config::Mode::Queue;
    env.run(&["connect", "--harness", "claude", "--no-mirror", "--land-test", "make test"]);
    env.run(&["connect", "--land-queue", "make test", "--land-queue-setup", "make deps", "--land-batch", "4"]);
    let st: Value = serde_json::from_str(&env.run(&["connect", "status", "--json"])).unwrap();
    assert_eq!(st["land_queue"], json!({"test": "make test", "setup": "make deps", "batch": 4}), "{st}");
    assert!(st["land_test"].is_null(), "the queue replaced the land test: {st}");
    assert!(env.run(&["connect", "status"]).contains("land queue: make test (batch 4, setup make deps)"));
    let url = st["url"].as_str().unwrap().to_string();
    let ch = treeyard::client::call(&url, "open_change", json!({"agent": "a", "intent": "x"})).unwrap();
    let ch = treeyard::client::call(&url, "change", json!({"change": ch["id"]})).unwrap();
    assert_eq!(ch["land_queue"]["batch"], 4, "the server reads it: {ch}");
    assert!(ch.get("land_test").is_none());
    // Both at once is a usage error.
    let out = env.cmd(&["connect", "--land-test", "x", "--land-queue", "y"]).output().unwrap();
    assert!(!out.status.success());
    env.run(&["connect", "--no-land-queue"]);
    assert_ne!(mode(), queue);
    // Disconnect takes back a queue that connect set (the data stays).
    env.run(&["connect", "--land-queue", "make test"]);
    assert_eq!(mode(), queue);
    env.run(&["disconnect"]);
    assert_ne!(mode(), queue);
}

fn undo_case(extra: &[(&str, &str)], purge: bool) {
    let env = Env::new(extra);
    // Untracked user files count too.
    std::fs::write(env.repo.join("notes.txt"), "not in git\n").unwrap();
    let skip: &[&str] = if purge { &[] } else { &[".git/treeyard"] };
    let before = tree_hash(&env.repo, skip);
    env.run(&["connect", "--harness", "all", "--agents", "2"]);
    let pid = env.state()["pid"].as_u64().unwrap();
    assert_ne!(tree_hash(&env.repo, skip), before, "connect changed files");
    let out = env.run(if purge { &["disconnect", "--purge"] } else { &["disconnect"] });
    assert!(out.contains("stopped server"), "{out}");
    assert!(!out.contains("warning"), "{out}");
    assert_eq!(tree_hash(&env.repo, skip), before, "disconnect restores every byte ({extra:?}, purge {purge})");
    assert!(!pid_alive(pid), "the server stopped");
    let again = env.run(&["disconnect"]);
    assert_eq!(again.trim(), "not connected");
    assert_eq!(tree_hash(&env.repo, skip), before, "a second disconnect changes nothing");
    assert!(env.user_dirs_empty());
}

#[test]
fn disconnect_restores_a_repo_without_agent_files() {
    undo_case(&[], true);
}

#[test]
fn disconnect_restores_a_repo_with_agent_files_and_configs() {
    undo_case(
        &[
            ("AGENTS.md", "# Agents\nno newline at the end"),
            ("CLAUDE.md", "# Claude\n"),
            (".mcp.json", "{\n    \"mcpServers\": {\"other\": {\"command\": \"other-mcp\"}}\n}\n"),
            ("opencode.json", "{\"$schema\": \"https://opencode.ai/config.json\", \"model\": \"x/y\"}"),
            (".codex/config.toml", "model = \"o4\"\n\n[mcp_servers.other]\ncommand = \"other-mcp\"\n"),
        ],
        false,
    );
}

#[test]
fn disconnect_keeps_edits_made_after_connect() {
    let env = Env::new(&[("AGENTS.md", "# Agents\n")]);
    env.run(&["connect", "--harness", "claude,codex"]);
    let mut f = std::fs::OpenOptions::new().append(true).open(env.repo.join("AGENTS.md")).unwrap();
    writeln!(f, "A line the user added after connect.").unwrap();
    let out = env.run(&["disconnect"]);
    assert!(out.contains("warning: AGENTS.md changed after connect"), "{out}");
    let now = std::fs::read_to_string(env.repo.join("AGENTS.md")).unwrap();
    assert!(!now.contains("treeyard:connect"), "the block is gone: {now}");
    assert!(now.starts_with("# Agents\n") && now.contains("A line the user added"), "{now}");
    assert!(!env.repo.join(".mcp.json").exists() && !env.repo.join(".codex").exists());
}

/// One stdio MCP bridge started exactly as the harness starts it, from `.mcp.json`.
struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    id: u64,
}

impl Bridge {
    fn from_mcp_json(env: &Env) -> Bridge {
        let cfg: Value = serde_json::from_slice(&std::fs::read(env.repo.join(".mcp.json")).unwrap()).unwrap();
        let s = &cfg["mcpServers"]["treeyard"];
        let args: Vec<String> = s["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
        let mut child = Command::new(s["command"].as_str().unwrap())
            .args(&args)
            .env("TREEYARD_URL", s["env"]["TREEYARD_URL"].as_str().unwrap())
            .env_remove("TREEYARD_TOKEN")
            .env_remove("TREEYARD_AGENT")
            .current_dir(&env.repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut b = Bridge { child, stdin, stdout, id: 0 };
        b.rpc("initialize", json!({}));
        b
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        assert!(r["result"]["isError"] != true, "{name}: {r}");
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    fn close(mut self) {
        drop(self.stdin);
        self.child.wait().unwrap();
    }
}

#[test]
fn status_shows_two_agents_that_edit_one_file() {
    let env = Env::new(&[]);
    env.run(&["connect", "--harness", "claude", "--agents", "2"]);
    let url = env.state()["url"].as_str().unwrap().to_string();

    let mut a = Bridge::from_mcp_json(&env);
    let mut b = Bridge::from_mcp_json(&env);
    let ca = a.tool("open_change", json!({"intent": "rename the first line", "paths": ["f00.txt"]}));
    let cb = b.tool("open_change", json!({"intent": "number the first line", "paths": ["f00.txt"]}));
    assert_eq!(ca["agent"], "claude-1", "the first bridge takes the first name of the pool");
    assert_eq!(cb["agent"], "claude-2", "the second bridge takes the next free name");
    let edit = |new: &str| json!([{"path": "f00.txt", "replace": {"old": "file 0 line 0", "new": new}}]);
    a.tool("apply", json!({"change": ca["id"], "edits": edit("FIRST LINE")}));
    let rb = b.tool("apply", json!({"change": cb["id"], "edits": edit("line number 1")}));
    assert_eq!(rb["collisions"][0]["kind"], "conflict", "{rb}");

    let text = env.run(&["connect", "status"]);
    assert!(text.contains(&format!("server: {url} (pid ")) && text.contains("running"), "{text}");
    assert!(text.contains("claude: .mcp.json, agents claude-1, claude-2"), "{text}");
    assert!(text.contains("open changes: 2"), "{text}");
    for (c, agent, intent) in [(&ca, "claude-1", "rename the first line"), (&cb, "claude-2", "number the first line")] {
        let id = c["id"].as_str().unwrap();
        assert!(text.contains(&format!("{id} {agent} \"{intent}\" paths: f00.txt")), "{text}");
    }
    assert!(text.contains("warnings: 2"), "{text}");
    assert!(text.contains("conflict: claude-2"), "{text}");
    assert!(text.contains("in f00.txt with"), "{text}");

    let v: Value = serde_json::from_str(&env.run(&["connect", "status", "--json"])).unwrap();
    assert_eq!(v["url"], url.as_str());
    assert_eq!(v["server_alive"], true);
    assert_eq!(v["harnesses"][0]["name"], "claude");
    assert_eq!(v["changes"].as_array().unwrap().len(), 2);
    let kinds: Vec<&str> = v["warnings"].as_array().unwrap().iter().map(|w| w["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["conflict", "conflict"], "{v}");

    // A third bridge finds both names taken; a closed bridge frees its name.
    let mut c = Bridge::from_mcp_json(&env);
    let cc = c.tool("open_change", json!({"intent": "third"}));
    assert!(cc["agent"].as_str().unwrap().starts_with("claude-1-"), "{cc}");
    a.close();
    let mut d = Bridge::from_mcp_json(&env);
    assert_eq!(d.tool("open_change", json!({"intent": "fourth"}))["agent"], "claude-1");
    for x in [b, c, d] {
        x.close();
    }
    env.run(&["disconnect"]);
    assert_eq!(env.run(&["connect", "status"]).trim(), "not connected (run `treeyard connect`)");
}

#[test]
fn mirror_starts_only_with_the_flag() {
    let env = Env::new(&[]);
    let origin = env.repo.parent().unwrap().join("origin.git");
    git(env.repo.parent().unwrap(), &["clone", "-q", "--bare", "src", "origin.git"]);
    git(&env.repo, &["remote", "add", "origin", origin.to_str().unwrap()]);

    // stdin is not a TTY (the test runs it with stdin null): no mirror, one hint line.
    let out = env.run(&["connect", "--harness", "claude"]);
    assert!(out.contains("mirror is off. `treeyard connect --mirror`"), "{out}");
    assert!(env.state()["mirror"].is_null());

    let out = env.run(&["connect", "--mirror"]);
    assert!(out.contains("mirror: pid"), "{out}");
    let pid = env.state()["mirror"]["pid"].as_u64().unwrap();
    assert!(pid_alive(pid));
    assert!(env.run(&["connect"]).starts_with("already connected"), "a running mirror stays on");

    let out = env.run(&["disconnect"]);
    assert!(out.contains(&format!("stopped mirror (pid {pid})")), "{out}");
    assert!(!pid_alive(pid));
}

#[test]
fn connect_refuses_outside_a_git_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(bin()).arg("connect").current_dir(tmp.path()).stdin(Stdio::null()).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a git repository"));
}

#[test]
fn codex_tools_approval_is_written_shown_and_undone() {
    let user = "model = \"some-model\"\n\n[mcp_servers.other]\ncommand = \"other\"\n";
    let env = Env::new(&[(".codex/config.toml", user)]);
    env.run(&["connect", "--harness", "codex"]);
    let text = std::fs::read_to_string(env.repo.join(".codex/config.toml")).unwrap();
    assert!(text.starts_with(user), "the user's lines stay first:\n{text}");
    let v: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(v["mcp_servers"]["treeyard"]["default_tools_approval_mode"].as_str(), Some("approve"));
    assert_eq!(v["mcp_servers"]["other"].get("default_tools_approval_mode"), None, "only its own entry");
    assert_eq!(v["model"].as_str(), Some("some-model"));

    let status = env.run(&["connect", "status"]);
    assert!(status.contains("codex: .codex/config.toml, tools approval approve, agents codex-1"), "{status}");
    let j: Value = serde_json::from_str(&env.run(&["connect", "status", "--json"])).unwrap();
    assert_eq!(j["harnesses"][0]["tools_approval"], "approve");

    env.run(&["disconnect"]);
    assert_eq!(std::fs::read_to_string(env.repo.join(".codex/config.toml")).unwrap(), user, "byte-identical");
    assert!(env.user_dirs_empty(), "nothing in CODEX_HOME");
}
