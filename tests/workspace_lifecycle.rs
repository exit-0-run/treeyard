//! Workspace lifecycle: the repo config file, states and logs, gc, polite git, mirrors,
//! sparse workspaces and seeding from a dirty checkout.

mod common;

use common::fixture_repo;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use treeyard::cow::Strategy;
use treeyard::repo::Repo;
use treeyard::workspace::{CreateOptions, GcOptions, Node, WsState};

fn server(root: &Path, extra: &[(&str, &str)]) -> String {
    let src = fixture_repo(root, 5, 20, extra);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    common::spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

fn opts(name: &str) -> CreateOptions {
    CreateOptions {
        name: name.into(),
        agent: format!("agent-{name}"),
        intent: format!("work of {name}"),
        layers: Some(vec![]),
        ..CreateOptions::default()
    }
}

fn read_at(url: &str, change: &str, path: &str) -> Value {
    treeyard::client::call(url, "read", json!({"path": path, "at": change})).unwrap_or(Value::Null)
}

#[test]
fn repo_config_drives_setup_copy_ports_run_and_teardown() {
    let tmp = tempfile::tempdir().unwrap();
    let bye = tmp.path().join("teardown.txt");
    let config = format!(
        "setup = \"mkdir -p deps && echo built > deps/out && echo ok > setup.out\"\n\
         teardown = \"echo $TREEYARD_WORKSPACE >> {}\"\n\
         layers = [\"deps=deps.lock\"]\n\
         copy = [\".env*\"]\n\
         ports = 3\n\
         run = \"echo ran > ran.txt\"\n",
        bye.display()
    );
    let url = server(
        tmp.path(),
        &[
            (".treeyard/workspace.toml", &config),
            ("deps.lock", "a==1\n"),
            (".gitignore", ".env*\ndeps/\nsetup.out\n"),
            (".worktreeinclude", "# certificates too\ncerts/*.pem\n"),
        ],
    );
    // The checkout that holds the gitignored files.
    let main = tmp.path().join("main-checkout");
    fs::create_dir_all(main.join("certs")).unwrap();
    fs::write(main.join(".env"), "KEY=dev\n").unwrap();
    fs::write(main.join(".env.local"), "LOCAL=1\n").unwrap();
    fs::write(main.join("certs/dev.pem"), "cert\n").unwrap();
    fs::create_dir_all(main.join("node_modules")).unwrap();
    fs::write(main.join("node_modules/.env"), "never walked\n").unwrap();

    let home = tmp.path().join("node");
    let node = Node::new(&home, &url);
    let o = CreateOptions { layers: None, copy_from: Some(main.clone()), ..opts("a") };
    let (a, timing) = node.create(&o).unwrap();
    // setup, layers and ports come from the repo file.
    assert_eq!(fs::read_to_string(a.dir.join("setup.out")).unwrap(), "ok\n");
    assert_eq!(a.layers[0]["dir"], "deps", "{:?}", a.layers);
    assert_eq!(a.ports.1 - a.ports.0 + 1, 3);
    // Ignored files are copied; a file that a sync would send is not.
    assert_eq!(timing["copy"]["copied"], json!([".env", ".env.local"]), "{timing}");
    assert_eq!(timing["copy"]["skipped"][0]["path"], "certs/dev.pem", "{timing}");
    assert_eq!(fs::read_to_string(a.dir.join(".env")).unwrap(), "KEY=dev\n");
    assert!(!a.dir.join("certs/dev.pem").exists());
    assert_eq!(node.sync("a").unwrap()["changesets"], 0, "copied and setup files are never synced");

    // The repo file is a repo file: an edit of it reaches the change.
    let cfg_path = a.dir.join(".treeyard/workspace.toml");
    fs::write(&cfg_path, fs::read_to_string(&cfg_path).unwrap().replace("ports = 3", "ports = 4")).unwrap();
    assert_eq!(node.sync("a").unwrap()["paths"], json!([".treeyard/workspace.toml"]));

    // The node file overrides the repo file, and options override both.
    fs::write(home.join("workspace.toml"), "ports = 6\n").unwrap();
    let (b, _) = node.create(&opts("b")).unwrap();
    assert_eq!(b.ports.1 - b.ports.0 + 1, 6);
    assert!(b.ports.0 > a.ports.1, "blocks do not overlap: {:?} {:?}", a.ports, b.ports);
    let (c, _) = node.create(&CreateOptions { ports: Some(2), teardown: Some("exit 3".into()), ..opts("c") }).unwrap();
    assert_eq!(c.ports.1 - c.ports.0 + 1, 2);

    // exec with no command runs `run`.
    let bin = env!("CARGO_BIN_EXE_treeyard");
    let out = Command::new(bin)
        .args(["workspace", "exec", "a"])
        .env("TREEYARD_HOME", &home)
        .env("TREEYARD_URL", &url)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(read_at(&url, &a.change, "ran.txt")["content"], "ran\n");

    // remove runs the teardown; a failing teardown is reported and the remove still happens.
    let r = node.remove("a", false, false).unwrap();
    assert_eq!(r["teardown"]["ok"], true, "{r}");
    assert_eq!(fs::read_to_string(&bye).unwrap(), "a\n");
    let r = node.remove("c", true, false).unwrap();
    assert_eq!(r["teardown"]["ok"], false, "{r}");
    assert!(!c.dir.exists());
    assert!(Path::new(r["teardown"]["log"].as_str().unwrap()).exists());
}

#[test]
fn copy_without_a_source_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[(".treeyard/workspace.toml", "copy = [\".env\"]\n")]);
    let node = Node::new(tmp.path().join("node"), &url);
    let (_, timing) = node.create(&opts("a")).unwrap();
    assert!(timing["copy"]["error"].as_str().unwrap().contains("--copy-from"), "{timing}");
    // A broken repo file stops the create with the file name in the error.
    let url = server(&tmp.path().join("bad"), &[(".treeyard/workspace.toml", "setpu = \"x\"\n")]);
    let node = Node::new(tmp.path().join("node2"), &url);
    let e = format!("{:#}", node.create(&opts("b")).unwrap_err());
    assert!(e.contains("workspace.toml") && e.contains("setpu"), "{e}");
}

#[test]
fn create_records_state_counts_and_logs() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[(".gitignore", "*.out\n")]);
    let node = Node::new(tmp.path().join("node"), &url);
    let (ws, _) =
        node.create(&CreateOptions { setup: Some("echo hello; echo x > x.out".into()), ..opts("a") }).unwrap();
    assert_eq!(ws.state, WsState::Ready);
    assert_eq!(ws.counts["files"], 6, "{}", ws.counts);
    let listed = serde_json::to_value(node.list().unwrap()).unwrap();
    assert_eq!(listed[0]["state"], "ready", "{listed}");
    let create_log = fs::read_to_string(ws.create_log.as_ref().unwrap()).unwrap();
    for step in ["base ready", "cloned 6 files", "setup", "ready in"] {
        assert!(create_log.contains(step), "{step:?} missing in {create_log}");
    }
    assert_eq!(fs::read_to_string(ws.setup_log.as_ref().unwrap()).unwrap(), "hello\n");

    // A failed setup leaves state failed, the error and a readable log; remove still cleans up.
    let bad = CreateOptions { setup: Some("echo step one; echo boom >&2; exit 2".into()), ..opts("b") };
    let e = format!("{:#}", node.create(&bad).unwrap_err());
    assert!(e.contains("boom") && e.contains("b.setup.log"), "{e}");
    let b = node.get("b").unwrap();
    assert_eq!(b.state, WsState::Failed);
    assert!(b.error.as_deref().unwrap().contains("boom"));
    assert_eq!(fs::read_to_string(b.setup_log.as_ref().unwrap()).unwrap(), "step one\nboom\n");
    assert!(fs::read_to_string(b.create_log.as_ref().unwrap()).unwrap().contains("failed: setup"));
    node.remove("b", true, false).unwrap();
    assert!(!b.dir.exists());

    // A create cut short in `creating` never syncs (its files are not edits); remove cleans it up.
    let (c, _) = node.create(&opts("c")).unwrap();
    let meta = tmp.path().join("node/meta/c.json");
    let mut rec: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
    rec["state"] = json!("creating");
    fs::write(&meta, rec.to_string()).unwrap();
    fs::remove_file(c.dir.join("f00.txt")).unwrap();
    assert!(node.sync("c").unwrap_err().to_string().contains("still being created"));
    let r = node.remove("c", false, false).unwrap();
    assert!(r["synced"].is_null(), "{r}");
    assert!(read_at(&url, &c.change, "f00.txt")["content"].is_string(), "no deletion was sent");
}

fn gc_opts(max: usize) -> GcOptions {
    GcOptions { max: Some(max), max_bytes: None, min_age: Duration::ZERO, dry_run: false }
}

#[test]
fn gc_removes_finished_then_oldest_and_never_unsynced_work() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let node = Node::new(tmp.path().join("node"), &url);
    for n in ["old", "landed", "dirty", "fresh"] {
        node.create(&opts(n)).unwrap();
    }
    // `landed` lands; `dirty` has an edit no sync has sent; `fresh` synced last.
    fs::write(node.ws_dir("landed").join("f00.txt"), "landed edit\n").unwrap();
    node.land("landed").unwrap();
    fs::write(node.ws_dir("dirty").join("f01.txt"), "unsent\n").unwrap();
    fs::write(node.ws_dir("fresh").join("f02.txt"), "sent\n").unwrap();
    node.sync("fresh").unwrap();
    // Young workspaces are protected by default.
    let r = node.gc(&GcOptions { max: Some(0), ..GcOptions::default() }).unwrap();
    assert_eq!(r["removed"], json!([]), "{r}");
    assert!(r["skipped"].to_string().contains("younger than 10 minutes"), "{r}");

    let meta = tmp.path().join("node/meta/old.json");
    let mut rec: Value = serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
    rec["created"] = json!(1);
    fs::write(&meta, rec.to_string()).unwrap();

    // A dry run reports and removes nothing.
    let r = node.gc(&GcOptions { dry_run: true, ..gc_opts(2) }).unwrap();
    let names = |r: &Value| -> Vec<String> {
        r["removed"].as_array().unwrap().iter().map(|w| w["workspace"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(names(&r), vec!["landed", "old"], "{r}");
    assert_eq!(node.list().unwrap().len(), 4);

    let r = node.gc(&gc_opts(2)).unwrap();
    assert_eq!(names(&r), vec!["landed", "old"], "{r}");
    assert_eq!(r["removed"][0]["change"], "landed");
    assert!(r["freed_bytes"].as_u64().unwrap() > 0, "{r}");
    assert!(r["skipped"].to_string().contains("1 edited files not synced yet"), "{r}");
    // Down to zero: the dirty workspace stays, the others go.
    let r = node.gc(&gc_opts(0)).unwrap();
    assert_eq!(names(&r), vec!["fresh"], "{r}");
    let left: Vec<String> = node.list().unwrap().into_iter().map(|w| w.name).collect();
    assert_eq!(left, vec!["dirty"]);
    assert_eq!(fs::read_to_string(node.ws_dir("dirty").join("f01.txt")).unwrap(), "unsent\n");

    // One gc at a time.
    let f = fs::File::options().write(true).open(tmp.path().join("node/gc.lock")).unwrap();
    f.lock().unwrap();
    assert!(node.gc(&gc_opts(0)).unwrap_err().to_string().contains("another gc"));
    drop(f);

    // The daemon runs gc at start when --gc-every is set.
    node.sync("dirty").unwrap();
    let stop = Arc::new(AtomicBool::new(true));
    let mut lines = Vec::new();
    let o = treeyard::daemon::Options { gc: Some((Duration::from_secs(3600), gc_opts(0))), ..Default::default() };
    treeyard::daemon::run(&node, &o, stop, &mut |l| lines.push(l)).unwrap();
    assert!(lines.iter().any(|l| l.contains("\"gc\"") && l.contains("dirty")), "{lines:?}");
    assert!(node.list().unwrap().is_empty());
}

#[test]
fn daemon_waits_while_git_is_busy_in_a_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let home = tmp.path().join("node");
    let node = Node::new(&home, &url);
    let (ws, _) = node.create(&CreateOptions { git: true, ..opts("g") }).unwrap();
    // A merge stopped half way: the daemon must not send its files.
    fs::write(ws.dir.join(".git/MERGE_HEAD"), "0000000000000000000000000000000000000000\n").unwrap();
    fs::write(ws.dir.join("f00.txt"), "<<<<<<< HEAD\nmid-merge\n").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (stop2, lines2, home2, url2) = (stop.clone(), lines.clone(), home.clone(), url.clone());
    let daemon = std::thread::spawn(move || {
        let o = treeyard::daemon::Options {
            debounce: Duration::from_millis(50),
            rescan: Duration::from_millis(100),
            git_backoff: vec![Duration::from_millis(50)],
            ..Default::default()
        };
        treeyard::daemon::run(&Node::new(home2, url2), &o, stop2, &mut |l| lines2.lock().unwrap().push(l)).unwrap();
    });
    let wait = |what: &str, done: &dyn Fn() -> bool| {
        let t = std::time::Instant::now();
        while !done() {
            assert!(t.elapsed() < Duration::from_secs(20), "{what}; log: {:?}", lines.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let skips = || lines.lock().unwrap().iter().filter(|l| l.contains("MERGE_HEAD")).count();
    wait("two skipped tries", &|| skips() >= 2);
    assert!(read_at(&url, &ws.change, "f00.txt")["content"].as_str().unwrap().starts_with("file 0"));
    // The merge ends: the next try sends the result.
    fs::write(ws.dir.join("f00.txt"), "merged\n").unwrap();
    fs::remove_file(ws.dir.join(".git/MERGE_HEAD")).unwrap();
    wait("sync after the merge", &|| read_at(&url, &ws.change, "f00.txt")["content"] == "merged\n");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    daemon.join().unwrap();
}

#[test]
fn mirror_keeps_one_directory_in_step_with_the_change() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[("bin/run.sh", "#!/bin/sh\necho run\n")]);
    let node = Node::new(tmp.path().join("node"), &url);
    let (a, _) = node.create(&opts("a")).unwrap();
    let into = tmp.path().join("app");
    let r = node.mirror("a", Some(&into)).unwrap();
    assert_eq!(r["written"].as_array().unwrap().len(), 6, "{r}");
    assert_eq!(fs::read_to_string(into.join("f00.txt")).unwrap(), fs::read_to_string(a.dir.join("f00.txt")).unwrap());
    assert!(into.join(treeyard::workspace::MIRROR_MARK).exists());
    // Mark an unchanged file with an old mtime, and leave a local file the mirror never wrote.
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options().write(true).open(into.join("f01.txt")).unwrap().set_modified(old).unwrap();
    fs::write(into.join("local.env"), "mine\n").unwrap();

    fs::write(a.dir.join("f00.txt"), "edited\n").unwrap();
    fs::remove_file(a.dir.join("f02.txt")).unwrap();
    fs::create_dir_all(a.dir.join("new")).unwrap();
    fs::write(a.dir.join("new/x.txt"), "x\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(a.dir.join("bin/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let s = node.sync("a").unwrap();
    assert_eq!(s["mirror"]["written"], json!(["bin/run.sh", "f00.txt", "new/x.txt"]), "{s}");
    assert_eq!(s["mirror"]["deleted"], json!(["f02.txt"]), "{s}");
    assert_eq!(fs::read_to_string(into.join("f00.txt")).unwrap(), "edited\n");
    assert!(!into.join("f02.txt").exists());
    assert_eq!(fs::metadata(into.join("f01.txt")).unwrap().modified().unwrap(), old, "unchanged file was touched");
    assert_eq!(fs::read_to_string(into.join("local.env")).unwrap(), "mine\n");
    assert_ne!(fs::metadata(into.join("bin/run.sh")).unwrap().permissions().mode() & 0o111, 0);

    // A directory under ws/, or a non-empty directory that is not a mirror, is refused.
    let e = node.mirror("a", Some(&node.ws_dir("a").join("sub"))).unwrap_err().to_string();
    assert!(e.contains("outside"), "{e}");
    let busy = tmp.path().join("busy");
    fs::create_dir_all(&busy).unwrap();
    fs::write(busy.join("x"), "").unwrap();
    assert!(node.mirror("a", Some(&busy)).unwrap_err().to_string().contains("not empty"));
    // Another workspace cannot take a live mirror; after --off it can.
    node.create(&opts("b")).unwrap();
    assert!(node.mirror("b", Some(&into)).unwrap_err().to_string().contains("mirror of workspace \"a\""));

    // exec syncs, so it updates the mirror too.
    let bin = env!("CARGO_BIN_EXE_treeyard");
    let home = tmp.path().join("node");
    let out = Command::new(bin)
        .args(["workspace", "exec", "a", "--", "sh", "-c", "echo via-exec > f03.txt"])
        .env("TREEYARD_HOME", &home)
        .env("TREEYARD_URL", &url)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(fs::read_to_string(into.join("f03.txt")).unwrap(), "via-exec\n");

    node.mirror("a", None).unwrap();
    fs::write(a.dir.join("f00.txt"), "after off\n").unwrap();
    assert!(node.sync("a").unwrap()["mirror"].is_null());
    assert_eq!(fs::read_to_string(into.join("f00.txt")).unwrap(), "edited\n");
    let r = node.mirror("b", Some(&into)).unwrap();
    assert!(r["written"].as_array().unwrap().contains(&json!("f00.txt")), "{r}");
    assert!(!into.join("new/x.txt").exists(), "b has no new/x.txt");
}

#[test]
fn sparse_workspace_holds_only_its_cone_and_never_deletes_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let files: &[(&str, &str)] = &[
        ("a/x.txt", "x\n"),
        ("a/deep/y.txt", "y\n"),
        ("b/z.txt", "z\n"),
        ("c/sub/v.txt", "v\n"),
        ("p/top.txt", "top\n"),
        ("p/q/mid.txt", "mid\n"),
        ("p/q/r/s.txt", "s\n"),
        ("p/other/no.txt", "no\n"),
    ];
    let url = server(tmp.path(), files);
    let home = tmp.path().join("node");
    let node = Node::new(&home, &url);
    let (ws, _) = node.create(&CreateOptions { paths: vec!["a/".into(), "p/q/r/".into()], ..opts("s") }).unwrap();
    assert_eq!(ws.paths, vec!["a", "p/q/r"]);
    for f in ["f00.txt", "a/x.txt", "a/deep/y.txt", "p/top.txt", "p/q/mid.txt", "p/q/r/s.txt"] {
        assert!(ws.dir.join(f).exists(), "{f} missing");
    }
    for f in ["b", "c", "p/other"] {
        assert!(!ws.dir.join(f).exists(), "{f} must not be there");
    }
    assert_eq!(ws.counts["files"], 10, "{}", ws.counts);
    let listed = serde_json::to_value(node.list().unwrap()).unwrap();
    assert_eq!(listed[0]["paths"], json!(["a", "p/q/r"]));
    // Missing directories are not deletions, and files written outside the cone are not edits.
    fs::write(ws.dir.join("a/x.txt"), "x2\n").unwrap();
    fs::create_dir_all(ws.dir.join("b")).unwrap();
    fs::write(ws.dir.join("b/z.txt"), "blind overwrite\n").unwrap();
    fs::write(ws.dir.join("root-new.txt"), "new\n").unwrap();
    let s = node.sync("s").unwrap();
    assert_eq!(s["paths"], json!(["a/x.txt", "root-new.txt"]), "{s}");
    let landed = node.land("s").unwrap();
    assert_eq!(landed["land"]["status"], "landed", "{landed}");
    assert_eq!(read_at(&url, "main", "b/z.txt")["content"], "z\n");
    assert_eq!(read_at(&url, "main", "c/sub/v.txt")["content"], "v\n");
    assert_eq!(read_at(&url, "main", "a/x.txt")["content"], "x2\n");

    let e = node.create(&CreateOptions { paths: vec!["nope/".into()], ..opts("bad") }).unwrap_err();
    assert!(format!("{e:#}").contains("not a directory"), "{e:#}");

    // Through the CLI, with git: git status shows no deletions for the missing directories.
    let bin = env!("CARGO_BIN_EXE_treeyard");
    let out = Command::new(bin)
        .args(["workspace", "create", "g", "--git", "--paths", "b/,c/sub"])
        .env("TREEYARD_HOME", &home)
        .env("TREEYARD_URL", &url)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let g = node.get("g").unwrap();
    assert_eq!(g.paths, vec!["b", "c/sub"]);
    assert!(!g.dir.join("a").exists());
    assert_eq!(common::git(&g.dir, &["status", "--porcelain"]), "");
    fs::write(g.dir.join("b/z.txt"), "z2\n").unwrap();
    assert_eq!(common::git(&g.dir, &["status", "--porcelain"]), " M b/z.txt\n");
    assert_eq!(node.sync("g").unwrap()["paths"], json!(["b/z.txt"]));
}

#[test]
fn from_dir_seeds_the_change_with_a_checkouts_uncommitted_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[(".gitignore", "*.log\n"), ("sub/a.txt", "a\n"), ("other/b.txt", "b\n")]);
    // The source repo of the server is a checkout at the base commit; make it dirty.
    let src = tmp.path().join("src");
    fs::write(src.join("f00.txt"), "edited, not staged\n").unwrap();
    fs::remove_file(src.join("f01.txt")).unwrap();
    fs::write(src.join("new.txt"), "untracked\n").unwrap();
    fs::write(src.join("staged.txt"), "staged\n").unwrap();
    common::git(&src, &["add", "staged.txt"]);
    fs::write(src.join("debug.log"), "ignored\n").unwrap();
    fs::write(src.join("other/b.txt"), "b2\n").unwrap();
    let node = Node::new(tmp.path().join("node"), &url);
    let (ws, timing) = node
        .create(&CreateOptions { from_dir: Some(src.join("sub")), strategy: Strategy::Hardlink, ..opts("d") })
        .unwrap();
    let seeded = &timing["from_dir"];
    assert_eq!(seeded["written"], json!(["f00.txt", "other/b.txt", "staged.txt", "new.txt"]), "{timing}");
    assert_eq!(seeded["deleted"], json!(["f01.txt"]));
    assert_eq!(seeded["sync"]["changesets"], 1, "{timing}");
    assert!(!ws.dir.join("debug.log").exists(), "ignored files are not seeded");
    let ch = common::call(&url, "change", json!({"change": ws.change}));
    assert_eq!(ch["paths"], json!(["f00.txt", "f01.txt", "new.txt", "other/b.txt", "staged.txt"]), "{ch}");
    assert_eq!(read_at(&url, &ws.change, "f00.txt")["content"], "edited, not staged\n");
    assert_eq!(ch["revisions"], 2, "the empty head, then one changeset: {ch}");
    // The base was not written through (hardlink strategy replaces files, never writes them).
    let base = node.home.join("bases").join(&ws.base).join("f00.txt");
    assert!(fs::read_to_string(base).unwrap().starts_with("file 0"));
    // The probes never left an index.lock in the checkout.
    assert!(!src.join(".git/index.lock").exists());

    // A sparse workspace takes only the edits in its cone.
    let (_, timing) =
        node.create(&CreateOptions { from_dir: Some(src.clone()), paths: vec!["sub".into()], ..opts("s") }).unwrap();
    assert_eq!(timing["from_dir"]["skipped"][0], json!({"path": "other/b.txt", "why": "outside --paths"}), "{timing}");
    // Not a git checkout: the create fails and says so.
    let plain = tmp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let e = node.create(&CreateOptions { from_dir: Some(plain), ..opts("p") }).unwrap_err();
    assert!(format!("{e:#}").contains("git"), "{e:#}");
    assert_eq!(node.get("p").unwrap().state, WsState::Failed);
}
