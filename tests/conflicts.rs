//! Conflicts that do not block (`[coordination] keep_conflicted`): a conflicted change keeps
//! rebasing with continuous rebase, the agent keeps editing, `land` waits until it is resolved.
//! The default keeps the old rule: a conflicted change stays where it is until resolved.

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use treeyard::repo::Repo;

fn setup(root: &Path, config: &str) -> String {
    let src = fixture_repo(root, 3, 30, &[]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    std::fs::write(data.join("treeyard.toml"), config).unwrap();
    spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

fn open(url: &str, agent: &str) -> String {
    call(url, "open_change", json!({"agent": agent, "intent": format!("work of {agent}")}))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn read(url: &str, path: &str, at: &str) -> String {
    call(url, "read", json!({"path": path, "at": at}))["content"].as_str().unwrap().to_string()
}

fn write(url: &str, ch: &str, path: &str, content: &str) -> Value {
    call(url, "apply", json!({"change": ch, "edits": [{"path": path, "content": content}]}))
}

fn set_line(text: &str, j: usize, new: &str) -> String {
    text.lines().enumerate().map(|(i, l)| if i == j { format!("{new}\n") } else { format!("{l}\n") }).collect()
}

/// a and b edit line 3 of f00; b lands, so continuous rebase gives a a conflict. a then edits
/// f01, and c lands an edit to line 20 of f00 and to f02. Returns (url, a).
fn conflicted(root: &Path, config: &str) -> (String, String) {
    let url = setup(root, config);
    let f00 = read(&url, "f00.txt", "main");
    let a = open(&url, "a");
    write(&url, &a, "f00.txt", &set_line(&f00, 3, "a's line 3"));
    let b = open(&url, "b");
    write(&url, &b, "f00.txt", &set_line(&f00, 3, "b's line 3"));
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    let ch = call(&url, "change", json!({"change": a}));
    assert_eq!(ch["conflicts"][0]["path"], "f00.txt", "{ch}");
    assert_eq!(ch["conflicted"], true);

    call(
        &url,
        "apply",
        json!({"change": a, "edits": [{"path": "f01.txt", "replace": {"old": "file 1 line 0", "new": "a's f01"}}]}),
    );
    let c = open(&url, "c");
    let main_f00 = read(&url, "f00.txt", "main");
    write(&url, &c, "f00.txt", &set_line(&main_f00, 20, "c's line 20"));
    write(&url, &c, "f02.txt", "c's f02\n");
    assert_eq!(call(&url, "land", json!({"change": c}))["status"], "landed");
    (url, a)
}

#[test]
fn by_default_a_conflicted_change_stays_until_resolved() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, a) = conflicted(tmp.path(), "[coordination]\ncontinuous_rebase = true\n");
    let ch = call(&url, "change", json!({"change": a}));
    assert_ne!(ch["base"], call(&url, "status", json!({}))["main"], "continuous rebase skipped it: {ch}");
    let e = treeyard::client::call(&url, "rebase", json!({"change": a})).unwrap_err();
    assert!(format!("{e:#}").contains("unresolved conflicts"), "{e:#}");
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "conflict");
}

#[test]
fn kept_conflicts_rebase_and_land_waits_until_resolved() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, a) = conflicted(tmp.path(), "[coordination]\ncontinuous_rebase = true\nkeep_conflicted = true\n");
    let main = call(&url, "status", json!({}))["main"].as_str().unwrap().to_string();
    let ch = call(&url, "change", json!({"change": a}));
    assert_eq!(ch["base"], main, "the conflicted change moved with main: {ch}");
    assert_eq!(ch["conflicted"], true);
    let c = &ch["conflicts"][0];
    assert_eq!(c["path"], "f00.txt");
    assert!(c["text"]["ours"].as_str().unwrap().contains("a's line 3"), "ours is a's side, not markers: {c}");
    assert!(c["text"]["theirs"].as_str().unwrap().contains("c's line 20"), "theirs is the new main: {c}");
    assert!(!c["text"]["base"].as_str().unwrap().contains("b's line 3"), "base is the conflict's own base");
    assert_eq!(c["theirs_by"].as_array().unwrap().len(), 2, "b and c touched the path: {c}");

    let head = ch["head"].as_str().unwrap();
    let text = read(&url, "f00.txt", head);
    assert!(text.contains("<<<<<<<") && text.contains("c's line 20"), "markers plus main's later edit: {text}");
    assert!(read(&url, "f01.txt", head).contains("a's f01"), "edits on top of the conflict stay");
    assert_eq!(read(&url, "f02.txt", head), "c's f02\n");

    // land refuses while the conflict is open, rebase is allowed.
    let land = call(&url, "land", json!({"change": a}));
    assert_eq!(land["status"], "conflict", "{land}");
    call(&url, "rebase", json!({"change": a}));

    // Writing the path resolves it (no merge with the stale view), then it lands.
    let fixed = set_line(&read(&url, "f00.txt", "main"), 3, "a and b's line 3");
    let after = write(&url, &a, "f00.txt", &fixed);
    assert_eq!(after["conflicted"], false, "{after}");
    assert_eq!(read(&url, "f00.txt", after["head"].as_str().unwrap()), fixed);
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    let f00 = read(&url, "f00.txt", "main");
    assert!(f00.contains("a and b's line 3") && f00.contains("c's line 20"), "{f00}");
    assert!(read(&url, "f01.txt", "main").contains("a's f01"));
}

#[test]
fn a_kept_conflict_that_main_resolves_clears_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, a) = conflicted(tmp.path(), "[coordination]\ncontinuous_rebase = true\nkeep_conflicted = true\n");
    // d sets line 3 to exactly a's version: the next rebase merges cleanly.
    let d = open(&url, "d");
    write(&url, &d, "f00.txt", &set_line(&read(&url, "f00.txt", "main"), 3, "a's line 3"));
    assert_eq!(call(&url, "land", json!({"change": d}))["status"], "landed");
    let ch = call(&url, "change", json!({"change": a}));
    assert_eq!(ch["conflicts"], json!([]), "{ch}");
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
}
