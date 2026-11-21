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
/// A land that replaces another agent's landed line, with the owner's yes.
fn land_yes(url: &str, ch: &str) -> Value {
    call(url, "land", json!({"change": ch, "undo": true, "reason": "the test settles the conflict on one side"}))
}

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
    let ch = call(&url, "change", json!({"change": a, "full": true}));
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

/// A conflict comes back as regions of the file with markers, not as three whole files, and the
/// regions alone are enough to resolve it: one `replace` per region, old = the region's text.
#[test]
fn conflicts_come_as_regions_that_resolve_with_replace() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path(), "");
    let f00 = read(&url, "f00.txt", "main");
    let a = open(&url, "a");
    write(&url, &a, "f00.txt", &set_line(&set_line(&f00, 3, "a's line 3"), 25, "a's line 25"));
    let b = open(&url, "b");
    write(&url, &b, "f00.txt", &set_line(&set_line(&f00, 3, "b's line 3"), 25, "b's line 25"));
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    let land = call(&url, "land", json!({"change": a}));
    assert_eq!(land["status"], "conflict", "{land}");
    let c = &land["change"]["conflicts"][0];
    assert!(c.get("text").is_none(), "no whole files without full: {c}");
    assert!(c["ours"]["size"].as_u64().unwrap() > 0 && c["theirs"]["blob"].is_string(), "{c}");
    let regions = c["regions"].as_array().unwrap();
    assert_eq!(regions.len(), 2, "{c}");
    // Line numbers are those of the change's text with markers; context is the lines around.
    let marked = read(&url, "f00.txt", &a);
    let lines: Vec<&str> = marked.lines().collect();
    let r0 = &regions[0];
    let (lo, hi) = (r0["lines"][0].as_u64().unwrap() as usize, r0["lines"][1].as_u64().unwrap() as usize);
    assert_eq!(lines[lo - 1], "<<<<<<< main");
    assert_eq!(lines[hi - 1], ">>>>>>> change");
    assert!(r0["text"].as_str().unwrap().contains("b's line 3\n=======\na's line 3\n"), "{r0}");
    assert_eq!(r0["before"].as_str().unwrap(), format!("{}\n{}\n{}\n", lines[lo - 4], lines[lo - 3], lines[lo - 2]));
    assert!(land["change"]["resolve"].as_str().unwrap().contains("replace"), "help once per reply");
    let reply = serde_json::to_string(&land).unwrap();
    assert!(reply.len() < f00.len() + 3000, "the reply is smaller than the file: {} bytes", reply.len());

    let edits: Vec<Value> = regions
        .iter()
        .enumerate()
        .map(|(i, r)| json!({"path": "f00.txt", "replace": {"old": r["text"], "new": format!("merged {i}\n")}}))
        .collect();
    let after = call(&url, "apply", json!({"change": a, "edits": edits}));
    assert_eq!(after["conflicted"], false, "{after}");
    // The merged text replaces b's landed lines: that takes the owner's yes (`undoes`).
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "undoes");
    assert_eq!(land_yes(&url, &a)["status"], "landed");
    let main = read(&url, "f00.txt", "main");
    assert!(main.contains("merged 0\n") && main.contains("merged 1\n") && !main.contains("<<<<<<<"), "{main}");

    // full = true still returns the three texts.
    let c2 = open(&url, "c");
    write(&url, &c2, "f00.txt", &set_line(&f00, 3, "c's line 3"));
    let d = open(&url, "d");
    write(&url, &d, "f00.txt", &set_line(&main, 3, "d's line 3"));
    assert_eq!(land_yes(&url, &d)["status"], "landed");
    assert_eq!(call(&url, "land", json!({"change": c2}))["status"], "conflict");
    let full = call(&url, "change", json!({"change": c2, "full": true}));
    assert!(full["conflicts"][0]["text"]["theirs"].as_str().unwrap().contains("d's line 3"), "{full}");
    assert!(full["conflicts"][0]["regions"].is_array());
}

#[test]
fn a_kept_conflict_that_main_resolves_clears_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let (url, a) = conflicted(tmp.path(), "[coordination]\ncontinuous_rebase = true\nkeep_conflicted = true\n");
    // d sets line 3 to exactly a's version: the next rebase merges cleanly.
    let d = open(&url, "d");
    write(&url, &d, "f00.txt", &set_line(&read(&url, "f00.txt", "main"), 3, "a's line 3"));
    assert_eq!(land_yes(&url, &d)["status"], "landed");
    let ch = call(&url, "change", json!({"change": a}));
    assert_eq!(ch["conflicts"], json!([]), "{ch}");
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
}

#[test]
fn a_resolution_that_keeps_the_markers_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path(), "");
    let f00 = read(&url, "f00.txt", "main");
    let a = open(&url, "a");
    write(&url, &a, "f00.txt", &set_line(&f00, 3, "a's line 3"));
    let b = open(&url, "b");
    write(&url, &b, "f00.txt", &set_line(&f00, 3, "b's line 3"));
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "conflict");
    let with_markers = read(&url, "f00.txt", &a);
    assert!(with_markers.contains("<<<<<<< main\n") && with_markers.contains(">>>>>>> change\n"), "{with_markers}");

    // Seen with a real agent: a replace inside one side left the markers, and land put them on main.
    let e = treeyard::client::call(
        &url,
        "apply",
        json!({"change": a, "edits": [{"path": "f00.txt", "replace": {"old": "a's line 3", "new": "a and b's line 3"}}]}),
    )
    .unwrap_err();
    assert!(format!("{e:#}").contains("still holds the conflict marker `<<<<<<< main`"), "{e:#}");
    assert_eq!(call(&url, "change", json!({"change": a}))["conflicted"], true, "a refused write resolves nothing");

    let main_f00 = read(&url, "f00.txt", "main");
    write(&url, &a, "f00.txt", &set_line(&main_f00, 3, "a and b's line 3"));
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    let landed = read(&url, "f00.txt", "main");
    assert!(!landed.contains("<<<<<<<") && landed.contains("a and b's line 3\n"), "{landed}");
}

#[test]
fn one_apply_may_remove_the_markers_with_several_replace_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path(), "");
    let f00 = read(&url, "f00.txt", "main");
    let a = open(&url, "a");
    write(&url, &a, "f00.txt", &set_line(&f00, 3, "a's line 3"));
    let b = open(&url, "b");
    write(&url, &b, "f00.txt", &set_line(&f00, 3, "b's line 3"));
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "conflict");
    let marked = read(&url, "f00.txt", &a);
    assert!(marked.contains("<<<<<<< main\nb's line 3\n=======\na's line 3\n>>>>>>> change\n"), "{marked}");

    // One edit that leaves markers: the error says that one apply must remove all of them.
    let one = json!({"path": "f00.txt", "replace": {"old": ">>>>>>> change\n", "new": ""}});
    let e = treeyard::client::call(&url, "apply", json!({"change": a, "edits": [one.clone()]})).unwrap_err();
    let msg = format!("{e:#}");
    assert!(msg.contains("One apply must remove every conflict marker of f00.txt"), "{msg}");
    assert!(msg.contains("1 conflict region left"), "{msg}");

    // Both edits in the same apply: the file is checked once, after both, and it is clean.
    let two = json!({"path": "f00.txt", "replace": {"old": "<<<<<<< main\nb's line 3\n=======\n", "new": ""}});
    let r = call(&url, "apply", json!({"change": a, "edits": [one, two]}));
    assert_eq!(r["conflicted"], false, "{r}");
    assert_eq!(land_yes(&url, &a)["status"], "landed");
    assert_eq!(read(&url, "f00.txt", "main"), set_line(&f00, 3, "a's line 3"));
}

#[test]
fn a_replace_that_misses_says_how_far_behind_main_the_change_is() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path(), "");
    let a = open(&url, "a");
    for (agent, new) in [("b", "b's line 3"), ("c", "c's line 4")] {
        let ch = open(&url, agent);
        let old = if agent == "b" { "file 0 line 3\n" } else { "file 0 line 4\n" };
        call(
            &url,
            "apply",
            json!({"change": ch, "edits": [{"path": "f00.txt", "replace": {"old": old, "new": format!("{new}\n")}}]}),
        );
        assert_eq!(call(&url, "land", json!({"change": ch}))["status"], "landed");
    }
    let miss = |old: &str| -> String {
        let e = treeyard::client::call(
            &url,
            "apply",
            json!({"change": a, "edits": [{"path": "f00.txt", "replace": {"old": old, "new": "x"}}]}),
        )
        .unwrap_err();
        format!("{e:#}")
    };
    // The agent read main, where another agent's text is; its change is two landings behind.
    let m = miss("b's line 3");
    assert!(m.starts_with("replace in f00.txt: old text not found"), "{m}");
    assert!(m.contains(&format!("your change {a} is 2 landings behind main, and main has this text")), "{m}");
    assert!(m.contains(&format!("Call rebase with change {a}")) && m.contains(&format!("at = {a}")), "{m}");
    // Text that is nowhere.
    let m = miss("no such text");
    assert!(m.contains("main lacks it too (main is 2 landings ahead") && m.contains(&format!("at = {a}")), "{m}");

    // After rebase: the text of main is found; text the change itself replaced gets its own hint.
    call(&url, "rebase", json!({"change": a}));
    call(
        &url,
        "apply",
        json!({"change": a, "edits": [{"path": "f00.txt", "replace": {"old": "b's line 3", "new": "a's line 3"}}]}),
    );
    let m = miss("b's line 3");
    assert!(m.contains(&format!("main has this text, but your change {a} already changed it")), "{m}");
    // Other errors keep their text.
    let e = treeyard::client::call(
        &url,
        "apply",
        json!({"change": a, "edits": [{"path": "f00.txt", "replace": {"old": "file 0 line", "new": "x"}}]}),
    )
    .unwrap_err();
    assert!(format!("{e:#}").ends_with("add context or set all=true"), "{e:#}");
}
