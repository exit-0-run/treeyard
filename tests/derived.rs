//! Phase 4 derived data through the HTTP API: blame equals `git blame --first-parent`, the
//! changed-path index equals the scan and `git log`, indexed grep equals the scan, symbol
//! lookup, and bisect finds a planted bad change, which revert then removes.

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use treeyard::repo::Repo;

fn write(dir: &Path, path: &str, body: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

const RUST: &str = "use std::fmt;\n\npub struct Point {\n    pub x: i32,\n}\n\nimpl Point {\n    pub fn norm(&self) -> i32 {\n        self.x.abs()\n    }\n}\n\npub fn main() {\n    println!(\"hi\");\n}\n";
const PY: &str =
    "class Repo:\n    def land(self, change):\n        return change\n\n\ndef open_repo(path):\n    return Repo()\n";
const GO: &str = "package server\n\ntype Server struct{}\n\nfunc (s *Server) Serve() error {\n\treturn nil\n}\n\nfunc New() *Server {\n\treturn &Server{}\n}\n";
const TS: &str = "export interface Change {\n  id: string;\n}\n\nexport function land(c: Change): string {\n  return c.id;\n}\n\nexport class Queue {\n  push(c: Change): void {}\n}\n";

/// A history with edits, an insert the indent heuristic slides, a rename with an edit, a
/// delete and re-add, a side branch merged with --no-ff, and a mode change.
fn history_repo(root: &Path) -> std::path::PathBuf {
    let src =
        fixture_repo(root, 3, 30, &[("src/lib.rs", RUST), ("app.py", PY), ("server/server.go", GO), ("ui/app.ts", TS)]);
    let edit = |path: &str, from: &str, to: &str| {
        let p = src.join(path);
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains(from), "{path} lacks {from:?}");
        std::fs::write(&p, t.replacen(from, to, 1)).unwrap();
    };
    edit("f00.txt", "file 0 line 3\n", "file 0 line 3 changed\nan inserted line\n");
    edit("src/lib.rs", "pub fn main() {", "pub fn helper() -> i32 {\n    42\n}\n\npub fn main() {");
    commit(&src, "edit and insert");
    std::fs::create_dir_all(src.join("moved")).unwrap();
    git(&src, &["mv", "f01.txt", "moved/f01.txt"]);
    edit("moved/f01.txt", "file 1 line 10\n", "file 1 line ten\n");
    commit(&src, "rename f01 with an edit");
    git(&src, &["rm", "-q", "f02.txt"]);
    commit(&src, "delete f02");
    let body: String =
        (0..30).map(|j| if j % 3 == 0 { format!("new {j}\n") } else { format!("file 2 line {j}\n") }).collect();
    write(&src, "f02.txt", &body);
    commit(&src, "re-add f02");
    git(&src, &["checkout", "-q", "-b", "side"]);
    edit("app.py", "        return change\n", "        print(change)\n        return change\n");
    commit(&src, "side: log in land");
    edit("f00.txt", "file 0 line 20\n", "file 0 line 20 from side\n");
    commit(&src, "side: f00");
    git(&src, &["checkout", "-q", "main"]);
    edit("f00.txt", "file 0 line 1\n", "file 0 line one\n");
    commit(&src, "main: f00 line 1");
    git(&src, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    git(&src, &["update-index", "--chmod=+x", "f00.txt"]);
    git(&src, &["commit", "-q", "-m", "make f00 executable"]);
    edit("ui/app.ts", "  return c.id;\n", "  const id = c.id;\n  return id;\n");
    commit(&src, "ts: name the id");
    src
}

/// (commit, original line) per line from `git blame --first-parent --porcelain`.
fn git_blame(dir: &Path, git_dir: Option<&Path>, rev: &str, path: &str) -> Vec<(String, u64)> {
    let mut args: Vec<String> = Vec::new();
    if let Some(g) = git_dir {
        args.extend(["--git-dir".into(), g.to_string_lossy().into_owned()]);
    }
    args.extend(["blame", "--first-parent", "--porcelain", rev, "--", path].map(String::from));
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = git(dir, &a);
    out.lines()
        .filter_map(|l| {
            let mut p = l.split(' ');
            let (c, o) = (p.next()?, p.next()?);
            (c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (c.to_string(), o.parse().unwrap()))
        })
        .collect()
}

fn rs_blame(url: &str, path: &str, at: Option<&str>) -> Vec<(String, u64)> {
    let mut p = json!({"path": path});
    if let Some(a) = at {
        p["at"] = json!(a);
    }
    let v = call(url, "blame", p);
    let mut out = Vec::new();
    for h in v["hunks"].as_array().unwrap() {
        let commit = v["origins"][h["origin"].as_str().unwrap()]["commit"].as_str().unwrap().to_string();
        for k in 0..h["count"].as_u64().unwrap() {
            out.push((commit.clone(), h["orig_start"].as_u64().unwrap() + k));
        }
    }
    out
}

fn files(dir: &Path) -> Vec<String> {
    git(dir, &["ls-files"]).lines().map(str::to_string).collect()
}

#[test]
fn blame_equals_git_blame_on_imported_history_and_after_agents_land() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history_repo(tmp.path());
    let data = tmp.path().join("data");
    Repo::init_with_history(&data, &src, "main", None, 50).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    for f in files(&src) {
        assert_eq!(rs_blame(&url, &f, None), git_blame(&src, None, "HEAD", &f), "blame of {f} at the import");
    }
    // The renamed file reaches back past the rename to the first commit.
    let moved = call(&url, "blame", json!({"path": "moved/f01.txt"}));
    assert!(moved["origins"].as_object().unwrap().len() >= 2, "{moved}");

    // Agents land three changes; blame must follow them like git does on the export repo.
    for (i, (path, from, to)) in [
        ("f00.txt", "file 0 line 5", "agent edit 5"),
        ("src/lib.rs", "    42\n", "    41 + 1\n"),
        ("f00.txt", "file 0 line 25", "agent edit 25\nand one more"),
    ]
    .into_iter()
    .enumerate()
    {
        let ch = call(
            &url,
            "open_change",
            json!({"agent": format!("agent-{i}"), "intent": format!("edit {path}"), "model": "sim-1", "session": "s-1"}),
        );
        let id = ch["id"].as_str().unwrap();
        call(&url, "apply", json!({"change": id, "edits": [{"path": path, "replace": {"old": from, "new": to}}]}));
        assert_eq!(call(&url, "land", json!({"change": id}))["status"], "landed");
    }
    let gd = data.join("git");
    for f in files(&src) {
        assert_eq!(rs_blame(&url, &f, None), git_blame(tmp.path(), Some(&gd), "main", &f), "blame of {f} after agents");
    }
    let text = call(&url, "read", json!({"path": "f00.txt"}))["content"].as_str().unwrap().to_string();
    let line = 1 + text.lines().position(|l| l == "agent edit 5").unwrap() as u64;
    let v = call(&url, "blame", json!({"path": "f00.txt", "start": line, "end": line}));
    assert_eq!(v["hunks"].as_array().unwrap().len(), 1, "the range keeps one hunk: {v}");
    let agent_line = &v["hunks"][0];
    let origin = &v["origins"][agent_line["origin"].as_str().unwrap()];
    assert_eq!(
        (origin["agent"].as_str(), origin["model"].as_str(), origin["session"].as_str()),
        (Some("agent-0"), Some("sim-1"), Some("s-1"))
    );
    assert_eq!(origin["intent"], "edit f00.txt");

    // An open change (not on main): blame at its head equals git blame of its exported commit.
    let ch = call(&url, "open_change", json!({"agent": "agent-x", "intent": "pending"}));
    let id = ch["id"].as_str().unwrap();
    call(
        &url,
        "apply",
        json!({"change": id, "edits": [{"path": "app.py", "replace": {"old": "def open_repo", "new": "def open_repository"}}]}),
    );
    let exported = call(&url, "export_change", json!({"change": id}))["commit"].as_str().unwrap().to_string();
    assert_eq!(rs_blame(&url, "app.py", Some(id)), git_blame(tmp.path(), Some(&gd), &exported, "app.py"));
}

#[test]
fn log_index_equals_scan_and_git_log() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history_repo(tmp.path());
    let data = tmp.path().join("data");
    Repo::init_with_history(&data, &src, "main", None, 50).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let commits = |v: &Value| -> Vec<String> {
        v["entries"].as_array().unwrap().iter().map(|e| e["commit"].as_str().unwrap().to_string()).collect()
    };
    for path in ["f00.txt", "moved", "src", "app.py", "f02.txt", "server/server.go"] {
        let idx = call(&url, "log", json!({"path": path, "limit": 100}));
        let scan = call(&url, "log", json!({"path": path, "limit": 100, "scan": true}));
        assert_eq!(commits(&idx), commits(&scan), "{path}");
        let want: Vec<String> =
            git(&src, &["log", "--first-parent", "--format=%H", "--", path]).lines().map(str::to_string).collect();
        assert_eq!(commits(&idx), want, "{path} vs git log --first-parent");
        assert_eq!(idx["index"], true);
    }
    // Changed-path globs and the agent filter.
    let rs = call(&url, "log", json!({"paths": ["*.rs", "*.go"], "limit": 100}));
    let want: Vec<String> = git(&src, &["log", "--first-parent", "--format=%H", "--", "*.rs", "*.go"])
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(commits(&rs), want);
    let ch = call(&url, "open_change", json!({"agent": "bot", "intent": "touch go"}));
    let id = ch["id"].as_str().unwrap();
    call(
        &url,
        "apply",
        json!({"change": id, "edits": [{"path": "server/server.go", "replace": {"old": "return nil", "new": "return nil // ok"}}]}),
    );
    call(&url, "land", json!({"change": id}));
    let bot = call(&url, "log", json!({"agent": "bot"}));
    assert_eq!(bot["entries"].as_array().unwrap().len(), 1);
    assert_eq!(bot["entries"][0]["paths"], json!(["server/server.go"]));
    let all = call(&url, "log", json!({"limit": 1000}));
    let scan = call(&url, "log", json!({"limit": 1000, "scan": true}));
    assert_eq!(commits(&all), commits(&scan));
}

#[test]
fn indexed_grep_equals_the_scan_on_main_and_on_a_change() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history_repo(tmp.path());
    let data = tmp.path().join("data");
    Repo::init_with_history(&data, &src, "main", None, 50).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let ch = call(&url, "open_change", json!({"agent": "a", "intent": "add a needle"}));
    let id = ch["id"].as_str().unwrap().to_string();
    call(
        &url,
        "apply",
        json!({"change": id, "edits": [
            {"path": "notes/needle.txt", "content": "a unique needle_xyz here\nfn main_two\n"},
            {"path": "f00.txt", "delete": true},
        ]}),
    );
    let patterns = [
        "line 1",
        "needle_xyz",
        r"fn\s+\w+",
        "(?i)SERVER",
        "file [0-9] line 2[0-9]",
        "return|print",
        "^class ",
        r"\bid\b",
        "x{0}",
        "Queue|Repo|Point",
        "line (one|ten)",
    ];
    for at in [None, Some(id.as_str())] {
        for pat in patterns {
            let mut p = json!({"pattern": pat, "max": 1000});
            if let Some(a) = at {
                p["at"] = json!(a);
            }
            let idx = call(&url, "grep", p.clone());
            p["index"] = json!(false);
            let scan = call(&url, "grep", p);
            assert_eq!(idx["matches"], scan["matches"], "pattern {pat} at {at:?}");
            assert_eq!(idx["index"]["used"], true);
        }
    }
    let sel = call(&url, "grep", json!({"pattern": "needle_xyz", "at": id}));
    assert_eq!(sel["matches"].as_array().unwrap().len(), 1);
    let main_only = call(&url, "grep", json!({"pattern": "file 0 line 1"}));
    assert!(main_only["index"]["candidates"].as_u64().unwrap() < main_only["index"]["files"].as_u64().unwrap());
    // max and truncation behave like the scan.
    let a = call(&url, "grep", json!({"pattern": "line", "max": 3}));
    let b = call(&url, "grep", json!({"pattern": "line", "max": 3, "index": false}));
    assert_eq!((a["matches"].clone(), a["truncated"].clone()), (b["matches"].clone(), b["truncated"].clone()));
}

#[test]
fn symbols_are_found_at_main_and_in_an_open_change() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history_repo(tmp.path());
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let find = |p: Value| -> Vec<(String, String, String, u64)> {
        call(&url, "symbols", p)["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap().into(),
                    s["kind"].as_str().unwrap().into(),
                    s["path"].as_str().unwrap().into(),
                    s["line"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(find(json!({"name": "Point"})), vec![("Point".into(), "class".into(), "src/lib.rs".into(), 3)]);
    assert_eq!(find(json!({"name": "norm"})), vec![("norm".into(), "method".into(), "src/lib.rs".into(), 8)]);
    assert_eq!(find(json!({"name": "Serve"})), vec![("Serve".into(), "method".into(), "server/server.go".into(), 5)]);
    assert_eq!(find(json!({"name": "Repo"})), vec![("Repo".into(), "class".into(), "app.py".into(), 1)]);
    assert_eq!(find(json!({"name": "Change"})), vec![("Change".into(), "interface".into(), "ui/app.ts".into(), 1)]);
    let lands = find(json!({"name": "land"}));
    assert_eq!(lands.iter().map(|s| s.2.as_str()).collect::<Vec<_>>(), vec!["app.py", "ui/app.ts"]);
    assert_eq!(find(json!({"name": "land", "path": "ui"})).len(), 1);
    assert_eq!(find(json!({"name": "land", "kind": "function"})).len(), 2);
    let pre: Vec<String> = find(json!({"prefix": "op"})).into_iter().map(|s| s.0).collect();
    assert_eq!(pre, vec!["open_repo"]);
    let outline = call(&url, "outline", json!({"path": "src/lib.rs"}));
    let names: Vec<&str> = outline["symbols"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Point", "norm", "helper", "main"]);

    let ch = call(&url, "open_change", json!({"agent": "a", "intent": "add parse"}));
    let id = ch["id"].as_str().unwrap();
    call(
        &url,
        "apply",
        json!({"change": id, "edits": [{"path": "src/parse.rs", "content": "pub fn parse_args() {}\n"}]}),
    );
    assert!(find(json!({"name": "parse_args"})).is_empty(), "not on main yet");
    assert_eq!(find(json!({"name": "parse_args", "at": id})).len(), 1);
    call(&url, "land", json!({"change": id}));
    assert_eq!(
        find(json!({"name": "parse_args"})),
        vec![("parse_args".into(), "function".into(), "src/parse.rs".into(), 1)]
    );
}

#[test]
fn bisect_finds_a_planted_bad_change_and_revert_removes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 12, 20, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    std::fs::write(
        data.join("treeyard.toml"),
        "[bisect]\ntest = \"! grep -q PLANTED f00.txt\"\n[bisect.commands]\nfiles = \"test -f f05.txt\"\n",
    )
    .unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let good = call(&url, "status", json!({}))["main"].as_str().unwrap().to_string();
    let mut planted = String::new();
    for i in 0..40 {
        let (path, from, to) = if i == 23 {
            ("f00.txt".to_string(), "file 0 line 7\n".to_string(), "file 0 line 7 PLANTED\n".to_string())
        } else {
            (format!("f{:02}.txt", 1 + i % 11), format!("file {} line {}\n", 1 + i % 11, i / 11), format!("edit {i}\n"))
        };
        let ch = call(
            &url,
            "open_change",
            json!({"agent": format!("agent-{i}"), "intent": format!("change {i}"), "model": "sim"}),
        );
        let id = ch["id"].as_str().unwrap();
        call(&url, "apply", json!({"change": id, "edits": [{"path": path, "replace": {"old": from, "new": to}}]}));
        let landed = call(&url, "land", json!({"change": id}));
        assert_eq!(landed["status"], "landed", "{landed}");
        if i == 23 {
            planted = landed["changeset"].as_str().unwrap().to_string();
        }
    }
    let b = call(&url, "bisect", json!({"good": good}));
    assert_eq!(b["first_bad"]["changeset"], planted.as_str(), "{b}");
    assert_eq!(b["first_bad"]["agent"], "agent-23");
    assert_eq!(b["candidates"], 40);
    assert!(b["tests"].as_u64().unwrap() <= 2 + 6, "binary search: {} tests", b["tests"]);
    // A named command from [bisect.commands]; an unknown name is an error.
    let no = treeyard::client::call(&url, "bisect", json!({"good": good, "command": "nope"}));
    assert!(format!("{:#}", no.unwrap_err()).contains("no bisect command"));
    let reverted = call(&url, "revert", json!({"changeset": planted, "agent": "fixer"}));
    assert_eq!(reverted["status"], "landed");
    let text = call(&url, "read", json!({"path": "f00.txt"}))["content"].as_str().unwrap().to_string();
    assert!(!text.contains("PLANTED") && text.contains("file 0 line 7\n"));
    let again = treeyard::client::call(&url, "bisect", json!({"good": good}));
    assert!(format!("{:#}", again.unwrap_err()).contains("passes on bad"));
    // The audit log shows the bisect and the revert.
    let audit = call(&url, "audit", json!({"limit": 5}));
    let ops: Vec<&str> = audit["entries"].as_array().unwrap().iter().map(|e| e["op"].as_str().unwrap()).collect();
    assert!(ops.contains(&"bisect") && ops.contains(&"revert"), "{ops:?}");
}

/// Real-repo blame check (needs a clone): `TREEYARD_REAL_REPO=/path cargo test --release
/// --test derived -- --ignored --nocapture`. Default: the ripgrep clone of the harness.
#[test]
#[ignore]
fn blame_equals_git_blame_on_a_real_repo() {
    let repo = std::env::var("TREEYARD_REAL_REPO").unwrap_or("/private/tmp/rootstock-harness/oss/ripgrep".into());
    let src = Path::new(&repo);
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let branch = git(src, &["symbolic-ref", "--short", "HEAD"]).trim().to_string();
    Repo::init_with_history(&data, src, &branch, None, 300).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let touched = git(src, &["log", "--first-parent", "--format=", "--name-only", "-n300"]);
    let head: std::collections::HashSet<String> = files(src).into_iter().collect();
    let mut paths: Vec<&str> = touched.lines().filter(|p| head.contains(*p)).collect();
    paths.sort();
    paths.dedup();
    let (mut same, mut lines) = (0, 0);
    for p in &paths {
        let want = git_blame(src, None, "HEAD", p);
        lines += want.len();
        if rs_blame(&url, p, None) == want {
            same += 1;
        } else {
            println!("differs: {p}");
        }
    }
    println!("{same} of {} files ({lines} lines) equal to git blame --first-parent", paths.len());
    assert_eq!(same, paths.len());
}

#[test]
fn derived_data_is_persisted_and_grows_incrementally() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history_repo(tmp.path());
    let data = tmp.path().join("data");
    Repo::init_with_history(&data, &src, "main", None, 50).unwrap();
    std::fs::write(data.join("treeyard.toml"), "[index]\nsearch = \"eager\"\nsymbols = \"eager\"\n").unwrap();
    let repo = Arc::new(Repo::open(&data).unwrap());
    let (tri, sym) = treeyard::indexer::catch_up(&repo, true, true).unwrap();
    assert!(tri == 7 && sym == 4, "first pass indexes every blob: {tri} trigram rows, {sym} symbol rows");
    assert_eq!(treeyard::indexer::catch_up(&repo, true, true).unwrap(), (0, 0), "nothing new");
    let url = spawn_server(repo.clone());
    let ch = call(&url, "open_change", json!({"agent": "a", "intent": "one new file"}));
    let id = ch["id"].as_str().unwrap();
    call(&url, "apply", json!({"change": id, "edits": [{"path": "src/new.rs", "content": "pub fn fresh() {}\n"}]}));
    call(&url, "land", json!({"change": id}));
    assert_eq!(treeyard::indexer::catch_up(&repo, true, true).unwrap(), (1, 1), "only the new blob");
    let blame_rows = |r: &Repo| -> i64 {
        r.with(|i| Ok(i.store.conn().query_row("SELECT COUNT(*) FROM blame_cache", [], |x| x.get(0))?)).unwrap()
    };
    call(&url, "blame", json!({"path": "f00.txt"}));
    let cached = blame_rows(&repo);
    assert!(cached >= 2, "blame keeps one row per (changeset, path) it computed: {cached}");
    call(&url, "blame", json!({"path": "f00.txt"}));
    assert_eq!(blame_rows(&repo), cached, "a second blame reads the cache");
    // After a restart the rows are still there: nothing is computed again.
    drop(url);
    let again = Repo::open(&data).unwrap();
    let main = again.with(|r| r.main()).unwrap();
    assert_eq!(treeyard::search::prepare(&again, &main).unwrap(), 0);
    assert_eq!(treeyard::symbols::prepare(&again, &main).unwrap(), 0);
    assert_eq!(blame_rows(&again), cached);
}

/// `search::build_index` builds the posting index outside the repo lock. Lands after the build
/// (and blobs without a stored row) must still give the scan's matches, and a second build
/// must keep the first index.
#[test]
fn index_built_outside_the_lock_follows_later_lands() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history_repo(tmp.path());
    let data = tmp.path().join("data");
    Repo::init_with_history(&data, &src, "main", None, 10).unwrap();
    let repo = Arc::new(Repo::open(&data).unwrap());
    let old_main = repo.with(|r| r.main()).unwrap();
    // No `prepare` first: every row is computed inside `build_index`.
    assert!(treeyard::search::build_index(&repo, &old_main).unwrap());
    assert!(!treeyard::search::build_index(&repo, &old_main).unwrap(), "the first index stays");
    let url = spawn_server(repo.clone());
    for (i, body) in ["late needle_qq one\n", "late needle_qq two\n"].iter().enumerate() {
        let ch = call(&url, "open_change", json!({"agent": format!("late{i}"), "intent": "after the build"}));
        let id = ch["id"].as_str().unwrap();
        call(
            &url,
            "apply",
            json!({"change": id, "edits": [{"path": format!("late/{i}.txt"), "content": body}, {"path": format!("f0{}.txt", i * 2), "delete": true}]}),
        );
        assert_eq!(call(&url, "land", json!({"change": id}))["status"], "landed");
    }
    for pat in ["needle_qq", "line 1", "file 1 line"] {
        let idx = call(&url, "grep", json!({"pattern": pat, "max": 1000}));
        let scan = call(&url, "grep", json!({"pattern": pat, "max": 1000, "index": false}));
        assert_eq!(idx["matches"], scan["matches"], "pattern {pat}");
        assert_eq!(idx["index"]["used"], true);
    }
    let hits = call(&url, "grep", json!({"pattern": "needle_qq"}));
    assert_eq!(hits["matches"].as_array().unwrap().len(), 2);
}
