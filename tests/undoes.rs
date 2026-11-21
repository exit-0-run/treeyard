//! `undoes`: one test per probed case. Each case starts from a file whose lines agent `a` landed,
//! then agent `b` edits it. Quiet cases keep a's behaviour (moved, re-indented, renamed, other
//! line ends, extended); loud ones remove a's text.

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use treeyard::repo::Repo;

fn setup(root: &Path) -> String {
    let src = fixture_repo(root, 2, 10, &[]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

fn open(url: &str, agent: &str, extra: Value) -> String {
    let mut p = json!({"agent": agent, "intent": format!("work of {agent}")});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    call(url, "open_change", p)["id"].as_str().unwrap().to_string()
}

fn write(url: &str, ch: &str, path: &str, content: &str) -> Value {
    call(url, "apply", json!({"change": ch, "edits": [{"path": path, "content": content}]}))
}

/// Agent a lands `path` with `text`; returns the url.
fn landed(root: &Path, path: &str, text: &str) -> String {
    let url = setup(root);
    let a = open(&url, "a", json!({"session": "s-a"}));
    write(&url, &a, path, text);
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    url
}

/// b's apply of `path` = `new` on top of a's landed `old`: the `undoes` entries.
fn undoes_of(old: &str, new: &str) -> Vec<Value> {
    let tmp = tempfile::tempdir().unwrap();
    let url = landed(tmp.path(), "m.py", old);
    let b = open(&url, "b", json!({}));
    write(&url, &b, "m.py", new)["undoes"].as_array().unwrap().clone()
}

const A: &str = "def f(value):\n    value = value.lower()\n    return value\n";

#[test]
fn a_rewrite_that_keeps_every_word_is_quiet_one_that_drops_a_word_warns() {
    assert_eq!(undoes_of(A, &A.replace("value.lower()", "value.lower().strip()")), Vec::<Value>::new());
    let u = undoes_of(A, &A.replace("value = value.lower()", "value = value"));
    assert_eq!(u.len(), 1, "{u:?}");
    assert_eq!(u[0]["lines"], json!([2, 2]));
    assert_eq!(u[0]["text"], "    value = value.lower()\n");
    assert_eq!(u[0]["agent"], "a");
}

#[test]
fn a_block_moved_into_a_helper_or_another_file_is_quiet() {
    let old = "def f(x):\n    y = x * 2\n    z = y + 1\n    return z\n";
    let moved = "def g(x):\n    y = x * 2\n    z = y + 1\n    return z\n\ndef f(x):\n    return g(x)\n";
    assert_eq!(undoes_of(old, moved), Vec::<Value>::new());

    let tmp = tempfile::tempdir().unwrap();
    let url = landed(tmp.path(), "m.py", old);
    let b = open(&url, "b", json!({}));
    let v = call(
        &url,
        "apply",
        json!({"change": b, "edits": [
            {"path": "m.py", "content": "from h import f\n"},
            {"path": "h.py", "content": old},
        ]}),
    );
    assert_eq!(v["undoes"], json!([]), "a file renamed or moved: {v}");
}

#[test]
fn a_reindented_block_is_quiet() {
    let old = "def f(x):\n    y = x * 2\n    return y\n";
    let wrapped =
        "def f(x):\n    try:\n        y = x * 2\n        return y\n    except ValueError:\n        return None\n";
    assert_eq!(undoes_of(old, wrapped), Vec::<Value>::new());
}

#[test]
fn a_rename_is_quiet_a_delete_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let url = landed(tmp.path(), "m.py", A);
    let b = open(&url, "b", json!({}));
    let v = call(
        &url,
        "apply",
        json!({"change": b, "edits": [{"path": "m.py", "delete": true}, {"path": "n.py", "content": A}]}),
    );
    assert_eq!(v["undoes"], json!([]), "{v}");
    let v = call(&url, "apply", json!({"change": b, "edits": [{"path": "n.py", "delete": true}]}));
    assert_eq!(v["undoes"].as_array().unwrap().len(), 1, "the text is gone now: {v}");
    assert_eq!(v["undoes"][0]["lines"], json!([1, 3]), "{v}");
}

#[test]
fn appending_after_a_last_line_without_newline_is_quiet() {
    let old = "x = 1\ny = 2";
    assert_eq!(undoes_of(old, "x = 1\ny = 2\nz = 3\n"), Vec::<Value>::new());
}

#[test]
fn crlf_to_lf_is_quiet() {
    let old = "a = 1\r\nb = 2\r\nc = 3\r\n";
    assert_eq!(undoes_of(old, "a = 1\nb = 2\nc = 3\n"), Vec::<Value>::new());
}

#[test]
fn resolve_take_ours_says_it_removes_the_landed_line_and_land_says_it_too() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path());
    let f = call(&url, "read", json!({"path": "f00.txt"}))["content"].as_str().unwrap().to_string();
    let set = |line: &str| f.replacen("file 0 line 3", line, 1);
    let a = open(&url, "a", json!({}));
    write(&url, &a, "f00.txt", &set("a's line three"));
    let b = open(&url, "b", json!({}));
    write(&url, &b, "f00.txt", &set("b's line three"));
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "conflict");
    let v = call(&url, "resolve", json!({"change": b, "path": "f00.txt", "take": "ours"}));
    let u = v["undoes"].as_array().unwrap();
    assert_eq!(u.len(), 1, "take = ours drops a's landed line: {v}");
    assert_eq!(u[0]["text"], "a's line three\n");
    assert_eq!(u[0]["agent"], "a");
    let land = call(&url, "land", json!({"change": b}));
    assert_eq!(land["status"], "landed", "a land is never blocked: {land}");
    assert_eq!(land["undoes"].as_array().unwrap().len(), 1, "the land reply says so: {land}");
    assert!(land["undoes_advice"].is_string());
}

#[test]
fn a_change_stacked_on_an_open_change_that_removes_its_line_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path());
    let p = open(&url, "a", json!({}));
    write(&url, &p, "m.py", A);
    let s = open(&url, "b", json!({"base": p}));
    let v = write(&url, &s, "m.py", &A.replace("    value = value.lower()\n", ""));
    let u = v["undoes"].as_array().unwrap();
    assert_eq!(u.len(), 1, "{v}");
    assert_eq!(u[0]["change"], p.as_str());
    assert_eq!(u[0]["landed"], false, "the parent has not landed: {u:?}");
}

#[test]
fn one_session_under_two_agent_names_is_own_work() {
    let tmp = tempfile::tempdir().unwrap();
    let url = landed(tmp.path(), "m.py", A);
    let removed = A.replace("    value = value.lower()\n", "");
    // Same session as a's change: the same agent under a second name.
    let b = open(&url, "a-2", json!({"session": "s-a"}));
    assert_eq!(write(&url, &b, "m.py", &removed)["undoes"], json!([]));
    // Another session (or none): agent names are free strings, so this one counts as foreign.
    let c = open(&url, "a-3", json!({"session": "s-other"}));
    assert_eq!(write(&url, &c, "m.py", &removed)["undoes"].as_array().unwrap().len(), 1);
}

#[test]
fn an_entry_is_sent_once_and_change_lists_all() {
    let tmp = tempfile::tempdir().unwrap();
    let url = landed(tmp.path(), "m.py", A);
    let b = open(&url, "b", json!({}));
    let v = write(&url, &b, "m.py", &A.replace("    value = value.lower()\n", ""));
    assert_eq!(v["undoes"].as_array().unwrap().len(), 1);
    // An edit to another file: the entry stands but is not repeated.
    let v = write(&url, &b, "f01.txt", "other\n");
    assert_eq!(v["undoes"], json!([]), "{v}");
    assert_eq!(v["undoes_earlier"], 1, "{v}");
    assert!(v.get("undoes_advice").is_none(), "{v}");
    let ch = call(&url, "change", json!({"change": b}));
    assert_eq!(ch["undoes"].as_array().unwrap().len(), 1, "change gives the full list: {ch}");
    // Putting the line back clears it; removing it again reports it again.
    let v = write(&url, &b, "m.py", A);
    assert_eq!((v["undoes"].clone(), v.get("undoes_earlier").cloned()), (json!([]), None), "{v}");
    let v = write(&url, &b, "m.py", &A.replace("    value = value.lower()\n", ""));
    assert_eq!(v["undoes"].as_array().unwrap().len(), 1, "{v}");
}

#[test]
fn more_than_twenty_entries_are_counted_in_undoes_more() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 60, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let f = call(&url, "read", json!({"path": "f00.txt"}))["content"].as_str().unwrap().to_string();
    // a rewrites every second line: 25 runs of its own, each between two imported lines.
    let mine: String = f
        .lines()
        .enumerate()
        .map(|(i, l)| if i % 2 == 0 && i < 50 { format!("a wrote {i}\n") } else { format!("{l}\n") })
        .collect();
    let a = open(&url, "a", json!({}));
    write(&url, &a, "f00.txt", &mine);
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    let b = open(&url, "b", json!({}));
    let v = call(&url, "apply", json!({"change": b, "edits": [{"path": "f00.txt", "delete": true}]}));
    assert_eq!(v["undoes"].as_array().unwrap().len(), 20, "{v}");
    assert_eq!(v["undoes_more"], 5, "the cut says how many are left out: {v}");
    assert_eq!(serde_json::to_string(&v).unwrap().matches("another agent").count(), 1, "advice once");
    let ch = call(&url, "change", json!({"change": b}));
    assert_eq!((ch["undoes"].as_array().unwrap().len(), ch["undoes_more"].clone()), (20, json!(5)));
}

#[test]
fn entries_obey_the_callers_read_globs() {
    use treeyard::client::call_with;
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 3, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let repo = Arc::new(Repo::open(&data).unwrap());
    let (_, ta) = repo.auth.create("a", false, &["**".into()], &["**".into()]).unwrap();
    // b may write everywhere but read only src/.
    let (_, tb) = repo.auth.create("b", false, &["src/**".into()], &["**".into()]).unwrap();
    let url = spawn_server(repo);
    let ok = |t: &str, op: &str, p: Value| call_with(&url, Some(t), op, p).unwrap_or_else(|e| panic!("{op}: {e:#}"));
    let a = ok(&ta, "open_change", json!({"intent": "add two helpers"}))["id"].as_str().unwrap().to_string();
    ok(
        &ta,
        "apply",
        json!({"change": a, "edits": [{"path": "src/n.py", "content": A}, {"path": "secret/m.py", "content": A}]}),
    );
    assert_eq!(ok(&ta, "land", json!({"change": a}))["status"], "landed");
    let b = ok(&tb, "open_change", json!({"intent": "drop both"}))["id"].as_str().unwrap().to_string();
    let v = ok(
        &tb,
        "apply",
        json!({"change": b, "edits": [{"path": "src/n.py", "delete": true}, {"path": "secret/m.py", "delete": true}]}),
    );
    let u = v["undoes"].as_array().unwrap();
    assert_eq!(u.len(), 1, "only the path b may read: {v}");
    assert_eq!(u[0]["path"], "src/n.py");
    assert!(!v.to_string().contains("secret/"), "{v}");
}
