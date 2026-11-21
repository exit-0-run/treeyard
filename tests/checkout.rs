//! The `checkout` tool of the stdio MCP bridge: the built `treeyard mcp`, driven with JSON-RPC on
//! stdio against a real server, as a harness drives it. The agent runs the repository's own test
//! script in the directory that `checkout` returns, and the tree it tested is the tree that lands.

mod common;

use common::{git, spawn_server};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use treeyard::repo::Repo;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_treeyard"))
}

const LIB: &str = "status: broken\n\n\n\nnotes\n";

/// A repo with a test script (exec bit), a symlink, a file to delete and a `.gitignore`.
fn repo(root: &Path) -> PathBuf {
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("lib.txt"), LIB).unwrap();
    std::fs::write(src.join("old.txt"), "to delete\n").unwrap();
    std::fs::write(src.join(".gitignore"), "build/\n").unwrap();
    std::fs::write(src.join("run_tests.sh"), "#!/bin/sh\ngrep -q '^status: fixed' lib.txt\n").unwrap();
    std::fs::set_permissions(src.join("run_tests.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("lib.txt", src.join("link.txt")).unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "fixture"]);
    src
}

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    id: u64,
}

impl Bridge {
    /// `treeyard mcp` in `cwd` with `TMPDIR` = `tmp` (where default checkouts go).
    fn start(url: &str, agent: &str, cwd: &Path, tmp: &Path) -> Bridge {
        let mut child = Command::new(bin())
            .args(["mcp", "--url", url, "--agent", agent])
            .env("TMPDIR", tmp)
            .env_remove("TREEYARD_TOKEN")
            .current_dir(cwd)
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

    /// (is_error, text)
    fn raw(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        (r["result"]["isError"] == true, r["result"]["content"][0]["text"].as_str().unwrap().to_string())
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        let (err, text) = self.raw(name, args);
        assert!(!err, "{name}: {text}");
        serde_json::from_str(&text).unwrap()
    }

    fn close(mut self) {
        drop(self.stdin);
        self.child.wait().unwrap();
    }
}

fn server(root: &Path, src: &Path) -> String {
    let data = root.join("data");
    Repo::init(&data, src, "main", None).unwrap();
    spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

fn sh_ok(dir: &Path, script: &str) -> bool {
    Command::new("sh").arg("-c").arg(script).current_dir(dir).status().unwrap().success()
}

/// Every file of the tree at `at` has the same bytes and mode in `dir`.
fn same_tree(url: &str, at: &str, dir: &Path) {
    let tree = common::call(url, "tree", json!({"at": at}));
    for row in tree["entries"].as_array().unwrap() {
        let (path, mode, blob) = (row[0].as_str().unwrap(), row[1].as_u64().unwrap(), row[2].as_str().unwrap());
        let p = dir.join(path);
        let md = std::fs::symlink_metadata(&p).unwrap_or_else(|_| panic!("{path} missing in {}", dir.display()));
        let data = if md.file_type().is_symlink() {
            std::fs::read_link(&p).unwrap().to_string_lossy().into_owned().into_bytes()
        } else {
            std::fs::read(&p).unwrap()
        };
        assert_eq!(treeyard::git::object_id("blob", &data), blob, "{path}: bytes");
        let disk = if md.file_type().is_symlink() {
            0o120000
        } else if md.permissions().mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        };
        assert_eq!(disk, mode, "{path}: mode");
    }
}

#[test]
fn checkout_runs_the_real_tests_on_the_change_and_follows_it_to_land() {
    let tmp = tempfile::tempdir().unwrap();
    let src = repo(tmp.path());
    let url = server(tmp.path(), &src);
    let tmpdir = tmp.path().join("tmpdir");
    std::fs::create_dir_all(&tmpdir).unwrap();
    // The bridge runs in the repository checkout (connect without a workspace).
    let mut b = Bridge::start(&url, "agent-a", &src, &tmpdir);
    let tools = b.rpc("tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "checkout"));
    assert!(!treeyard::rpc::tools().iter().any(|t| t["name"] == "checkout"), "only the local bridge has it");
    assert!(treeyard::connect::INSTRUCTIONS.contains("run the repository's tests in `checkout.dir`"), "the block names it");

    let id = b.tool("open_change", json!({"intent": "fix the status", "paths": ["lib.txt"]}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    b.tool(
        "apply",
        json!({"change": id, "edits": [
            {"path": "lib.txt", "replace": {"old": "broken", "new": "fixed"}},
            {"path": "old.txt", "delete": true},
            {"path": "tools/new.sh", "content": "#!/bin/sh\necho new\n", "executable": true},
        ]}),
    );
    let co = b.tool("checkout", json!({"change": id}));
    let dir = PathBuf::from(co["dir"].as_str().unwrap());
    assert!(dir.starts_with(&tmpdir), "default checkout under TMPDIR: {}", dir.display());
    assert_eq!(co["main_moved"], false);
    assert!(sh_ok(&dir, "./run_tests.sh"), "the repository's test passes on the change");
    assert!(!dir.join("old.txt").exists(), "a deleted file is gone");
    assert!(std::fs::symlink_metadata(dir.join("link.txt")).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(dir.join("link.txt")).unwrap(), LIB.replace("broken", "fixed"));
    same_tree(&url, &id, &dir);
    assert!(!sh_ok(&src, "./run_tests.sh"), "the bridge's own directory is not touched");
    assert_eq!(git(&src, &["status", "--porcelain"]), "");

    // Edits in the checkout never count: the next apply puts the change back and says so.
    std::fs::write(dir.join("lib.txt"), LIB.replace("broken", "fixed by hand")).unwrap();
    std::fs::write(dir.join("notes.txt"), "scratch\n").unwrap();
    std::fs::create_dir_all(dir.join("build")).unwrap();
    std::fs::write(dir.join("build/out.o"), "object\n").unwrap();
    let r = b.tool(
        "apply",
        json!({"change": id, "edits": [{"path": "lib.txt", "replace": {"old": "fixed", "new": "fixed again"}}]}),
    );
    assert_eq!(r["checkout"]["dir"], co["dir"], "apply rewrites the checkout: {r}");
    assert_eq!(r["checkout"]["reverted"], json!(["lib.txt"]));
    assert_eq!(r["checkout"]["extra"], json!(["notes.txt"]), "gitignored build output is not listed");
    assert_eq!(r["checkout"]["written"], 1);
    assert_eq!(std::fs::read_to_string(dir.join("lib.txt")).unwrap(), LIB.replace("broken", "fixed again"));
    assert!(dir.join("build/out.o").exists(), "build output stays");
    assert!(sh_ok(&dir, "./run_tests.sh"));

    // Main moves in other files: no action, no advice, one short field.
    let other = common::call(
        &url,
        "apply",
        json!({"agent": "agent-b", "intent": "add docs", "edits": [{"path": "docs.txt", "content": "docs\n"}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(common::call(&url, "land", json!({"change": other}))["status"], "landed");
    let again = b.tool("checkout", json!({"change": id}));
    assert_eq!(
        (again["main_moved"].clone(), again["main_moved_elsewhere"].clone()),
        (json!(false), json!(true)),
        "{again}"
    );
    assert!(!again.to_string().contains("rebase"), "no rebase advice: {again}");
    assert_eq!(again["written"], 0, "nothing changed in the change itself");
    // Main moves in a file the change edits: the checkout says which, and rebase brings it in.
    let third = common::call(&url, "apply", json!({"agent": "agent-c", "intent": "notes", "edits": [{"path": "lib.txt", "replace": {"old": "notes", "new": "notes by c"}}]}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(common::call(&url, "land", json!({"change": third}))["status"], "landed");
    let again = b.tool("checkout", json!({"change": id}));
    assert_eq!(
        (again["main_moved"].clone(), again["main_moved_in"].clone()),
        (json!(true), json!(["lib.txt"])),
        "{again}"
    );
    assert!(again["advice"][0].as_str().unwrap().contains("Call rebase"), "{again}");
    let rb = b.tool("rebase", json!({"change": id}));
    assert_eq!(rb["checkout"]["main_moved"], false, "{rb}");
    assert!(rb["checkout"].get("main_moved_elsewhere").is_none(), "{rb}");
    assert!(dir.join("docs.txt").exists());
    assert!(std::fs::read_to_string(dir.join("lib.txt")).unwrap().contains("notes by c"));
    assert!(sh_ok(&dir, "./run_tests.sh"));

    // Land: the tree that landed is the tree that was tested.
    let landed = b.tool("land", json!({"change": id}));
    assert_eq!(landed["status"], "landed", "{landed}");
    same_tree(&url, "main", &dir);

    // A directory that is not a checkout is refused, whatever is in it.
    let (err, text) = b.raw("checkout", json!({"change": id, "into": src.to_str().unwrap()}));
    assert!(err && text.contains("not a checkout"), "{text}");
    assert_eq!(git(&src, &["status", "--porcelain"]), "");
    b.close();

    // The CLI does the same for scripts; a second run writes nothing.
    let into = tmp.path().join("cli-checkout");
    let run = |args: &[&str]| -> Value {
        let out = Command::new(bin()).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let first = run(&["checkout", &id, "--into", into.to_str().unwrap(), "--url", &url]);
    assert!(first["written"].as_u64().unwrap() >= 5, "{first}");
    let second = run(&["checkout", &id, "--into", into.to_str().unwrap(), "--url", &url]);
    assert_eq!(second["written"], 0);
    same_tree(&url, "main", &into);
}

#[test]
fn checkout_in_a_workspace_keeps_the_workspace_and_the_collision_warnings() {
    let tmp = tempfile::tempdir().unwrap();
    let src = repo(tmp.path());
    let url = server(tmp.path(), &src);
    let home = tmp.path().join("node");
    let tmpdir = tmp.path().join("tmpdir");
    std::fs::create_dir_all(&tmpdir).unwrap();
    // The orchestrator made a workspace with `workspace-hook create`; the agent runs in it.
    let out = Command::new(bin())
        .args(["workspace-hook", "create", "--name", "agent-1", "--agent", "agent-a", "--url", &url])
        .args(["--home", home.to_str().unwrap()])
        .current_dir(&src)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let ws = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let ws_change = common::call(&url, "changes", json!({}))["changes"][0]["id"].as_str().unwrap().to_string();

    let mut b = Bridge::start(&url, "agent-a", &ws, &tmpdir);
    let id = b.tool("open_change", json!({"intent": "fix", "paths": ["lib.txt"]}))["id"].as_str().unwrap().to_string();
    b.tool(
        "apply",
        json!({"change": id, "edits": [{"path": "lib.txt", "replace": {"old": "broken", "new": "fixed"}}]}),
    );
    let co = b.tool("checkout", json!({"change": id}));
    let dir = PathBuf::from(co["dir"].as_str().unwrap());
    assert!(!dir.starts_with(&ws), "the checkout is not the workspace");
    assert!(sh_ok(&dir, "./run_tests.sh"));

    // Another agent edits the same line: the apply reply still carries the conflict.
    let other = common::call(&url, "open_change", json!({"agent": "agent-b", "intent": "other status"}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    common::call(
        &url,
        "apply",
        json!({"change": other, "edits": [{"path": "lib.txt", "replace": {"old": "broken", "new": "unknown"}}]}),
    );
    let r = b.tool(
        "apply",
        json!({"change": id, "edits": [{"path": "lib.txt", "replace": {"old": "fixed", "new": "fixed!"}}]}),
    );
    let kinds: Vec<&str> = r["collisions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["files"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, ["conflict"], "{r}");
    assert_eq!(r["checkout"]["written"], 1);

    // The workspace directory and its own change are untouched.
    assert_eq!(std::fs::read_to_string(ws.join("lib.txt")).unwrap(), LIB);
    let wc = common::call(&url, "change", json!({"change": ws_change}));
    assert_eq!(wc["paths"], json!([]), "{wc}");
    b.close();
}
