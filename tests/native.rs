//! The native tool set of the stdio bridge (`treeyard mcp --tools native`): the built binary,
//! driven with JSON-RPC on stdio against a real server, as a harness drives it. Agents edit their
//! `start` directory with plain file writes (their own tools), then `sync` and `land`.

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

const LIB: &str = "one\ntwo\nthree\nfour\nfive\n";

fn repo(root: &Path) -> PathBuf {
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("lib.txt"), LIB).unwrap();
    std::fs::write(src.join("old.txt"), "to delete\n").unwrap();
    std::fs::write(src.join(".gitignore"), "build/\n").unwrap();
    std::fs::write(src.join("run_tests.sh"), "#!/bin/sh\ngrep -q '^TWO$' lib.txt\n").unwrap();
    std::fs::set_permissions(src.join("run_tests.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
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
    fn start(url: &str, agent: &str, cwd: &Path, tmp: &Path) -> Bridge {
        let mut child = Command::new(bin())
            .args(["mcp", "--url", url, "--agent", agent, "--tools", "native"])
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

struct World {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    url: String,
    tmpdir: PathBuf,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let src = repo(tmp.path());
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let tmpdir = tmp.path().join("tmpdir");
    std::fs::create_dir_all(&tmpdir).unwrap();
    World { _tmp: tmp, src, url, tmpdir }
}

impl World {
    fn bridge(&self, agent: &str) -> Bridge {
        Bridge::start(&self.url, agent, &self.src, &self.tmpdir)
    }

    fn main_file(&self, path: &str) -> Option<String> {
        let r = treeyard::client::call(&self.url, "read", json!({"path": path})).ok()?;
        r["content"].as_str().map(str::to_string)
    }
}

/// `start` and the change id and directory it returns.
fn start(b: &mut Bridge, intent: &str) -> (String, PathBuf) {
    let v = b.tool("start", json!({"intent": intent, "paths": ["lib.txt"]}));
    (v["id"].as_str().unwrap().to_string(), PathBuf::from(v["dir"].as_str().unwrap()))
}

fn edit(dir: &Path, path: &str, from: &str, to: &str) {
    let p = dir.join(path);
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains(from), "{path}: {text}");
    std::fs::write(&p, text.replacen(from, to, 1)).unwrap();
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect()).unwrap_or_default()
}

#[test]
fn start_edit_with_own_tools_sync_and_land() {
    let w = world();
    let mut b = w.bridge("agent-a");
    let init = b.rpc("initialize", json!({}));
    assert!(init["result"]["instructions"].as_str().unwrap().contains("start"));
    let list = b.rpc("tools/list", json!({}));
    let names: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["start", "sync", "land", "change", "diff"]);

    let (id, dir) = start(&mut b, "TWO in capitals");
    // A complete tree the tests run in; the manifest sits beside it, not in it.
    for f in ["lib.txt", "old.txt", ".gitignore", "run_tests.sh"] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    assert!(!dir.join(".treeyard").exists(), "nothing of TreeYard inside the tree");
    assert!(dir.with_file_name(format!("{id}.native.json")).is_file(), "manifest beside the tree");
    assert!(!Command::new("sh").arg("-c").arg("./run_tests.sh").current_dir(&dir).status().unwrap().success());

    // The agent's own tools: edit, add, delete; build output and caches stay local.
    edit(&dir, "lib.txt", "two\n", "TWO\n");
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
    std::fs::remove_file(dir.join("old.txt")).unwrap();
    for f in ["build/out.o", "__pycache__/m.cpython-312.pyc", "notes.pyc", "pkg.egg-info/PKG-INFO"] {
        std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
        std::fs::write(dir.join(f), "x\n").unwrap();
    }
    assert!(Command::new("sh").arg("-c").arg("./run_tests.sh").current_dir(&dir).status().unwrap().success());
    let s = b.tool("sync", json!({"change": id}));
    assert_eq!(strs(&s["sent"]), ["lib.txt", "new.txt"], "{s}");
    assert_eq!(strs(&s["deleted"]), ["old.txt"], "{s}");
    let diff = treeyard::client::call(&w.url, "diff", json!({"change": id})).unwrap();
    let paths: Vec<&str> = diff["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["lib.txt", "new.txt", "old.txt"]);
    // Nothing new: nothing sent.
    let s = b.tool("sync", json!({"change": id}));
    assert!(s.get("sent").is_none() && s.get("deleted").is_none(), "{s}");

    // A new bridge process (a harness restart) finds the checkout by its manifest.
    b.close();
    let mut b = w.bridge("agent-a");
    std::fs::write(dir.join("new.txt"), "newer\n").unwrap();
    let s = b.tool("sync", json!({"change": id}));
    assert_eq!(strs(&s["sent"]), ["new.txt"], "{s}");

    let l = b.tool("land", json!({"change": id}));
    assert_eq!(l["status"], "landed", "{l}");
    assert_eq!(w.main_file("lib.txt").unwrap(), "one\nTWO\nthree\nfour\nfive\n");
    assert_eq!(w.main_file("new.txt").unwrap(), "newer\n");
    assert!(w.main_file("old.txt").is_none());
    for f in ["build/out.o", "notes.pyc"] {
        assert!(w.main_file(f).is_none(), "{f} never lands");
        assert!(dir.join(f).exists(), "{f} stays in dir");
    }
    // The original directory never changed.
    assert_eq!(std::fs::read_to_string(w.src.join("lib.txt")).unwrap(), LIB);
    let (err, text) = b.raw("sync", json!({"change": id}));
    assert!(err && text.contains("landed"), "{text}");
    let (err, text) = b.raw("sync", json!({"change": "ch_000000000000"}));
    assert!(err && text.contains("call start"), "{text}");
    b.close();
}

#[test]
fn sync_warns_undoes_and_land_holds_until_the_line_is_back() {
    let w = world();
    let mut a = w.bridge("agent-a");
    let (ca, da) = start(&mut a, "three by a");
    edit(&da, "lib.txt", "three\n", "three by a\n");
    assert_eq!(a.tool("land", json!({"change": ca}))["status"], "landed");

    // B starts after A landed, then rewrites A's line away with its own tools.
    let mut b = w.bridge("agent-b");
    let (cb, db) = start(&mut b, "rewrite line three");
    assert!(std::fs::read_to_string(db.join("lib.txt")).unwrap().contains("three by a\n"));
    edit(&db, "lib.txt", "three by a\n", "three\n");
    let s = b.tool("sync", json!({"change": cb}));
    let undoes = s["undoes"].as_array().unwrap_or_else(|| panic!("no undoes: {s}"));
    assert_eq!(undoes[0]["path"], "lib.txt");
    assert_eq!(undoes[0]["agent"], "agent-a");
    assert_eq!(undoes[0]["change"], ca.as_str());
    let l = b.tool("land", json!({"change": cb}));
    assert_eq!(l["status"], "undoes", "{l}");
    assert!(w.main_file("lib.txt").unwrap().contains("three by a\n"), "held: main keeps A's line");
    // Its part back and B's own edit of the same line: lands.
    edit(&db, "lib.txt", "three\n", "three by a, checked by b\n");
    let l = b.tool("land", json!({"change": cb}));
    assert_eq!(l["status"], "landed", "{l}");
    assert!(w.main_file("lib.txt").unwrap().contains("three by a, checked by b\n"));
    a.close();
    b.close();
}

#[test]
fn a_land_conflict_becomes_markers_in_dir_and_lands_once_fixed() {
    let w = world();
    let mut a = w.bridge("agent-a");
    let mut b = w.bridge("agent-b");
    let (ca, da) = start(&mut a, "two by a");
    let (cb, db) = start(&mut b, "two by b");
    edit(&da, "lib.txt", "two\n", "two by a\n");
    assert_eq!(a.tool("land", json!({"change": ca}))["status"], "landed");
    edit(&db, "lib.txt", "two\n", "two by b\n");
    let l = b.tool("land", json!({"change": cb}));
    assert_eq!(l["status"], "conflict", "{l}");
    assert_eq!(strs(&l["conflicted"]), ["lib.txt"]);
    assert!(l["advice"].as_str().unwrap().contains("<<<<<<< main"));
    let text = std::fs::read_to_string(db.join("lib.txt")).unwrap();
    assert!(text.contains("<<<<<<< main\ntwo by a\n=======\ntwo by b\n>>>>>>> change\n"), "{text}");

    // Markers left: not sent, land refused; other files still go in.
    std::fs::write(db.join("notes.txt"), "b's notes\n").unwrap();
    let l = b.tool("land", json!({"change": cb}));
    assert_eq!(l["status"], "refused", "{l}");
    assert_eq!(strs(&l["markers_left"]), ["lib.txt"]);
    assert_eq!(strs(&l["sent"]), ["notes.txt"]);
    // A half-done fix stays as the agent left it.
    edit(&db, "lib.txt", "one\n", "ONE\n");
    let s = b.tool("sync", json!({"change": cb}));
    assert_eq!(strs(&s["markers_left"]), ["lib.txt"], "{s}");
    assert!(std::fs::read_to_string(db.join("lib.txt")).unwrap().starts_with("ONE\n<<<<<<< main"));
    assert!(w.main_file("notes.txt").is_none(), "nothing landed");

    std::fs::write(db.join("lib.txt"), "ONE\ntwo by a and b\nthree\nfour\nfive\n").unwrap();
    let l = b.tool("land", json!({"change": cb}));
    assert_eq!(l["status"], "landed", "{l}");
    assert_eq!(w.main_file("lib.txt").unwrap(), "ONE\ntwo by a and b\nthree\nfour\nfive\n");
    assert_eq!(w.main_file("notes.txt").unwrap(), "b's notes\n");
    a.close();
    b.close();
}

#[test]
fn main_moved_shows_in_sync_and_a_rebase_refreshes_dir() {
    let w = world();
    let mut a = w.bridge("agent-a");
    let mut b = w.bridge("agent-b");
    let (ca, da) = start(&mut a, "line one");
    let (cb, db) = start(&mut b, "line five");
    edit(&da, "lib.txt", "one\n", "one by a\n");
    assert_eq!(a.tool("land", json!({"change": ca}))["status"], "landed");
    edit(&db, "lib.txt", "five\n", "five by b\n");
    let s = b.tool("sync", json!({"change": cb}));
    assert_eq!(strs(&s["main_moved_in"]), ["lib.txt"], "{s}");
    assert!(s["main_moved_advice"].as_str().unwrap().contains("rebase"));
    assert!(!std::fs::read_to_string(db.join("lib.txt")).unwrap().contains("one by a"), "dir stays at its base");
    let s = b.tool("sync", json!({"change": cb, "rebase": true}));
    assert!(s.get("main_moved_in").is_none(), "{s}");
    assert_eq!(std::fs::read_to_string(db.join("lib.txt")).unwrap(), "one by a\ntwo\nthree\nfour\nfive by b\n");
    // An edit after the rebase goes on top of the rebased file, not over it.
    edit(&db, "lib.txt", "four\n", "four by b\n");
    assert_eq!(b.tool("land", json!({"change": cb}))["status"], "landed");
    assert_eq!(w.main_file("lib.txt").unwrap(), "one by a\ntwo\nthree\nfour by b\nfive by b\n");
    a.close();
    b.close();
}

#[test]
fn a_stale_directory_merges_instead_of_overwriting() {
    // The change moves on the server (a rebase through another client) while dir still holds the
    // old base: the whole-file write merges 3-way against what dir was written from.
    let w = world();
    let mut a = w.bridge("agent-a");
    let mut b = w.bridge("agent-b");
    let (ca, da) = start(&mut a, "line one");
    let (cb, db) = start(&mut b, "line five");
    edit(&da, "lib.txt", "one\n", "one by a\n");
    assert_eq!(a.tool("land", json!({"change": ca}))["status"], "landed");
    edit(&db, "lib.txt", "five\n", "five by b\n");
    b.tool("sync", json!({"change": cb}));
    // Rebased behind the bridge's back: dir does not have A's line.
    treeyard::client::call(&w.url, "rebase", json!({"change": cb})).unwrap();
    edit(&db, "lib.txt", "four\n", "four by b\n");
    let s = b.tool("sync", json!({"change": cb}));
    assert!(s.get("undoes").is_none(), "no undoes: the merge keeps A's line: {s}");
    assert_eq!(std::fs::read_to_string(db.join("lib.txt")).unwrap(), "one by a\ntwo\nthree\nfour by b\nfive by b\n");
    assert_eq!(b.tool("land", json!({"change": cb}))["status"], "landed");
    assert_eq!(w.main_file("lib.txt").unwrap(), "one by a\ntwo\nthree\nfour by b\nfive by b\n");
    a.close();
    b.close();
}

#[test]
fn a_refresh_never_overwrites_an_edit_made_after_the_scan() {
    // The refresh after sync or land rewrites dir to the change's head; an edit that reached the
    // disk after the sync's scan (a background process, a race) stays for the next sync.
    let w = world();
    let mut b = w.bridge("agent-a");
    let (id, dir) = start(&mut b, "late edit");
    edit(&dir, "lib.txt", "two\n", "TWO\n");
    b.tool("sync", json!({"change": id}));
    edit(&dir, "lib.txt", "four\n", "four late\n");
    let p = treeyard::native::place(&id).unwrap();
    assert!(p.keep_edits);
    // `place` reads TMPDIR: point it at the bridge's.
    let p = treeyard::workspace::Place {
        dir: dir.clone(),
        mark: dir.with_file_name(format!("{id}.native.json")),
        lock: dir.with_file_name(format!("{id}.native.lock")),
        keep_edits: true,
    };
    let _l = p.lock().unwrap();
    treeyard::workspace::checkout_locked(&w.url, &id, &p, &Default::default()).unwrap();
    drop(_l);
    assert!(std::fs::read_to_string(dir.join("lib.txt")).unwrap().contains("four late\n"), "kept");
    let s = b.tool("sync", json!({"change": id}));
    assert_eq!(strs(&s["sent"]), ["lib.txt"], "{s}");
    assert_eq!(b.tool("land", json!({"change": id}))["status"], "landed");
    assert_eq!(w.main_file("lib.txt").unwrap(), "one\nTWO\nthree\nfour late\nfive\n");
    b.close();
}

#[test]
fn checkout_directories_are_private() {
    let w = world();
    let mut b = w.bridge("agent-a");
    let (_, dir) = start(&mut b, "private");
    for d in [&dir, &dir.parent().unwrap().to_path_buf()] {
        assert_eq!(std::fs::metadata(d).unwrap().permissions().mode() & 0o777, 0o700, "{}", d.display());
    }
    b.close();
    // A checkout root that is a symlink is refused.
    let t2 = w.tmpdir.join("t2");
    std::fs::create_dir_all(&t2).unwrap();
    let uid = unsafe { libc::getuid() };
    std::os::unix::fs::symlink(dir.parent().unwrap(), t2.join(format!("treeyard-checkouts-{uid}"))).unwrap();
    let mut b = Bridge::start(&w.url, "agent-b", &w.src, &t2);
    let (err, text) = b.raw("start", json!({"intent": "through a link"}));
    assert!(err && text.contains("symlink"), "{text}");
    b.close();
}

#[test]
fn a_delete_against_an_edit_waits_for_a_decision() {
    for choice in ["main", "mine"] {
        let w = world();
        let mut a = w.bridge("agent-a");
        let mut b = w.bridge("agent-b");
        let (ca, da) = start(&mut a, "delete old");
        let (cb, db) = start(&mut b, "edit old");
        std::fs::remove_file(da.join("old.txt")).unwrap();
        std::fs::write(db.join("old.txt"), "to delete\nedited by b\n").unwrap();
        assert_eq!(a.tool("land", json!({"change": ca}))["status"], "landed");
        let l = b.tool("land", json!({"change": cb}));
        assert_eq!(l["status"], "conflict", "{l}");
        assert_eq!(l["decide"][0]["path"], "old.txt", "{l}");
        assert_eq!(l["decide"][0]["main"], "deleted", "{l}");
        assert!(l.get("conflicted").is_none(), "no markers to edit: {l}");
        // Not settled silently: land again without a decision is refused.
        let l = b.tool("land", json!({"change": cb}));
        assert_eq!(l["status"], "refused", "{l}");
        assert_eq!(l["decide"][0]["kind"], "modify_delete", "{l}");
        let l = b.tool("land", json!({"change": cb, "resolve": {"old.txt": choice}}));
        // Taking main's side leaves B's change with nothing of its own.
        let want = if choice == "main" { "already_on_main" } else { "landed" };
        assert_eq!(l["status"], want, "{l}");
        if choice == "main" {
            assert!(w.main_file("old.txt").is_none(), "main's delete kept");
            assert!(!db.join("old.txt").exists(), "dir shows main's side");
        } else {
            assert_eq!(w.main_file("old.txt").unwrap(), "to delete\nedited by b\n", "B's file kept");
        }
        a.close();
        b.close();
    }
}

#[test]
fn a_stale_directory_never_sends_a_conflicted_file() {
    // The rewrite after a conflict did not happen (here: a rebase through another client): dir still
    // holds the pre-conflict file. Its later edit must not become the resolution, and marker text
    // must never land.
    let w = world();
    let mut a = w.bridge("agent-a");
    let mut b = w.bridge("agent-b");
    let (ca, da) = start(&mut a, "two by a");
    let (cb, db) = start(&mut b, "two by b");
    edit(&da, "lib.txt", "two\n", "two by a\n");
    assert_eq!(a.tool("land", json!({"change": ca}))["status"], "landed");
    edit(&db, "lib.txt", "two\n", "two by b\n");
    b.tool("sync", json!({"change": cb}));
    let r = treeyard::client::call(&w.url, "rebase", json!({"change": cb})).unwrap();
    assert_eq!(r["conflicted"], true, "{r}");
    edit(&db, "lib.txt", "five\n", "five by b\n");
    let l = b.tool("land", json!({"change": cb}));
    assert_eq!(l["status"], "refused", "{l}");
    assert_eq!(strs(&l["stale_conflicts"]), ["lib.txt"], "{l}");
    let copy = PathBuf::from(l["stale_copies"].as_str().unwrap()).join("lib.txt");
    assert!(std::fs::read_to_string(&copy).unwrap().contains("five by b"), "the agent's copy is kept");
    let now = std::fs::read_to_string(db.join("lib.txt")).unwrap();
    assert!(now.contains("<<<<<<< main\ntwo by a\n=======\ntwo by b\n>>>>>>> change\n"), "{now}");
    assert_eq!(w.main_file("lib.txt").unwrap(), "one\ntwo by a\nthree\nfour\nfive\n");
    std::fs::write(db.join("lib.txt"), "one\ntwo by a and b\nthree\nfour\nfive by b\n").unwrap();
    assert_eq!(b.tool("land", json!({"change": cb}))["status"], "landed");
    assert_eq!(w.main_file("lib.txt").unwrap(), "one\ntwo by a and b\nthree\nfour\nfive by b\n");
    a.close();
    b.close();
}

#[test]
fn the_server_checks_markers_after_the_merge_of_a_stale_write() {
    // A whole-file write with an old `seen` on a conflicted path: the merge with the view brings the
    // markers back, so it is refused, not recorded as the resolution.
    let w = world();
    let c = |op: &str, p: Value| treeyard::client::call(&w.url, op, p);
    let a = c("open_change", json!({"agent": "a", "intent": "a"})).unwrap();
    let b = c("open_change", json!({"agent": "b", "intent": "b"})).unwrap();
    let (ia, ib) = (a["id"].as_str().unwrap(), b["id"].as_str().unwrap());
    c("apply", json!({"change": ia, "edits": [{"path": "lib.txt", "replace": {"old": "two\n", "new": "two a\n"}}]}))
        .unwrap();
    assert_eq!(c("land", json!({"change": ia})).unwrap()["status"], "landed");
    let applied = c(
        "apply",
        json!({"change": ib, "edits": [{"path": "lib.txt", "replace": {"old": "two\n", "new": "two b\n"}}]}),
    )
    .unwrap();
    let seen = applied["head"].as_str().unwrap().to_string();
    assert_eq!(c("rebase", json!({"change": ib})).unwrap()["conflicted"], true);
    let err = c(
        "apply",
        json!({"change": ib, "seen": seen, "edits": [{"path": "lib.txt", "content": "one\ntwo b\nthree\nfour\nfive b\n"}]}),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("marker"), "{err:#}");
}

#[test]
fn backups_are_skipped_and_new_files_listed() {
    let w = world();
    let mut b = w.bridge("agent-a");
    let (id, dir) = start(&mut b, "backups");
    edit(&dir, "lib.txt", "two\n", "TWO\n");
    for f in ["lib.txt-e", "x.orig", "x.rej", "notes.txt~", "a.bak", ".lib.txt.swp"] {
        std::fs::write(dir.join(f), "junk\n").unwrap();
    }
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
    std::fs::write(dir.join("solo-e"), "a real file named like this\n").unwrap();
    let s = b.tool("sync", json!({"change": id}));
    assert_eq!(strs(&s["added"]), ["new.txt", "solo-e"], "{s}");
    assert_eq!(strs(&s["skipped"]), [".lib.txt.swp", "a.bak", "lib.txt-e", "notes.txt~", "x.orig", "x.rej"], "{s}");
    assert_eq!(b.tool("land", json!({"change": id}))["status"], "landed");
    assert!(w.main_file("lib.txt-e").is_none());
    b.close();
}

#[test]
fn edits_after_a_landed_change_go_into_a_follow_up() {
    let w = world();
    let mut b = w.bridge("agent-a");
    let (id, dir) = start(&mut b, "first");
    edit(&dir, "lib.txt", "two\n", "TWO\n");
    assert_eq!(b.tool("land", json!({"change": id}))["status"], "landed");
    // Meanwhile another agent lands elsewhere in the file.
    let mut o = w.bridge("agent-o");
    let (co, d_o) = start(&mut o, "other");
    edit(&d_o, "lib.txt", "five\n", "five by o\n");
    assert_eq!(o.tool("land", json!({"change": co}))["status"], "landed");
    edit(&dir, "lib.txt", "one\n", "one, later\n");
    let s = b.tool("sync", json!({"change": id}));
    let next = s["follow_up"]["id"].as_str().unwrap_or_else(|| panic!("{s}")).to_string();
    assert_ne!(next, id);
    assert!(s["follow_up"]["note"].as_str().unwrap().contains(&next));
    assert_eq!(strs(&s["sent"]), ["lib.txt"], "{s}");
    assert!(s.get("undoes").is_none(), "the 3-way merge keeps o's line: {s}");
    // The old id still works and goes to the follow-up.
    let l = b.tool("land", json!({"change": id}));
    assert_eq!((l["status"].as_str(), l["follow_up"]["id"].as_str()), (Some("landed"), Some(next.as_str())), "{l}");
    assert_eq!(w.main_file("lib.txt").unwrap(), "one, later\nTWO\nthree\nfour\nfive by o\n");
    // Nothing left to send: the plain answer.
    let (err, text) = b.raw("sync", json!({"change": next}));
    assert!(err && text.contains("no unsent edits"), "{text}");
    b.close();
    o.close();
}

#[test]
fn the_override_rule_is_one_rule() {
    let rule = "names those lines for removal";
    assert!(treeyard::connect::NATIVE_INSTRUCTIONS.contains(rule));
    assert!(treeyard::native::HOLD_ADVICE.contains(rule));
    let land = treeyard::native::tools().into_iter().find(|t| t["name"] == "land").unwrap();
    assert!(land["description"].as_str().unwrap().contains(rule));
    assert!(!treeyard::connect::NATIVE_INSTRUCTIONS.contains("a person decides"));
}

#[test]
fn an_idle_change_is_not_in_start_advice() {
    // The kit's pre-created workspace change per agent: open, nothing declared, nothing edited.
    let w = world();
    treeyard::client::call(&w.url, "open_change", json!({"agent": "idle", "intent": "workspace agent-1"})).unwrap();
    let mut b = w.bridge("agent-a");
    let v = b.tool("start", json!({"intent": "x", "paths": ["lib.txt"]}));
    assert!(v.get("overlaps").is_none() && v.get("advice").is_none(), "{v}");
    b.close();
}
