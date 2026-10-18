//! Checkpoints inside an open change: `checkpoint`, `checkpoints`, `restore_change` (undo that
//! keeps history), restore after a rebase, and `treeyard workspace restore` on a node.

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use treeyard::repo::Repo;
use treeyard::workspace::{CreateOptions, Node};

fn setup(root: &Path) -> String {
    let src = fixture_repo(root, 3, 5, &[]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

fn write(url: &str, ch: &str, path: &str, content: &str) -> Value {
    call(url, "apply", json!({"change": ch, "edits": [{"path": path, "content": content}]}))
}

fn read(url: &str, path: &str, at: &str) -> Option<String> {
    treeyard::client::call(url, "read", json!({"path": path, "at": at}))
        .ok()
        .map(|v| v["content"].as_str().unwrap().to_string())
}

#[test]
fn checkpoints_mark_turns_and_restore_keeps_history() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path());
    let ch = call(&url, "open_change", json!({"agent": "a", "intent": "turns"}))["id"].as_str().unwrap().to_string();

    write(&url, &ch, "f00.txt", "turn 1\n");
    let c1 = call(&url, "checkpoint", json!({"change": ch, "label": "turn 1"}));
    assert_eq!(c1["checkpoint"]["n"], 1, "{c1}");
    assert!(c1["checkpoint"]["id"].as_str().unwrap().starts_with("cp_"));
    let again = call(&url, "checkpoint", json!({"change": ch}));
    assert_eq!(again["repeated"], true, "the same head is not marked twice: {again}");
    assert_eq!(again["checkpoint"]["n"], 1);

    write(&url, &ch, "f01.txt", "turn 2\n");
    write(&url, &ch, "new.txt", "added in turn 2\n");
    call(&url, "checkpoint", json!({"change": ch, "label": "turn 2"}));
    write(&url, &ch, "f00.txt", "turn 3 broke it\n");

    let list = call(&url, "checkpoints", json!({"change": ch}));
    let cps = list["checkpoints"].as_array().unwrap();
    assert_eq!(cps.len(), 2, "{list}");
    assert_eq!(cps[0]["paths"], json!(["f00.txt"]));
    assert_eq!(cps[1]["paths"], json!(["f01.txt", "new.txt"]), "paths since the previous checkpoint");
    assert_eq!(cps[1]["label"], "turn 2");
    assert!(cps[1]["time"].as_i64().unwrap() > 0);

    // Restore to checkpoint 1: f01 and new.txt go back to main, f00 to turn 1.
    let r = call(&url, "restore_change", json!({"change": ch, "to": 1}));
    let head = r["change"]["head"].as_str().unwrap().to_string();
    assert_eq!(read(&url, "f00.txt", &head).unwrap(), "turn 1\n");
    assert_eq!(read(&url, "f01.txt", &head), read(&url, "f01.txt", "main"));
    assert_eq!(read(&url, "new.txt", &head), None);
    assert_eq!(r["checkpoint"], 1);
    let before = call(&url, "change", json!({"change": ch}));
    assert!(before["revisions"].as_u64().unwrap() >= 5, "the restore is a new revision, earlier heads stay: {before}");
    let undo = r["undo"].as_i64().unwrap();
    assert_eq!(undo, 3, "the head before the restore became checkpoint 3: {r}");

    // The restore is undone by restoring to that checkpoint (number as a string works too).
    let u = call(&url, "restore_change", json!({"change": ch, "to": undo.to_string()}));
    let head = u["change"]["head"].as_str().unwrap().to_string();
    assert_eq!(read(&url, "f00.txt", &head).unwrap(), "turn 3 broke it\n");
    assert_eq!(read(&url, "new.txt", &head).unwrap(), "added in turn 2\n");

    // By checkpoint id and by changeset id; 0 is the base (no edits).
    let by_id = call(&url, "restore_change", json!({"change": ch, "to": cps[1]["id"]}));
    assert_eq!(read(&url, "f00.txt", by_id["change"]["head"].as_str().unwrap()).unwrap(), "turn 1\n");
    let by_cs = call(&url, "restore_change", json!({"change": ch, "to": cps[0]["changeset"]}));
    assert_eq!(by_cs["checkpoint"], 1);
    let empty = call(&url, "restore_change", json!({"change": ch, "to": 0}));
    assert_eq!(empty["change"]["paths"], json!([]), "{empty}");
    assert_eq!(empty["change"]["head"], empty["change"]["base"]);

    // A changeset of another change, or an unknown number, is refused.
    let other = call(&url, "open_change", json!({"agent": "b", "intent": "other"}))["id"].as_str().unwrap().to_string();
    let oh = write(&url, &other, "f02.txt", "b\n")["head"].as_str().unwrap().to_string();
    let e = treeyard::client::call(&url, "restore_change", json!({"change": ch, "to": oh})).unwrap_err();
    assert!(format!("{e:#}").contains("not a changeset of change"), "{e:#}");
    let e = treeyard::client::call(&url, "restore_change", json!({"change": ch, "to": 99})).unwrap_err();
    assert!(format!("{e:#}").contains("no checkpoint 99"), "{e:#}");

    // The restored change lands like any other.
    call(&url, "restore_change", json!({"change": ch, "to": 2}));
    assert_eq!(call(&url, "land", json!({"change": ch}))["status"], "landed");
    assert_eq!(read(&url, "new.txt", "main").unwrap(), "added in turn 2\n");
}

#[test]
fn restore_after_a_rebase_keeps_mains_work() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path());
    let ch = call(&url, "open_change", json!({"agent": "a", "intent": "mine"}))["id"].as_str().unwrap().to_string();
    let f00 = read(&url, "f00.txt", "main").unwrap();
    write(&url, &ch, "f00.txt", &f00.replace("file 0 line 0", "mine line 0"));
    call(&url, "checkpoint", json!({"change": ch}));
    write(&url, &ch, "f00.txt", "rewritten\n");

    // Another agent lands an edit to the end of f00 and to f01; the change rebases onto it.
    let b = call(&url, "open_change", json!({"agent": "b", "intent": "theirs"}))["id"].as_str().unwrap().to_string();
    write(&url, &b, "f00.txt", &f00.replace("file 0 line 4", "main line 4"));
    write(&url, &b, "f01.txt", "main's f01\n");
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    let rb = call(&url, "rebase", json!({"change": ch}));
    assert_eq!(rb["conflicts"].as_array().unwrap().len(), 1, "rewritten f00 conflicts with main: {rb}");
    call(&url, "resolve", json!({"change": ch, "path": "f00.txt", "take": "ours"}));

    let r = call(&url, "restore_change", json!({"change": ch, "to": 1}));
    assert_eq!(r["rebased_since"], true, "{r}");
    let head = r["change"]["head"].as_str().unwrap();
    let got = read(&url, "f00.txt", head).unwrap();
    assert!(got.contains("mine line 0") && got.contains("main line 4"), "checkpoint edits merged onto main: {got}");
    assert_eq!(read(&url, "f01.txt", head).unwrap(), "main's f01\n", "main's other work stays");
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

#[test]
fn workspace_restore_rewrites_only_differing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path());
    let node = Node::new(tmp.path().join("node"), url.clone());
    let opts = CreateOptions {
        name: "w1".into(),
        agent: "a".into(),
        intent: "restore".into(),
        base: None,
        strategy: treeyard::cow::Strategy::Copy,
        layers: Some(vec![]),
        setup: None,
        git: false,
        ..Default::default()
    };
    let (ws, _) = node.create(&opts).unwrap();
    std::fs::write(ws.dir.join("f00.txt"), "turn 1\n").unwrap();
    node.sync("w1").unwrap();
    call(&url, "checkpoint", json!({"change": ws.change}));

    std::fs::write(ws.dir.join("f00.txt"), "turn 2\n").unwrap();
    std::fs::create_dir_all(ws.dir.join("sub")).unwrap();
    std::fs::write(ws.dir.join("sub/new.txt"), "new\n").unwrap();
    std::fs::write(ws.dir.join("f01.txt"), "unsynced edit\n").unwrap();
    let kept = mtime(&ws.dir.join("f02.txt"));

    let r = node.restore("w1", "1").unwrap();
    assert_eq!(std::fs::read_to_string(ws.dir.join("f00.txt")).unwrap(), "turn 1\n", "{r}");
    assert!(!ws.dir.join("sub").exists(), "a file added after the checkpoint is gone, with its directory");
    assert_eq!(read(&url, "f01.txt", "main"), Some(std::fs::read_to_string(ws.dir.join("f01.txt")).unwrap()));
    assert_eq!(mtime(&ws.dir.join("f02.txt")), kept, "untouched files keep their mtime");
    assert_eq!(r["written"], json!(["f00.txt", "f01.txt"]), "{r}");
    assert_eq!(r["deleted"], json!(["sub/new.txt"]));

    // The edits made before the restore were synced first: checkpoint 2 holds them.
    let s = node.sync("w1").unwrap();
    assert_eq!(s["changesets"], 0, "the sync state matches the restored files: {s}");
    let r = node.restore("w1", &r["restore"]["undo"].to_string()).unwrap();
    assert_eq!(std::fs::read_to_string(ws.dir.join("f01.txt")).unwrap(), "unsynced edit\n", "{r}");
    assert_eq!(std::fs::read_to_string(ws.dir.join("sub/new.txt")).unwrap(), "new\n");
}
