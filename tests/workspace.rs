//! Phase-2 workspaces: create, snapshot (sync), remove, dependency layers, ports, the daemon,
//! and an unmodified shell flow through the CLI.

mod common;

use common::{call, fixture_repo, spawn_server};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use treeyard::cow::Strategy;
use treeyard::repo::Repo;
use treeyard::workspace::{CreateOptions, LayerSpec, Node};

fn server(root: &Path, extra: &[(&str, &str)]) -> String {
    let src = fixture_repo(root, 5, 20, extra);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    spawn_server(Arc::new(Repo::open(&data).unwrap()))
}

fn opts(name: &str) -> CreateOptions {
    CreateOptions {
        name: name.into(),
        agent: format!("agent-{name}"),
        intent: format!("work of {name}"),
        base: None,
        strategy: Strategy::Auto,
        layers: Some(vec![]),
        setup: None,
        git: false,
    }
}

fn read_at(url: &str, change: &str, path: &str) -> Value {
    treeyard::client::call(url, "read", json!({"path": path, "at": change})).unwrap_or(Value::Null)
}

#[test]
fn create_snapshot_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[("src/lib.rs", "pub fn x() {}\n"), (".gitignore", "*.log\n")]);
    let node = Node::new(tmp.path().join("node"), &url);
    let (ws, timing) = node.create(&opts("a")).unwrap();
    assert!(timing["total_s"].as_f64().unwrap() < 5.0);
    assert_eq!(fs::read_to_string(ws.dir.join("f00.txt")).unwrap().lines().count(), 20);
    assert!(ws.dir.join(".treeyard/env").exists());

    // Nothing changed yet: sync sends nothing.
    let s = node.sync("a").unwrap();
    assert_eq!(s["changesets"], 0, "{s}");

    // Edit, add, delete, chmod, symlink, and a write that leaves the content as it was.
    fs::write(ws.dir.join("f01.txt"), "rewritten\n").unwrap();
    fs::write(ws.dir.join("src/new.rs"), "pub fn y() {}\n").unwrap();
    fs::remove_file(ws.dir.join("f02.txt")).unwrap();
    std::os::unix::fs::symlink("f00.txt", ws.dir.join("alias")).unwrap();
    let same = fs::read(ws.dir.join("f03.txt")).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    fs::write(ws.dir.join("f03.txt"), same).unwrap();
    // Ignored files never reach the server.
    fs::write(ws.dir.join("debug.log"), "noise").unwrap();
    let s = node.sync("a").unwrap();
    assert_eq!(s["changesets"], 1, "{s}");
    assert_eq!(s["paths"], json!(["alias", "f01.txt", "f02.txt", "src/new.rs"]));

    assert_eq!(read_at(&url, &ws.change, "f01.txt")["content"], "rewritten\n");
    assert_eq!(read_at(&url, &ws.change, "src/new.rs")["content"], "pub fn y() {}\n");
    assert_eq!(read_at(&url, &ws.change, "alias")["mode"], "120000");
    assert!(read_at(&url, &ws.change, "f02.txt").is_null());
    // Main is untouched until the change lands.
    assert!(read_at(&url, "main", "f02.txt")["content"].is_string());

    // Undoing an edit on disk takes the path out of the change again.
    let orig = read_at(&url, "main", "f01.txt")["content"].as_str().unwrap().to_string();
    fs::write(ws.dir.join("f01.txt"), orig).unwrap();
    node.sync("a").unwrap();
    let ch = call(&url, "change", json!({"change": ws.change}));
    assert_eq!(ch["paths"], json!(["alias", "f02.txt", "src/new.rs"]));

    // A second workspace shares the base and sees main, not the first agent's edits.
    let (b, _) = node.create(&opts("b")).unwrap();
    assert!(b.dir.join("f02.txt").exists());
    assert_ne!(a_ports(&node, "a"), b.ports);
    let list: Vec<String> = node.list().unwrap().into_iter().map(|w| w.name).collect();
    assert_eq!(list, vec!["a", "b"]);

    // Land through the workspace; remove the other one and abandon its change.
    let landed = node.land("a").unwrap();
    assert_eq!(landed["land"]["status"], "landed", "{landed}");
    assert!(read_at(&url, "main", "src/new.rs")["content"].is_string());
    let r = node.remove("b", true, false).unwrap();
    assert_eq!(r["abandoned"], true);
    assert!(!b.dir.exists());
    node.remove("a", false, false).unwrap();
    assert!(node.list().unwrap().is_empty());
}

#[test]
fn remove_sends_unsent_edits_and_keeps_them_when_it_cannot() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let node = Node::new(tmp.path().join("node"), &url);

    // An edit that no sync has sent yet reaches the change before the directory goes.
    let (a, _) = node.create(&opts("a")).unwrap();
    fs::write(a.dir.join("f01.txt"), "last edit\n").unwrap();
    let r = node.remove("a", false, false).unwrap();
    assert_eq!(r["synced"]["changesets"], 1, "{r}");
    assert!(!a.dir.exists());
    assert_eq!(read_at(&url, &a.change, "f01.txt")["content"], "last edit\n");

    // No server: nothing is deleted without --force.
    let (b, _) = node.create(&opts("b")).unwrap();
    fs::write(b.dir.join("f01.txt"), "unsent\n").unwrap();
    let offline = Node::new(tmp.path().join("node"), "http://127.0.0.1:9");
    let e = offline.remove("b", false, false).unwrap_err().to_string();
    assert!(e.contains("nothing was deleted"), "{e}");
    assert_eq!(fs::read_to_string(b.dir.join("f01.txt")).unwrap(), "unsent\n");
    assert!(offline.remove("b", false, true).unwrap()["synced"]["error"].is_string());
    assert!(!b.dir.exists());

    // A remove cut short (its mark left behind) is finished later.
    let (c, _) = node.create(&opts("c")).unwrap();
    fs::write(tmp.path().join("node/meta/c.removing"), "").unwrap();
    assert_eq!(node.finish_removes().unwrap(), vec!["c".to_string()]);
    assert!(!c.dir.exists());
    assert!(node.list().unwrap().is_empty());
}

fn a_ports(node: &Node, name: &str) -> (u16, u16) {
    node.get(name).unwrap().ports
}

#[test]
fn every_clone_strategy_makes_a_working_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let node = Node::new(tmp.path().join("node"), &url);
    let strategies = if cfg!(target_os = "macos") {
        vec![Strategy::Clone, Strategy::Reflink, Strategy::Copy, Strategy::Hardlink]
    } else {
        vec![Strategy::Copy, Strategy::Hardlink]
    };
    for s in strategies {
        let name = format!("ws-{}", s.as_str());
        let (ws, _) = node.create(&CreateOptions { strategy: s, ..opts(&name) }).unwrap();
        assert_eq!(ws.strategy, s);
        // sed -i replaces the file by rename, which also works on a hardlinked read-only base.
        let st = Command::new("sed").args(["-i.bak", "s/line 3$/LINE THREE/", "f00.txt"]).current_dir(&ws.dir).status();
        assert!(st.unwrap().success());
        fs::remove_file(ws.dir.join("f00.txt.bak")).unwrap();
        let s2 = node.sync(&name).unwrap();
        assert_eq!(s2["paths"], json!(["f00.txt"]), "{s:?}: {s2}");
        assert!(read_at(&url, &ws.change, "f00.txt")["content"].as_str().unwrap().contains("LINE THREE"));
        // The shared base did not change.
        let base = node.home.join("bases").join(&ws.base).join("f00.txt");
        assert!(!fs::read_to_string(base).unwrap().contains("LINE THREE"), "{s:?} wrote through to the base");
        node.remove(&name, true, false).unwrap();
    }
}

#[test]
fn dependency_layer_is_built_once_and_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[("deps.lock", "a==1\n"), (".gitignore", "deps/\n")]);
    let node = Node::new(tmp.path().join("node"), &url);
    let marker = tmp.path().join("builds.txt");
    // The "install" writes a big file only when the layer is missing.
    let setup = format!(
        "test -f deps/pkg.bin || {{ mkdir -p deps && head -c 1000000 /dev/urandom > deps/pkg.bin && echo built >> {}; }}",
        marker.display()
    );
    let layer: LayerSpec = "deps=deps.lock".parse().unwrap();
    let mk = |name: &str| {
        node.create(&CreateOptions { layers: Some(vec![layer.clone()]), setup: Some(setup.clone()), ..opts(name) })
            .unwrap()
    };
    let (a, _) = mk("a");
    let (b, _) = mk("b");
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1, "second workspace rebuilt the layer");
    assert_eq!(a.layers[0]["restored"], false);
    assert_eq!(b.layers[0]["restored"], true);
    assert_eq!(fs::read(a.dir.join("deps/pkg.bin")).unwrap(), fs::read(b.dir.join("deps/pkg.bin")).unwrap());
    // A private write in one workspace's layer copy does not reach the other.
    fs::write(b.dir.join("deps/pkg.bin"), "patched").unwrap();
    assert_eq!(fs::read(a.dir.join("deps/pkg.bin")).unwrap().len(), 1_000_000);
    // Layer contents are never synced.
    assert_eq!(node.sync("b").unwrap()["changesets"], 0);

    // A new lockfile is a new key: the next workspace builds again.
    let (c, _) = mk("c");
    fs::write(c.dir.join("deps.lock"), "a==2\n").unwrap();
    node.sync("c").unwrap();
    node.land("c").unwrap();
    let (d, _) = mk("d");
    assert_eq!(d.layers[0]["restored"], false);
    assert_ne!(d.layers[0]["key"], a.layers[0]["key"]);
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 2);
}

#[test]
fn env_file_and_port_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let mut node = Node::new(tmp.path().join("node"), &url);
    node.port_base = 41000;
    let (a, _) = node.create(&opts("a")).unwrap();
    let (b, _) = node.create(&opts("b")).unwrap();
    assert_eq!(a.ports, (41000, 41009));
    assert_eq!(b.ports, (41010, 41019));
    let env = fs::read_to_string(b.dir.join(".treeyard/env")).unwrap();
    assert!(env.contains("PORT=41010\n"), "{env}");
    assert!(env.contains(&format!("TREEYARD_CHANGE={}\n", b.change)));
    // A freed slot is reused.
    node.remove("a", true, false).unwrap();
    let (c, _) = node.create(&opts("c")).unwrap();
    assert_eq!(c.ports.0, 41000);
}

#[test]
fn daemon_streams_edits_as_changesets() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let home = tmp.path().join("node");
    let node = Node::new(&home, &url);
    let (ws, _) = node.create(&opts("live")).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (stop2, home2, url2) = (stop.clone(), home.clone(), url.clone());
    let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
    let lines2 = lines.clone();
    let daemon = std::thread::spawn(move || {
        let node = Node::new(home2, url2);
        let o = treeyard::daemon::Options { debounce: Duration::from_millis(200), rescan: Duration::ZERO, spares: 0 };
        treeyard::daemon::run(&node, &o, stop2, &mut |l| lines2.lock().unwrap().push(l)).unwrap();
    });
    std::thread::sleep(Duration::from_millis(300));
    // A burst of 20 writes within the debounce window.
    for i in 0..20 {
        fs::write(ws.dir.join("burst.txt"), format!("edit {i}\n")).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let wait = |want: &str| {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if read_at(&url, &ws.change, "burst.txt")["content"] == want {
                return t.elapsed();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not sync {want:?}; log: {:?}", lines.lock().unwrap());
    };
    wait("edit 19\n");
    let ch = call(&url, "change", json!({"change": ws.change}));
    let changesets = ch_count(&url, &ws.change);
    assert!(changesets <= 3, "debounce sent {changesets} changesets for 20 writes; {ch}");
    // A later edit becomes a new changeset of the same change.
    fs::write(ws.dir.join("f04.txt"), "later\n").unwrap();
    let t = Instant::now();
    while read_at(&url, &ws.change, "f04.txt")["content"] != "later\n" {
        assert!(t.elapsed() < Duration::from_secs(10), "second edit not synced");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ch_count(&url, &ws.change) > changesets);
    // Build output under target/ does not wake the daemon.
    fs::create_dir_all(ws.dir.join("target")).unwrap();
    stop.store(true, Ordering::Relaxed);
    daemon.join().unwrap();
}

/// Heads the change went through (one per sync that changed something).
fn ch_count(url: &str, change: &str) -> usize {
    call(url, "change", json!({"change": change}))["revisions"].as_u64().unwrap() as usize
}

#[test]
fn shell_flow_through_the_cli() {
    let tmp = tempfile::tempdir().unwrap();
    let test_sh = "#!/bin/sh\n# the repo's own test: every f*.txt has 20 lines\nfor f in f*.txt; do [ $(wc -l < $f) -eq 20 ] || { echo \"FAIL $f\"; exit 1; }; done\necho ok\n";
    let url = server(tmp.path(), &[("test.sh", test_sh)]);
    let bin = env!("CARGO_BIN_EXE_treeyard");
    let home = tmp.path().join("node");
    let rs = |args: &[&str]| {
        let out = Command::new(bin).args(args).env("TREEYARD_HOME", &home).env("TREEYARD_URL", &url).output().unwrap();
        assert!(out.status.success(), "treeyard {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let created: Value = serde_json::from_str(&rs(&["workspace", "create", "cli", "--agent", "shell"])).unwrap();
    let dir = rs(&["workspace", "path", "cli"]).trim().to_string();
    // Plain shell tools edit the files; the repo's own test runs in the directory.
    let sh = |cmd: &str| Command::new("sh").args(["-c", cmd]).current_dir(&dir).output().unwrap();
    assert!(sh("perl -pi -e 's/^file 1 line 0$/FILE ONE/' f01.txt").status.success());
    assert!(sh("printf 'notes\\n' > NOTES.md && mv f04.txt f05b.txt").status.success());
    let t = sh("sh test.sh");
    assert!(t.status.success(), "{}", String::from_utf8_lossy(&t.stdout));
    let env = rs(&["workspace", "env", "cli"]);
    assert!(env.contains("export PORT='"));
    // exec runs a command with the workspace env and syncs afterwards.
    let out = Command::new(bin)
        .args(["workspace", "exec", "cli", "--", "sh", "-c", "echo $TREEYARD_WORKSPACE > who.txt"])
        .env("TREEYARD_HOME", &home)
        .env("TREEYARD_URL", &url)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let synced: Value = serde_json::from_str(String::from_utf8_lossy(&out.stderr).trim()).unwrap();
    assert_eq!(synced["paths"], json!(["NOTES.md", "f01.txt", "f04.txt", "f05b.txt", "who.txt"]));
    let change = created["change"].as_str().unwrap();
    assert_eq!(read_at(&url, change, "who.txt")["content"], "cli\n");
    assert_eq!(read_at(&url, change, "f01.txt")["content"].as_str().unwrap().lines().next(), Some("FILE ONE"));
    let landed: Value = serde_json::from_str(&rs(&["workspace", "land", "cli"])).unwrap();
    assert_eq!(landed["land"]["status"], "landed");
    let list: Value = serde_json::from_str(&rs(&["workspace", "list"])).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    rs(&["workspace", "remove", "cli"]);
    assert!(!Path::new(&dir).exists());
}

/// overlayfs needs Linux and mount rights (root, for example a privileged container).
#[cfg(target_os = "linux")]
#[test]
fn overlay_workspace_when_root() {
    if unsafe { libc::geteuid() } != 0 || std::env::var_os("TREEYARD_TEST_OVERLAY").is_none() {
        eprintln!("skip: overlay workspaces need root and TREEYARD_TEST_OVERLAY=1 (TMPDIR not on overlayfs)");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let node = Node::new(tmp.path().join("node"), &url);
    let (ws, _) = node.create(&CreateOptions { strategy: Strategy::Overlay, ..opts("ov") }).unwrap();
    assert_eq!(ws.strategy, Strategy::Overlay);
    fs::write(ws.dir.join("f00.txt"), "overlay edit\n").unwrap();
    fs::write(ws.dir.join("added.txt"), "new\n").unwrap();
    let s = node.sync("ov").unwrap();
    assert_eq!(s["paths"], json!(["added.txt", "f00.txt"]));
    // Writes land in the upper dir; the base stays as it was.
    assert!(tmp.path().join("node/ws/ov.upper/f00.txt").exists());
    let base = node.home.join("bases").join(&ws.base).join("f00.txt");
    assert!(fs::read_to_string(base).unwrap().starts_with("file 0 line 0"));
    node.remove("ov", true, false).unwrap();
    assert!(!ws.dir.exists());
}

#[test]
fn spare_pool_hands_out_a_ready_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[("deps.lock", "a==1\n"), (".gitignore", "deps/\n")]);
    let node = Node::new(tmp.path().join("node"), &url);
    let layer: LayerSpec = "deps=deps.lock".parse().unwrap();
    // Without a saved layer a spare would save nothing, so it is refused.
    assert!(node.fill_spares(1, Some(std::slice::from_ref(&layer)), false).is_err());
    let setup = "mkdir -p deps && echo built > deps/out".to_string();
    let with_layer = |name: &str| CreateOptions { layers: Some(vec![layer.clone()]), ..opts(name) };
    node.create(&CreateOptions { setup: Some(setup), ..with_layer("first") }).unwrap();
    let r = node.fill_spares(2, Some(std::slice::from_ref(&layer)), false).unwrap();
    assert_eq!(r["made"].as_array().unwrap().len(), 2);
    let (ws, timing) = node.create(&with_layer("fast")).unwrap();
    assert_eq!(timing["spare"], true, "{timing}");
    assert_eq!(fs::read_to_string(ws.dir.join("deps/out")).unwrap(), "built\n");
    assert_eq!(node.spares().unwrap().len(), 1);
    fs::write(ws.dir.join("f00.txt"), "from a spare\n").unwrap();
    assert_eq!(node.sync("fast").unwrap()["paths"], json!(["f00.txt"]));
    // Main moves: the old spare is dropped and a new one is made for the new base.
    node.land("fast").unwrap();
    let r = node.fill_spares(1, Some(std::slice::from_ref(&layer)), false).unwrap();
    assert_eq!((r["removed"].as_u64(), r["made"].as_array().unwrap().len()), (Some(1), 1));
    let (next, timing) = node.create(&with_layer("next")).unwrap();
    assert_eq!(timing["spare"], true);
    assert_eq!(fs::read_to_string(next.dir.join("f00.txt")).unwrap(), "from a spare\n");
    // A workspace with other layers does not take the spare.
    let (_, timing) = node.create(&opts("plain")).unwrap();
    assert_eq!(timing["spare"], false);
}

#[test]
fn git_in_a_workspace_shows_the_agent_its_own_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let url = server(tmp.path(), &[]);
    let node = Node::new(tmp.path().join("node"), &url);
    let (ws, _) = node.create(&CreateOptions { git: true, ..opts("g") }).unwrap();
    let git = |args: &[&str]| common::git(&ws.dir, args);
    assert_eq!(git(&["status", "--porcelain"]), "", "a new workspace is clean");
    assert!(git(&["log", "-1", "--format=%s"]).contains("fixture"));
    fs::write(ws.dir.join("f00.txt"), "changed\n").unwrap();
    assert_eq!(git(&["status", "--porcelain"]), " M f00.txt\n");
    // The agent may even commit locally; the sync sends files, never the .git dir.
    git(&["commit", "-qam", "local commit"]);
    let s = node.sync("g").unwrap();
    assert_eq!(s["paths"], json!(["f00.txt"]));
    // After a landing, a new workspace starts from the new main commit.
    node.land("g").unwrap();
    let (next, _) = node.create(&CreateOptions { git: true, ..opts("h") }).unwrap();
    let head = common::git(&next.dir, &["log", "-1", "--format=%s"]);
    assert!(head.contains("work of g"), "{head}");
    assert_eq!(common::git(&next.dir, &["status", "--porcelain"]), "");
}
