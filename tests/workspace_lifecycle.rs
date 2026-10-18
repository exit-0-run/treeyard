//! Workspace lifecycle: the repo config file, states and logs, gc, polite git, mirrors,
//! sparse workspaces and seeding from a dirty checkout.

mod common;

use common::fixture_repo;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use treeyard::repo::Repo;
use treeyard::workspace::{CreateOptions, Node, WsState};

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
