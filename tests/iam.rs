//! Access control and audit over HTTP, git smart HTTP and MCP: API tokens per agent with
//! read and write path globs, an admin token, ownership of changes, and the audit log.

mod common;

use common::*;
use rootstock::client::call_with;
use rootstock::repo::Repo;
use serde_json::{Value, json};
use std::sync::Arc;

struct Setup {
    _tmp: tempfile::TempDir,
    url: String,
    admin: String,
    data: std::path::PathBuf,
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(
        tmp.path(),
        1,
        3,
        &[
            ("src/a.rs", "pub fn a() -> i32 {\n    1\n}\n"),
            ("docs/guide.md", "# Guide\nsee src/a.rs\n"),
            ("secret/plan.txt", "launch plan: key word sesame\n"),
        ],
    );
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let repo = Arc::new(Repo::open(&data).unwrap());
    // `rootstock token create --admin` does the same on the data directory.
    let (_, admin) = repo.auth.create("root", true, &["**".into()], &["**".into()]).unwrap();
    let url = spawn_server(repo);
    Setup { _tmp: tmp, url, admin, data }
}

fn ok(url: &str, token: &str, op: &str, p: Value) -> Value {
    call_with(url, Some(token), op, p).unwrap_or_else(|e| panic!("{op} failed: {e:#}"))
}

fn err(url: &str, token: Option<&str>, op: &str, p: Value) -> String {
    match call_with(url, token, op, p) {
        Ok(v) => panic!("{op} should fail, got {v}"),
        Err(e) => format!("{e:#}"),
    }
}

fn token(url: &str, admin: &str, name: &str, read: &[&str], write: &[&str]) -> String {
    let v = ok(url, admin, "token_create", json!({"name": name, "read": read, "write": write}));
    v["token"].as_str().unwrap().to_string()
}

#[test]
fn tokens_scope_reads_and_writes_and_every_write_is_audited() {
    let s = setup();
    let url = &s.url;
    // Auth is on: no token or a bad token is 401.
    assert!(err(url, None, "status", json!({})).contains("HTTP 401"));
    assert!(err(url, Some("rst_000000000000_0000"), "status", json!({})).contains("HTTP 401"));
    let agent = token(url, &s.admin, "agent-1", &["**"], &["src/**"]);
    let other = token(url, &s.admin, "agent-2", &["**"], &["src/**", "docs/**"]);
    let reader = token(url, &s.admin, "reader", &["src/**", "docs/**"], &[]);
    assert_eq!(ok(url, &agent, "whoami", json!({}))["name"], "agent-1");
    // Only admin tokens manage tokens.
    assert!(err(url, Some(&agent), "token_create", json!({"name": "x"})).contains("HTTP 403"));

    // The reader sees only its globs: read, list, tree, grep, blobs.
    assert!(err(url, Some(&reader), "read", json!({"path": "secret/plan.txt"})).contains("HTTP 403"));
    assert_eq!(ok(url, &reader, "read", json!({"path": "src/a.rs"}))["content"], "pub fn a() -> i32 {\n    1\n}\n");
    let names: Vec<String> = ok(url, &reader, "list", json!({}))["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["docs", "src"]);
    let tree = ok(url, &reader, "tree", json!({}));
    assert_eq!(tree["files"], 2);
    let hits = ok(url, &reader, "grep", json!({"pattern": "sesame|Guide"}));
    assert_eq!(hits["matches"].as_array().unwrap().len(), 1, "{hits}");
    let all = ok(url, &s.admin, "tree", json!({}));
    let secret_blob = all["entries"].as_array().unwrap().iter().find(|e| e[0] == "secret/plan.txt").unwrap()[2].clone();
    assert!(err(url, Some(&reader), "blobs", json!({"ids": [secret_blob]})).contains("HTTP 403"));
    // A read-only token cannot open a change.
    assert!(err(url, Some(&reader), "open_change", json!({"intent": "try"})).contains("read-only"));

    // agent-1 acts as itself and writes only src/**.
    assert!(
        err(url, Some(&agent), "open_change", json!({"agent": "agent-9", "intent": "spoof"}))
            .contains("may not act as agent agent-9")
    );
    let ch = ok(url, &agent, "open_change", json!({"intent": "bump a", "model": "sim-model", "session": "sess-7"}));
    assert_eq!(ch["agent"], "agent-1");
    let id = ch["id"].as_str().unwrap().to_string();
    let denied =
        err(url, Some(&agent), "apply", json!({"change": id, "edits": [{"path": "README.md", "content": "hacked\n"}]}));
    assert!(denied.contains("HTTP 403") && denied.contains("README.md"), "{denied}");
    ok(
        url,
        &agent,
        "apply",
        json!({"change": id, "edits": [{"path": "src/a.rs", "replace": {"old": "1\n", "new": "2\n"}}]}),
    );
    // agent-2 may write src/** too, but not agent-1's change.
    assert!(err(url, Some(&other), "land", json!({"change": id})).contains("belongs to agent-1"));
    let landed = ok(url, &agent, "land", json!({"change": id}));
    assert_eq!(landed["status"], "landed");

    // Revoked tokens stop working at once.
    let tokens = ok(url, &s.admin, "token_list", json!({}));
    let id2 = tokens["tokens"].as_array().unwrap().iter().find(|t| t["name"] == "agent-2").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!tokens.to_string().contains(&other), "token_list never shows a secret");
    ok(url, &s.admin, "token_revoke", json!({"id": id2}));
    assert!(err(url, Some(&other), "status", json!({})).contains("revoked"));

    // The audit log: who, model, session, intent, paths, changeset, commit, and the denials.
    assert!(err(url, Some(&reader), "audit", json!({})).contains("HTTP 403"));
    let a = ok(url, &s.admin, "audit", json!({"agent": "agent-1"}));
    let rows = a["entries"].as_array().unwrap();
    let land = rows.iter().find(|r| r["op"] == "land").expect("land row");
    assert_eq!(land["actor"], "agent-1");
    assert_eq!(
        (land["model"].as_str(), land["session"].as_str(), land["intent"].as_str()),
        (Some("sim-model"), Some("sess-7"), Some("bump a"))
    );
    assert_eq!(land["paths"], json!(["src/a.rs"]));
    assert_eq!(land["status"], "landed");
    assert_eq!(land["changeset"], landed["changeset"]);
    assert_eq!(land["commit"], landed["commit"]);
    let deny = rows.iter().find(|r| r["op"] == "apply" && r["status"] == "denied").expect("denied apply row");
    assert_eq!(deny["paths"], json!(["README.md"]));
    let by_path = ok(url, &s.admin, "audit", json!({"path": "src", "status": "landed"}));
    assert_eq!(by_path["entries"].as_array().unwrap().len(), 1);
    let denials = ok(url, &s.admin, "audit", json!({"status": "denied"}));
    let ops: Vec<(&str, &str)> = denials["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["actor"].as_str().unwrap(), r["op"].as_str().unwrap()))
        .collect();
    for want in [
        ("reader", "read"),
        ("reader", "open_change"),
        ("agent-1", "open_change"),
        ("agent-1", "apply"),
        ("agent-2", "land"),
        ("agent-1", "token_create"),
        ("reader", "blobs"),
    ] {
        assert!(ops.contains(&want), "{want:?} missing from denials {ops:?}");
    }
    let created = ok(url, &s.admin, "audit", json!({"op": "token_create"}));
    assert!(!created.to_string().contains(&agent), "the audit log never holds a secret");
    assert!(s.data.join("rootstock.db").exists());
}

#[test]
fn git_over_http_needs_a_token_and_push_obeys_write_globs() {
    let s = setup();
    let url = &s.url;
    let agent = token(url, &s.admin, "agent-1", &["**"], &["src/**"]);
    let reader = token(url, &s.admin, "reader", &["src/**"], &[]);
    let tmp = tempfile::tempdir().unwrap();
    let host = url.trim_start_matches("http://");
    let (cloned, _, e) = git_status(tmp.path(), &["clone", "-q", &format!("{url}/git"), "anon"]);
    assert!(!cloned && (e.contains("401") || e.contains("Authentication") || e.contains("could not read")), "{e}");
    let (cloned, _, e) = git_status(tmp.path(), &["clone", "-q", &format!("http://x:{reader}@{host}/git"), "partial"]);
    assert!(!cloned && e.contains("403"), "a partial reader may not clone: {e}");
    let with = format!("http://x:{agent}@{host}/git");
    git(tmp.path(), &["clone", "-q", &with, "w"]);
    let w = tmp.path().join("w");
    std::fs::write(w.join("f00.txt"), "changed by a person\n").unwrap();
    git(&w, &["commit", "-qam", "edit f00"]);
    let (pushed, _, e) = git_status(&w, &["push", "-q", "origin", "main"]);
    assert!(!pushed && e.contains("may not write f00.txt"), "{e}");
    git(&w, &["reset", "-q", "--hard", "origin/main"]);
    std::fs::write(w.join("src/a.rs"), "pub fn a() -> i32 {\n    3\n}\n").unwrap();
    git(&w, &["commit", "-qam", "edit a"]);
    git(&w, &["push", "-q", "origin", "main"]);
    let rows = ok(url, &s.admin, "audit", json!({"op": "push"}));
    let st: Vec<&str> = rows["entries"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(st, vec!["landed", "denied"]);
    assert_eq!(rows["entries"][0]["paths"], json!(["src/a.rs"]));
    assert_eq!(rows["entries"][0]["actor"], "agent-1");
}

#[test]
fn mcp_bridge_sends_the_token_from_the_environment() {
    let s = setup();
    let agent = token(&s.url, &s.admin, "agent-mcp", &["**"], &["src/**"]);
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_rootstock"))
        .args(["mcp", "--url", &s.url])
        .env("ROOTSTOCK_TOKEN", &agent)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for m in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "whoami", "arguments": {}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "blame", "arguments": {"path": "src/a.rs"}}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "token_list", "arguments": {}}}),
        ] {
            writeln!(stdin, "{m}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let replies: Vec<Value> =
        String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let text = |i: usize| replies[i]["result"]["content"][0]["text"].as_str().unwrap().to_string();
    let who: Value = serde_json::from_str(&text(0)).unwrap();
    assert_eq!(who["name"], "agent-mcp");
    let blame: Value = serde_json::from_str(&text(1)).unwrap();
    assert_eq!(blame["lines"], 3);
    assert_eq!(replies[2]["result"]["isError"], true);
    assert!(text(2).contains("403"));
}
