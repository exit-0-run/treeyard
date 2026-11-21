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
fn resolve_take_ours_says_it_removes_the_landed_line_and_land_holds_it() {
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
    let main = call(&url, "status", json!({}))["main"].clone();
    let land = call(&url, "land", json!({"change": b}));
    assert_eq!(land["status"], "undoes", "the land is held: {land}");
    assert_eq!(land["undoes"].as_array().unwrap().len(), 1, "{land}");
    assert_eq!(land["undoes"][0]["text"], "a's line three\n", "{land}");
    assert!(land["advice"].as_str().unwrap().contains("land again with undo = true"), "{land}");
    assert_eq!(call(&url, "status", json!({}))["main"], main, "main did not move");
    assert_eq!(call(&url, "change", json!({"change": b}))["state"], "open");
    let ev = call(&url, "events", json!({"kinds": ["undoes"]}))["events"].clone();
    assert_eq!(ev.as_array().unwrap().len(), 1, "{ev}");
    assert_eq!(ev[0]["cause"], "land");
    assert_eq!(ev[0]["agents"], json!(["b", "a"]), "{ev}");
    assert_eq!(ev[0]["changes"], json!([b, a]), "{ev}");

    // The override: undo needs a reason, a reason needs undo.
    let e = treeyard::client::call(&url, "land", json!({"change": b, "undo": true})).unwrap_err();
    assert!(format!("{e:#}").contains("needs a non-empty reason"), "{e:#}");
    let e = treeyard::client::call(&url, "land", json!({"change": b, "undo": true, "reason": "  "})).unwrap_err();
    assert!(format!("{e:#}").contains("needs a non-empty reason"), "{e:#}");
    let e = treeyard::client::call(&url, "land", json!({"change": b, "reason": "x"})).unwrap_err();
    assert!(format!("{e:#}").contains("reason goes with undo"), "{e:#}");
    let why = "my task sets line 3 to b's text";
    let land = call(&url, "land", json!({"change": b, "undo": true, "reason": why}));
    assert_eq!(land["status"], "landed", "{land}");
    assert_eq!((land["undid"]["by"].as_str(), land["undid"]["reason"].as_str()), (Some("b"), Some(why)), "{land}");
    assert_eq!(land["undid"]["undoes"][0]["agent"], "a");
    assert_eq!(land["undoes"].as_array().unwrap().len(), 1, "the reply still lists them: {land}");

    // People see it without the transcripts: change, events, status, log, audit.
    let ch = call(&url, "change", json!({"change": b}));
    assert_eq!((ch["state"].as_str(), ch["undid"]["reason"].as_str()), (Some("landed"), Some(why)), "{ch}");
    assert_eq!(ch["undid"]["undoes"][0]["change"], a.as_str(), "{ch}");
    let ev = call(&url, "events", json!({"kinds": ["undo_override"]}))["events"].clone();
    assert_eq!(ev.as_array().unwrap().len(), 1, "{ev}");
    assert_eq!((ev[0]["by"].as_str(), ev[0]["reason"].as_str()), (Some("b"), Some(why)), "{ev}");
    assert_eq!(ev[0]["agents"], json!(["a"]), "{ev}");
    assert_eq!(ev[0]["undoes"][0]["lines"], json!([4, 4]), "{ev}");
    assert!(ev[0]["undoes"][0].get("text").is_none(), "events stay small: {ev}");
    let st = call(&url, "status", json!({}));
    assert_eq!(st["undid_total"], 1, "{st}");
    assert_eq!((st["undid"][0]["change"].as_str(), st["undid"][0]["by"].as_str()), (Some(b.as_str()), Some("b")));
    assert_eq!(st["undid"][0]["agents"], json!(["a"]));
    let log = call(&url, "log", json!({"limit": 5}));
    let entry = log["entries"].as_array().unwrap().iter().find(|e| e["change"] == b.as_str()).unwrap().clone();
    assert_eq!(entry["undid"]["reason"], why, "{entry}");
    assert!(log["entries"].as_array().unwrap().iter().filter(|e| e.get("undid").is_some()).count() == 1);
    let audit = call(&url, "audit", json!({"change": b, "op": "land"}));
    let rows: Vec<&Value> = audit["entries"].as_array().unwrap().iter().collect();
    let statuses: Vec<&str> = rows.iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert!(statuses.contains(&"undoes") && statuses.contains(&"landed"), "{audit}");
    let landed = rows.iter().find(|r| r["status"] == "landed").unwrap();
    let detail = landed["detail"].as_str().unwrap();
    assert!(detail.contains("b removed landed lines of a (") && detail.contains(why), "{detail}");
}

#[test]
fn own_lines_moved_lines_and_reindented_lines_do_not_hold_a_land() {
    let tmp = tempfile::tempdir().unwrap();
    let url = landed(tmp.path(), "m.py", A);
    // a removes its own line: own work.
    let a2 = open(&url, "a", json!({}));
    write(&url, &a2, "m.py", &format!("{A}# a's note\n"));
    assert_eq!(call(&url, "land", json!({"change": a2}))["status"], "landed");
    let a3 = open(&url, "a", json!({}));
    write(&url, &a3, "m.py", A);
    assert_eq!(call(&url, "land", json!({"change": a3}))["status"], "landed");
    // b moves a's function into another file, then re-indents it in a class.
    let b = open(&url, "b", json!({}));
    let v = call(
        &url,
        "apply",
        json!({"change": b, "edits": [{"path": "m.py", "content": "from h import f\n"}, {"path": "h.py", "content": A}]}),
    );
    assert_eq!(v["undoes"], json!([]), "{v}");
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "landed");
    let c = open(&url, "c", json!({}));
    let indented: String = A.lines().map(|l| format!("    {l}\n")).collect();
    write(&url, &c, "h.py", &format!("class H:\n{indented}"));
    let land = call(&url, "land", json!({"change": c}));
    assert_eq!(land["status"], "landed", "{land}");
    assert_eq!(land["undoes"], json!([]), "{land}");
    assert!(land.get("undid").is_none(), "{land}");
}

#[test]
fn lines_of_the_change_it_was_stacked_on_do_not_hold_a_land() {
    let tmp = tempfile::tempdir().unwrap();
    let url = setup(tmp.path());
    let p = open(&url, "a", json!({}));
    write(&url, &p, "m.py", A);
    let s = open(&url, "b", json!({"base": p}));
    assert_eq!(call(&url, "change", json!({"change": s}))["stacked_on"], json!([p]));
    write(&url, &s, "m.py", &A.replace("    value = value.lower()\n", ""));
    assert_eq!(call(&url, "land", json!({"change": s}))["status"], "waiting");
    assert_eq!(call(&url, "land", json!({"change": p}))["status"], "landed");
    let land = call(&url, "land", json!({"change": s}));
    assert_eq!(land["status"], "landed", "the parent's lines were removed while stacked on it: {land}");
    assert_eq!(land["undoes"].as_array().unwrap().len(), 1, "still listed: {land}");
}

#[test]
fn hold_undoes_false_lands_as_before() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 2, 10, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    std::fs::write(data.join("treeyard.toml"), "[coordination]\nhold_undoes = false\n").unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let a = open(&url, "a", json!({}));
    write(&url, &a, "m.py", A);
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "landed");
    let b = open(&url, "b", json!({}));
    write(&url, &b, "m.py", &A.replace("    value = value.lower()\n", ""));
    let land = call(&url, "land", json!({"change": b}));
    assert_eq!(land["status"], "landed", "{land}");
    assert_eq!(land["undoes"].as_array().unwrap().len(), 1, "the reply says so: {land}");
    assert!(land["undoes_advice"].is_string() && land.get("undid").is_none(), "{land}");
    assert_eq!(call(&url, "status", json!({}))["undid_total"], 0);
}

#[test]
fn queue_mode_holds_at_land_and_again_before_the_batch() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 2, 10, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    std::fs::write(data.join("treeyard.toml"), "[landing]\nmode = \"queue\"\ntest = \"true\"\n").unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let a = open(&url, "a", json!({}));
    write(&url, &a, "m.py", A);
    assert_eq!(call(&url, "land", json!({"change": a}))["status"], "queued");
    assert_eq!(call(&url, "queue_run", json!({}))["landed"].as_array().unwrap().len(), 1);
    let removed = A.replace("    value = value.lower()\n", "");
    let b = open(&url, "b", json!({}));
    write(&url, &b, "m.py", &removed);
    assert_eq!(call(&url, "land", json!({"change": b}))["status"], "undoes");
    assert_eq!(call(&url, "queue", json!({}))["queue"], json!([]), "not queued");
    let q = call(&url, "land", json!({"change": b, "undo": true, "reason": "the task drops lower()"}));
    assert_eq!((q["status"].as_str(), q["undid"]["by"].as_str()), (Some("queued"), Some("b")), "{q}");
    // A later edit is not covered by the yes: the queue drops the change before the batch.
    write(&url, &b, "m.py", &format!("{removed}# more\n"));
    let step = call(&url, "queue_run", json!({}));
    assert_eq!(step["status"], "idle", "{step}");
    assert_eq!(step["notes"][0]["status"], "undoes", "{step}");
    assert_eq!(call(&url, "land_status", json!({"change": b}))["verdict"], "undoes");
    assert_eq!(call(&url, "events", json!({"kinds": ["undoes"]}))["events"][1]["cause"], "queue");
    // A new yes for this head: it lands and the record shows it.
    call(&url, "land", json!({"change": b, "undo": true, "reason": "the task drops lower()"}));
    assert_eq!(call(&url, "queue_run", json!({}))["landed"][0]["change"], b.as_str());
    assert_eq!(call(&url, "status", json!({}))["undid"][0]["change"], b.as_str());
    assert_eq!(call(&url, "events", json!({"kinds": ["undo_override"]}))["events"].as_array().unwrap().len(), 1);
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
    // The held land shows only the readable entry and counts the other.
    let land = ok(&tb, "land", json!({"change": b}));
    assert_eq!(land["status"], "undoes", "{land}");
    assert_eq!((land["undoes"].as_array().unwrap().len(), land["undoes_hidden"].clone()), (1, json!(1)), "{land}");
    assert!(!land.to_string().contains("secret/"), "{land}");
}

const NORM: &str =
    "def norm(value):\n    normed_value = value\n    normed_value = normed_value.casefold()\n    return normed_value\n";
const CASEFOLD: &str = "    normed_value = normed_value.casefold()\n";
const STRIP: &str = "    normed_value = normed_value.strip().casefold()\n";

fn with_line(line: &str) -> String {
    NORM.replace(CASEFOLD, line)
}

/// The case of two real 5-agent runs: agent 1 strips spaces (lands), agent 4 swaps `casefold()`
/// for `lower()` on the same line, gets a land conflict and settles it.
#[test]
fn keeping_the_other_agents_contribution_on_a_line_both_edit_is_quiet() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 3, &[("norm.py", NORM)]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let a1 = open(&url, "agent-1", json!({}));
    let a4 = open(&url, "agent-4", json!({}));
    write(&url, &a1, "norm.py", &with_line(STRIP));
    assert_eq!(call(&url, "land", json!({"change": a1}))["status"], "landed");
    write(&url, &a4, "norm.py", &with_line("    normed_value = normed_value.lower()\n"));
    assert_eq!(call(&url, "land", json!({"change": a4}))["status"], "conflict");

    // Dropping agent 1's `strip` warns and says what it contributed.
    let v = write(&url, &a4, "norm.py", &with_line("    normed_value = normed_value.lower()\n"));
    let u = v["undoes"].as_array().unwrap();
    assert_eq!(u.len(), 1, "{v}");
    assert_eq!((u[0]["agent"].as_str(), u[0]["lines"].clone()), (Some("agent-1"), json!([3, 3])), "{v}");
    assert_eq!(u[0]["text"], STRIP);
    assert_eq!(
        u[0]["contributed"],
        json!([{"line": 3, "words": ["strip"], "before": CASEFOLD, "after": STRIP}]),
        "{v}"
    );
    assert!(v["undoes_advice"].as_str().unwrap().contains("not giving up yours"), "{v}");
    // ... and so does putting the line back as it was before agent 1.
    let v = write(&url, &a4, "norm.py", NORM);
    assert_eq!(v["undoes_earlier"], 1, "the same entry, sent before: {v}");
    assert_eq!(call(&url, "change", json!({"change": a4}))["undoes"].as_array().unwrap().len(), 1);
    // The land holds by the same rule.
    let land = call(&url, "land", json!({"change": a4}));
    assert_eq!(land["status"], "undoes", "{land}");
    assert_eq!(land["undoes"][0]["contributed"][0]["words"], json!(["strip"]), "{land}");
    assert!(land["advice"].as_str().unwrap().contains("keeping your own edit"), "{land}");

    // Both tasks on the line: quiet, and it lands.
    let both = "    normed_value = normed_value.strip().lower()\n";
    let v = write(&url, &a4, "norm.py", &with_line(both));
    assert_eq!(v["undoes"], json!([]), "{v}");
    let land = call(&url, "land", json!({"change": a4}));
    assert_eq!(land["status"], "landed", "{land}");
    assert_eq!(land["undoes"], json!([]), "{land}");

    // A third agent: blame names agent 4, whose own part is `lower`; agent 1's `strip`, which
    // agent 4 kept, still counts for agent 1.
    let c = open(&url, "agent-5", json!({}));
    let v = write(&url, &c, "norm.py", &with_line("    normed_value = normed_value.strip().lower()[:80]\n"));
    assert_eq!(v["undoes"], json!([]), "an extension keeps both parts: {v}");
    let v = write(&url, &c, "norm.py", &with_line("    normed_value = normed_value.lower()\n"));
    let u = v["undoes"].as_array().unwrap();
    assert_eq!(u.len(), 1, "{v}");
    assert_eq!(u[0]["agent"], "agent-1", "the earlier contribution is lost: {v}");
    assert_eq!(u[0]["contributed"][0]["words"], json!(["strip"]), "{v}");
    let v = write(&url, &c, "norm.py", &with_line(STRIP));
    let u = v["undoes"].as_array().unwrap();
    assert_eq!((u.len(), u[0]["agent"].as_str()), (1, Some("agent-4")), "agent 4's lower is lost: {v}");
    assert_eq!(u[0]["contributed"][0]["words"], json!(["lower"]), "{v}");
    assert_eq!(u[0]["contributed"][0]["before"], STRIP, "{v}");
    // Removing the whole line loses both; one entry, for blame's origin.
    write(&url, &c, "norm.py", &NORM.replace(CASEFOLD, ""));
    let v = call(&url, "change", json!({"change": c}));
    let u = v["undoes"].as_array().unwrap();
    assert_eq!((u.len(), u[0]["agent"].as_str()), (1, Some("agent-4")), "{v}");
}

#[test]
fn a_land_holds_on_an_entry_after_twenty_that_do_not_hold() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 1, 60, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let c = open(&url, "c", json!({}));
    write(&url, &c, "z.py", A);
    assert_eq!(call(&url, "land", json!({"change": c}))["status"], "landed");
    // p writes 25 runs into f00.txt; s, stacked on p, deletes f00.txt (25 entries of the change it
    // is stacked on: they do not hold) and z.py (one entry of c's landed lines: it holds).
    let f = call(&url, "read", json!({"path": "f00.txt"}))["content"].as_str().unwrap().to_string();
    let mine: String = f
        .lines()
        .enumerate()
        .map(|(i, l)| if i % 2 == 0 && i < 50 { format!("p wrote {i}\n") } else { format!("{l}\n") })
        .collect();
    let p = open(&url, "p", json!({}));
    write(&url, &p, "f00.txt", &mine);
    let s = open(&url, "s", json!({"base": p}));
    call(
        &url,
        "apply",
        json!({"change": s, "edits": [{"path": "f00.txt", "delete": true}, {"path": "z.py", "delete": true}]}),
    );
    assert_eq!(call(&url, "land", json!({"change": p}))["status"], "landed");
    let ch = call(&url, "change", json!({"change": s}));
    assert_eq!((ch["undoes"].as_array().unwrap().len(), ch["undoes_more"].clone()), (20, json!(6)), "{ch}");
    let land = call(&url, "land", json!({"change": s}));
    assert_eq!(land["status"], "undoes", "the 26th entry holds: {land}");
    let u = land["undoes"].as_array().unwrap();
    assert_eq!((u.len(), u[0]["path"].as_str(), u[0]["agent"].as_str()), (1, Some("z.py"), Some("c")), "{land}");
}

#[test]
fn queue_mode_a_second_land_of_a_queued_yes_is_not_held() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 2, 10, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    std::fs::write(data.join("treeyard.toml"), "[landing]\nmode = \"queue\"\ntest = \"true\"\n").unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let a = open(&url, "a", json!({}));
    write(&url, &a, "m.py", A);
    call(&url, "land", json!({"change": a}));
    call(&url, "queue_run", json!({}));
    let b = open(&url, "b", json!({}));
    write(&url, &b, "m.py", &A.replace("    value = value.lower()\n", ""));
    let q = call(&url, "land", json!({"change": b, "undo": true, "reason": "the task drops lower()"}));
    assert_eq!(q["status"], "queued", "{q}");
    let again = call(&url, "land", json!({"change": b}));
    assert_eq!(again["status"], "queued", "the yes for this head stands: {again}");
    assert_eq!(call(&url, "queue", json!({}))["queue"].as_array().unwrap().len(), 1, "queued once");
    assert_eq!(call(&url, "events", json!({"kinds": ["undoes"]}))["events"], json!([]));
    assert_eq!(call(&url, "queue_run", json!({}))["landed"][0]["change"], b.as_str());
}
