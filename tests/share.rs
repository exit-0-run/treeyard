//! Hunk ownership in a directory that several agents share (src/hunks.rs, src/share.rs):
//! `assign`, `claim`, `apply_assigned` and `treeyard share`.

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use treeyard::repo::Repo;

const LIB: &str = "use std::fmt;\n\npub fn parse(s: &str) -> u32 {\n    s.len() as u32\n}\n\npub fn render(n: u32) -> String {\n    format!(\"{n}\")\n}\n";
const PARSE: (&str, &str) = ("s.len() as u32", "s.trim().len() as u32");
const RENDER: (&str, &str) = ("format!(\"{n}\")", "format!(\"<{n}>\")");

/// A served repo whose only file is src/lib.rs; returns the source checkout too.
fn setup(root: &Path) -> (String, PathBuf) {
    let src = fixture_repo(root, 0, 0, &[("src/lib.rs", LIB)]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    (spawn_server(Arc::new(Repo::open(&data).unwrap())), src)
}

fn open(url: &str, agent: &str, extra: Value) -> String {
    let mut p = json!({"agent": agent, "intent": format!("work of {agent}")});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    call(url, "open_change", p)["id"].as_str().unwrap().to_string()
}

fn edit(edits: &[(&str, &str)]) -> String {
    edits.iter().fold(LIB.to_string(), |t, (old, new)| t.replace(old, new))
}

fn assign(url: &str, changes: &[&str], text: &str) -> Value {
    call(url, "assign", json!({"share": "dir", "changes": changes, "files": [{"path": "src/lib.rs", "content": text}]}))
}

/// The hunks the reply gives `change`.
fn hunks_of<'a>(v: &'a Value, change: &str) -> &'a Vec<Value> {
    v["changes"].as_array().unwrap().iter().find(|c| c["change"] == change).unwrap()["hunks"].as_array().unwrap()
}

fn read(url: &str, at: &str) -> String {
    call(url, "read", json!({"path": "src/lib.rs", "at": at}))["content"].as_str().unwrap().to_string()
}

#[test]
fn two_agents_in_one_file_become_two_changes_and_both_land() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, _) = setup(tmp.path());
    let a = open(&url, "a", json!({"symbols": ["parse"]}));
    let b = open(&url, "b", json!({"symbols": ["render"]}));
    let v = assign(&url, &[&a, &b], &edit(&[PARSE, RENDER]));
    let (ha, hb) = (hunks_of(&v, &a), hunks_of(&v, &b));
    assert_eq!((ha.len(), hb.len()), (1, 1), "{v}");
    assert_eq!(ha[0]["why"], "intent");
    assert_eq!(ha[0]["old"], json!([4, 5]));
    assert!(ha[0]["diff"][1].as_str().unwrap().contains("trim"), "{v}");
    assert_eq!(v["unassigned"], json!([]));

    for ch in [&a, &b] {
        let r = call(&url, "apply_assigned", json!({"change": ch}));
        assert_eq!(r["applied"].as_array().unwrap().len(), 1, "{r}");
        assert_eq!(r["paths"], json!(["src/lib.rs"]));
    }
    assert_eq!(read(&url, &a), edit(&[PARSE]));
    assert_eq!(read(&url, &b), edit(&[RENDER]));
    // Applying again writes nothing new.
    assert_eq!(call(&url, "apply_assigned", json!({"change": a}))["paths"], json!([]));

    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    // B moves onto the new main, then grows its hunk: the write is merged onto the rebased head.
    call(&url, "rebase", json!({"change": b}));
    let grown = ("pub fn render(n: u32)", "pub fn render(n: u64)");
    let v = assign(&url, &[&a, &b], &edit(&[PARSE, RENDER, grown]));
    assert_eq!(v["landed"][0]["landed_in"], a.as_str(), "{v}");
    assert_eq!(v["unassigned"], json!([]));
    assert_eq!(hunks_of(&v, &b)[0]["why"], "intent");
    call(&url, "apply_assigned", json!({"change": b}));
    assert_eq!(read(&url, &b), edit(&[PARSE, RENDER, grown]));
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    assert_eq!(read(&url, "main"), edit(&[PARSE, RENDER, grown]));
}

#[test]
fn a_claim_moves_a_hunk_and_follows_it_while_it_grows() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, _) = setup(tmp.path());
    let a = open(&url, "a", json!({"paths": ["src/lib.rs"]}));
    let b = open(&url, "b", json!({}));
    let v = assign(&url, &[&a, &b], &edit(&[PARSE, RENDER]));
    assert_eq!(hunks_of(&v, &a).len(), 2, "{v}");
    call(&url, "apply_assigned", json!({"change": a}));
    assert_eq!(read(&url, &a), edit(&[PARSE, RENDER]));

    let render = hunks_of(&v, &a)[1]["id"].as_str().unwrap().to_string();
    let v = call(&url, "claim", json!({"change": b, "hunks": [render]}));
    assert_eq!(hunks_of(&v, &b)[0]["id"], render.as_str(), "{v}");
    assert_eq!(hunks_of(&v, &b)[0]["why"], "claim");
    assert_eq!(hunks_of(&v, &a).len(), 1);
    for ch in [&a, &b] {
        call(&url, "apply_assigned", json!({"change": ch}));
    }
    assert_eq!(read(&url, &a), edit(&[PARSE]));
    assert_eq!(read(&url, &b), edit(&[RENDER]));

    // B keeps editing its hunk: the id changes, the claim (and so the owner) stays.
    let more = edit(&[PARSE, RENDER, ("pub fn render(n: u32)", "pub fn render(n: u64)")]);
    let v = assign(&url, &[&a, &b], &more);
    let hb = hunks_of(&v, &b);
    assert_eq!(hb.len(), 1, "{v}");
    assert_ne!(hb[0]["id"], render.as_str());
    assert_eq!(hb[0]["why"], "claim");
    call(&url, "apply_assigned", json!({"change": b}));
    assert_eq!(read(&url, &b), edit(&[RENDER, ("pub fn render(n: u32)", "pub fn render(n: u64)")]));
    // Unknown ids are refused.
    let e = treeyard::client::call(&url, "claim", json!({"change": b, "hunks": ["h000000000000"]}));
    assert!(e.unwrap_err().to_string().contains("no hunk"));
}

#[test]
fn a_hunk_that_builds_on_another_changes_lines_is_held() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, _) = setup(tmp.path());
    let a = open(&url, "a", json!({"paths": ["src/lib.rs"]}));
    let b = open(&url, "b", json!({}));
    let helper = format!("{LIB}\npub fn helper() -> u32 {{\n    1\n}}\n");
    let v = assign(&url, &[&a, &b], &helper);
    assert_eq!(hunks_of(&v, &a).len(), 1, "{v}");
    call(&url, "apply_assigned", json!({"change": a}));
    assert_eq!(read(&url, &a), helper);

    // B now declares helper and edits a line that exists only because of A's hunk.
    call(&url, "declare", json!({"change": b, "symbols": ["helper"]}));
    let changed = helper.replace("    1\n}\n", "    2\n}\n");
    let v = assign(&url, &[&a, &b], &changed);
    let hb = hunks_of(&v, &b);
    assert_eq!(hb.len(), 1, "{v}");
    assert_eq!(hb[0]["depends_on"], json!([a]), "{v}");
    let r = call(&url, "apply_assigned", json!({"change": b}));
    assert_eq!((r["applied"].as_array().unwrap().len(), r["held"].as_array().unwrap().len()), (0, 1), "{r}");
    assert_eq!(r["paths"], json!([]));
    // A keeps its own version of the hunk; nothing is split.
    let r = call(&url, "apply_assigned", json!({"change": a}));
    assert_eq!(r["paths"], json!([]), "{r}");
    assert_eq!(read(&url, &a), helper);

    // A claims the grown hunk: no dependency left, and A gets B's line.
    let id = hb[0]["id"].as_str().unwrap();
    let v = call(&url, "claim", json!({"change": a, "hunks": [id]}));
    assert!(hunks_of(&v, &a)[0].get("depends_on").is_none(), "{v}");
    call(&url, "apply_assigned", json!({"change": a}));
    assert_eq!(read(&url, &a), changed);
}

#[test]
fn hunk_ids_survive_edits_elsewhere_in_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, _) = setup(tmp.path());
    let a = open(&url, "a", json!({}));
    let v = assign(&url, &[&a], &edit(&[PARSE]));
    let un = v["unassigned"].as_array().unwrap();
    assert_eq!(un.len(), 1, "{v}");
    let id = un[0]["id"].as_str().unwrap().to_string();
    assert_eq!(un[0]["why"], "none");

    call(&url, "claim", json!({"change": a, "hunks": [id]}));
    let top = format!("// header\n{}", edit(&[PARSE]));
    let v = assign(&url, &[&a], &top);
    let ha = hunks_of(&v, &a);
    assert_eq!(ha.len(), 1, "{v}");
    assert_eq!(ha[0]["id"], id.as_str(), "the parse hunk keeps its id");
    assert_eq!(ha[0]["new"], json!([5, 6]));
    assert_eq!(v["unassigned"].as_array().unwrap().len(), 1, "the new header line has nobody");
}

#[test]
fn share_cli_joins_and_syncs_a_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, src) = setup(tmp.path());
    let bin = env!("CARGO_BIN_EXE_treeyard");
    let run = |args: &[&str]| -> Value {
        let out = std::process::Command::new(bin).args(args).env("TREEYARD_URL", &url).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let dir = src.to_str().unwrap();
    let a = run(&["share", "join", "--dir", dir, "--agent", "a", "--intent", "parse", "--symbols", "parse"]);
    let b = run(&["share", "join", "--dir", dir, "--agent", "b", "--intent", "render", "--symbols", "render"]);
    std::fs::write(src.join("src/lib.rs"), edit(&[PARSE, RENDER])).unwrap();
    std::fs::write(src.join("NOTES.md"), "notes\n").unwrap();
    let v = run(&["share", "sync", "--dir", dir]);
    assert_eq!(v["agents"]["a"]["applied"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["agents"]["b"]["applied"].as_array().unwrap().len(), 1, "{v}");
    let un = v["unassigned"].as_array().unwrap();
    assert_eq!(un.len(), 1, "{v}");
    assert_eq!(un[0]["path"], "NOTES.md");
    assert_eq!(read(&url, a["change"].as_str().unwrap()), edit(&[PARSE]));
    assert_eq!(read(&url, b["change"].as_str().unwrap()), edit(&[RENDER]));

    // b takes the new file.
    let id = un[0]["id"].as_str().unwrap();
    run(&["share", "claim", "--dir", dir, "--agent", "b", id]);
    let v = run(&["share", "sync", "--dir", dir]);
    assert_eq!(v["unassigned"], json!([]), "{v}");
    let notes = call(&url, "read", json!({"path": "NOTES.md", "at": b["change"]}));
    assert_eq!(notes["content"], "notes\n");
}
