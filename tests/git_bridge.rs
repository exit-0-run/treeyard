mod common;

use common::*;
use treeyard::{git, repo::Repo};
use serde_json::json;
use std::sync::Arc;

/// Names that exercise git's tree ordering (a directory sorts as if it ended in '/').
const TRICKY: &[(&str, &str)] = &[
    ("a.b", "dot\n"),
    ("a/b.txt", "nested\n"),
    ("a/c/d.txt", "deeper\n"),
    ("a-b", "dash\n"),
    ("a0", "zero\n"),
    ("ab/x", "x\n"),
    ("README.md", "# fixture\n"),
];

#[test]
fn import_tree_matches_git_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 3, 5, TRICKY);
    std::fs::write(src.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    git(&src, &["update-index", "--add", "--chmod=+x", "run.sh"]);
    std::os::unix::fs::symlink("a.b", src.join("link")).unwrap();
    git(&src, &["add", "link"]);
    git(&src, &["commit", "-q", "-m", "exec and link"]);

    let data = tmp.path().join("data");
    let root = Repo::init(&data, &src, "main", None).unwrap();
    let repo = Repo::open(&data).unwrap();
    let ours = repo
        .with(|r| {
            let m = r.manifest(&root)?;
            git::write_tree(&mut r.odb, &m)
        })
        .unwrap();
    let theirs = git(&src, &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(ours, theirs.trim());
}

#[test]
fn landed_changes_become_valid_git_history() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 3, 5, TRICKY);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));

    let ch =
        call(&url, "open_change", json!({"agent": "agent-a", "intent": "reshape the tree", "model": "test-model"}));
    let id = ch["id"].as_str().unwrap();
    let applied = call(
        &url,
        "apply",
        json!({"change": id, "message": "Reshape the tree\n\nBody line.", "edits": [
            {"path": "z/y/x.txt", "content": "new deep file\n"},
            {"path": "a-b", "delete": true},
            {"path": "a.b", "executable": true},
            {"path": "a/b.txt", "replace": {"old": "nested", "new": "NESTED"}},
            {"path": "bin.dat", "content_b64": "AAEC/w=="},
        ]}),
    );
    assert_eq!(applied["paths"].as_array().unwrap().len(), 5);
    let err =
        treeyard::client::call(&url, "apply", json!({"change": id, "edits": [{"path": "a.b/x", "content": "no"}]}));
    assert!(err.unwrap_err().to_string().contains("is a file"));

    let landed = call(&url, "land", json!({"change": id}));
    assert_eq!(landed["status"], "landed");
    let commit = landed["commit"].as_str().unwrap();

    let gd = data.join("git");
    let gd_s = gd.to_str().unwrap();
    let (ok, _, err) = git_status(tmp.path(), &["--git-dir", gd_s, "fsck", "--strict", "--no-progress"]);
    assert!(ok, "fsck: {err}");
    assert_eq!(git(tmp.path(), &["--git-dir", gd_s, "rev-parse", "main"]).trim(), commit);
    let parent = git(tmp.path(), &["--git-dir", gd_s, "rev-parse", "main^"]);
    assert_eq!(parent.trim(), git(&src, &["rev-parse", "HEAD"]).trim(), "history continues the imported commit");

    let msg = git(tmp.path(), &["--git-dir", gd_s, "log", "-1", "--format=%an <%ae>%n%B", "main"]);
    assert!(msg.starts_with("agent-a <agent-a@agents.treeyard>\nReshape the tree\n\nBody line."), "{msg}");
    assert!(msg.contains(&format!("TreeYard-Change: {id}")) && msg.contains("TreeYard-Model: test-model"));

    let clone = tmp.path().join("clone");
    git(tmp.path(), &["clone", "-q", gd_s, clone.to_str().unwrap()]);
    assert_eq!(std::fs::read_to_string(clone.join("z/y/x.txt")).unwrap(), "new deep file\n");
    assert_eq!(std::fs::read_to_string(clone.join("a/b.txt")).unwrap(), "NESTED\n");
    assert_eq!(std::fs::read(clone.join("bin.dat")).unwrap(), vec![0, 1, 2, 255]);
    assert!(!clone.join("a-b").exists());
    let ls = git(&clone, &["ls-files", "-s", "a.b"]);
    assert!(ls.starts_with("100755"), "{ls}");

    // Read side: log, grep, diff, read at an old revision, revert.
    let log = call(&url, "log", json!({"path": "a"}));
    assert_eq!(log["entries"][0]["change"], id);
    let grep = call(&url, "grep", json!({"pattern": "^NEST"}));
    assert_eq!(grep["matches"][0]["path"], "a/b.txt");
    let diff = call(&url, "diff", json!({"change": id}));
    assert_eq!(diff["files"].as_array().unwrap().len(), 5);
    let old = call(&url, "read", json!({"path": "a-b", "at": git(&src, &["rev-parse", "HEAD"]).trim()}));
    assert_eq!(old["content"], "dash\n");
    let reverted = call(&url, "revert", json!({"changeset": landed["changeset"], "agent": "agent-b"}));
    assert_eq!(reverted["status"], "landed");
    let tree_now = git(tmp.path(), &["--git-dir", gd_s, "rev-parse", "main^{tree}"]);
    assert_eq!(tree_now, git(&src, &["rev-parse", "HEAD^{tree}"]), "revert restores the imported tree");
}

#[test]
fn mcp_bridge_lists_and_calls_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 3, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));

    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_treeyard"))
        .args(["mcp", "--url", &url, "--agent", "mcp-agent"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let msgs = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "open_change", "arguments": {"intent": "try mcp"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "read", "arguments": {"path": "missing.txt"}}}),
    ];
    {
        let mut stdin = child.stdin.take().unwrap();
        for m in &msgs {
            writeln!(stdin, "{m}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let replies: Vec<serde_json::Value> =
        String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 4, "the notification gets no reply");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "treeyard");
    assert!(replies[1]["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "land"));
    let opened: serde_json::Value =
        serde_json::from_str(replies[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(opened["agent"], "mcp-agent");
    assert_eq!(replies[3]["result"]["isError"], true);
}

#[test]
fn commit_message_survives_rebase_and_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 3, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let gd = data.join("git");
    let gd_s = gd.to_str().unwrap();
    let subject = |rev: &str| git(tmp.path(), &["--git-dir", gd_s, "log", "-1", "--format=%s", rev]).trim().to_string();

    let a = call(&url, "open_change", json!({"agent": "a", "intent": "intent a"}))["id"].as_str().unwrap().to_string();
    let b = call(&url, "open_change", json!({"agent": "b", "intent": "intent b"}))["id"].as_str().unwrap().to_string();
    let c = call(&url, "open_change", json!({"agent": "c", "intent": "intent c"}))["id"].as_str().unwrap().to_string();
    call(
        &url,
        "apply",
        json!({"change": a, "message": "Message A", "edits": [{"path": "f00.txt", "replace": {"old": "line 0", "new": "A0"}}]}),
    );
    call(
        &url,
        "apply",
        json!({"change": b, "message": "Message B", "edits": [{"path": "f00.txt", "replace": {"old": "line 2", "new": "B2"}}]}),
    );
    call(
        &url,
        "apply",
        json!({"change": c, "message": "Message C", "edits": [{"path": "f00.txt", "replace": {"old": "line 0", "new": "C0"}}]}),
    );
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    // b is rebased on the server: its message must reach the commit.
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    assert_eq!(subject("main"), "Message B");
    // c conflicts with a; settling it with resolve keeps the message too.
    assert_eq!(call(&url, "land", json!({"change": c}))["status"], "conflict");
    call(&url, "resolve", json!({"change": c, "path": "f00.txt", "take": "ours"}));
    assert_eq!(call(&url, "land", json!({"change": c}))["status"], "landed");
    assert_eq!(subject("main"), "Message C");
}
