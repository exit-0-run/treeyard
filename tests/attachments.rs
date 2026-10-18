//! Attachments of a change (`attach`, `attachments`): redacted on the way in, kept in SQLite
//! while the change is open, and a git note under refs/notes/treeyard on the landed commit
//! (direct and queue landing), readable through a clone over HTTP.

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use treeyard::repo::Repo;

fn setup(root: &Path, config: &str) -> (Arc<Repo>, String) {
    let src = fixture_repo(root, 3, 5, &[]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    std::fs::write(data.join("treeyard.toml"), config).unwrap();
    let repo = Arc::new(Repo::open(&data).unwrap());
    let url = spawn_server(repo.clone());
    (repo, url)
}

fn change(url: &str, agent: &str, path: &str) -> String {
    let ch = call(url, "open_change", json!({"agent": agent, "intent": format!("edit {path}")}))["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(url, "apply", json!({"change": ch, "edits": [{"path": path, "content": format!("{agent}\n")}]}));
    ch
}

fn err(url: &str, op: &str, p: Value) -> String {
    format!("{:#}", treeyard::client::call(url, op, p).unwrap_err())
}

#[test]
fn attachments_are_redacted_and_land_as_a_git_note() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, url) = setup(tmp.path(), "");
    let ch = change(&url, "a", "f00.txt");
    let key = format!("sk-{}", "k".repeat(30));
    let transcript = format!("user: fix the parser\nassistant: set OPENAI_API_KEY={key} first\nassistant: done\n");
    let r = call(&url, "attach", json!({"change": ch, "kind": "transcript", "text": transcript}));
    assert_eq!(r["dropped_lines"], 1, "{r}");
    call(&url, "attach", json!({"change": ch, "kind": "summary", "text": "Why: the parser lost a trailing comma."}));

    let list = call(&url, "attachments", json!({"change": ch}));
    let rows = list["attachments"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["text"], "user: fix the parser\nassistant: done\n");
    assert_eq!(list["note"], Value::Null, "no git note before land");
    assert!(!rows[0]["text"].as_str().unwrap().contains(&key));

    assert!(err(&url, "attach", json!({"change": ch, "kind": "diary", "text": "x"})).contains("kind must be"));
    let big = "x".repeat(256 * 1024 + 1);
    assert!(err(&url, "attach", json!({"change": ch, "kind": "note", "text": big})).contains("up to 262144 bytes"));

    // Nothing reaches git objects before land.
    let gd = repo.git_dir().to_path_buf();
    assert!(!git_status(&gd, &["rev-parse", "--verify", "refs/notes/treeyard"]).0);

    let land = call(&url, "land", json!({"change": ch}));
    assert_eq!(land["status"], "landed");
    let commit = land["commit"].as_str().unwrap();
    let note = git(&gd, &["notes", "--ref=treeyard", "show", commit]);
    assert!(note.starts_with(&format!("TreeYard attachments of change {ch} (2)\n")), "{note}");
    assert!(note.contains("## transcript 1 (agent a, secret lines dropped: 1)"), "{note}");
    assert!(note.contains("Why: the parser lost a trailing comma.") && !note.contains(&key));
    assert_eq!(land["notes"]["commit"], git(&gd, &["rev-parse", "refs/notes/treeyard"]).trim());

    // Deterministic: the notes commit takes its dates from the landed changeset.
    let dates = |c: &str| git(&gd, &["show", "-s", "--format=%at %ct", c]);
    assert_eq!(dates("refs/notes/treeyard"), dates(commit));
    let listed = call(&url, "attachments", json!({"change": ch}));
    assert_eq!(listed["note"]["commit"], commit);
    assert!(err(&url, "attach", json!({"change": ch, "kind": "note", "text": "late"})).contains("attach before land"));

    // A second land adds to the notes history; the first note stays.
    let b = change(&url, "b", "f01.txt");
    call(&url, "attach", json!({"change": b, "kind": "note", "text": "b's note"}));
    let land_b = call(&url, "land", json!({"change": b}));
    assert_eq!(git(&gd, &["rev-list", "--count", "refs/notes/treeyard"]).trim(), "2");
    assert!(git(&gd, &["notes", "--ref=treeyard", "show", commit]).contains("trailing comma"));

    // A change without attachments gets no note.
    let c = change(&url, "c", "f02.txt");
    assert_eq!(call(&url, "land", json!({"change": c}))["notes"], Value::Null);

    // A clone over HTTP fetches the notes ref and reads them with git notes.
    let clone = tmp.path().join("clone");
    git(tmp.path(), &["clone", "-q", &format!("{url}/git"), clone.to_str().unwrap()]);
    git(&clone, &["fetch", "-q", "origin", "refs/notes/treeyard:refs/notes/treeyard"]);
    let shown = git(&clone, &["notes", "--ref=treeyard", "show", land_b["commit"].as_str().unwrap()]);
    assert!(shown.contains("b's note"), "{shown}");
}

#[test]
fn queue_landing_writes_notes_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, url) = setup(tmp.path(), "[landing]\nmode = \"queue\"\ntest = \"true\"\n");
    let ch = change(&url, "q", "f00.txt");
    call(&url, "attach", json!({"change": ch, "kind": "summary", "text": "queued work"}));
    assert_eq!(call(&url, "land", json!({"change": ch}))["status"], "queued");
    let step = call(&url, "queue_run", json!({}));
    assert_eq!(step["landed"][0]["change"], ch.as_str(), "{step}");
    let commit = call(&url, "attachments", json!({"change": ch}))["note"]["commit"].as_str().unwrap().to_string();
    let note = git(repo.git_dir(), &["notes", "--ref=treeyard", "show", &commit]);
    assert!(note.contains("queued work"), "{note}");
}
