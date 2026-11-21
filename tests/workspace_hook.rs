//! `treeyard workspace-hook create|remove` the way an orchestrator runs it: the built binary,
//! a cleared environment, only arguments and environment variables. The server is a real
//! `treeyard serve` child on a port the OS picks (`--port-file`), never a fixed one.

mod common;

use common::{fixture_repo, git};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_treeyard"))
}

/// A cleared environment: PATH (for git and sh), HOME and the given variables.
fn hook(dir: &Path, home: &Path, vars: &[(&str, &str)], args: &[&str]) -> Output {
    let mut c = Command::new(bin());
    c.arg("workspace-hook").args(args).current_dir(dir).env_clear().stdin(Stdio::null());
    c.env("PATH", std::env::var_os("PATH").unwrap_or_default()).env("HOME", home);
    for (k, v) in vars {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A source repo, its TreeYard data, a running server, a linked worktree (the orchestrator's).
struct Setup {
    tmp: tempfile::TempDir,
    repo: PathBuf,
    worktree: PathBuf,
    node: PathBuf,
    url: String,
    server: Child,
}

impl Setup {
    fn new() -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = fixture_repo(&root, 2, 5, &[]);
        let data = root.join("data");
        let init = Command::new(bin()).args(["init", "--data"]).arg(&data).arg("--from").arg(&repo).output().unwrap();
        assert!(init.status.success(), "init: {}", String::from_utf8_lossy(&init.stderr));
        let port_file = root.join("port");
        let server = Command::new(bin())
            .args(["serve", "--addr", "127.0.0.1:0", "--data"])
            .arg(&data)
            .arg("--port-file")
            .arg(&port_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let t0 = std::time::Instant::now();
        let port = loop {
            if let Ok(p) = std::fs::read_to_string(&port_file)
                && !p.trim().is_empty()
            {
                break p.trim().to_string();
            }
            assert!(t0.elapsed().as_secs() < 30, "serve did not write its port");
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let worktree = root.join("orchestrator").join("feat-login");
        git(&repo, &["worktree", "add", "-q", "-b", "feat-login", worktree.to_str().unwrap()]);
        for d in ["node", "home"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        Setup { node: root.join("node"), repo, worktree, url: format!("http://127.0.0.1:{port}"), server, tmp }
    }

    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }

    /// Runs the hook in the worktree with TREEYARD_URL and TREEYARD_HOME set.
    fn run(&self, args: &[&str]) -> Output {
        let vars = [("TREEYARD_URL", self.url.as_str()), ("TREEYARD_HOME", self.node.to_str().unwrap())];
        hook(&self.worktree, &self.home(), &vars, args)
    }

    fn open_changes(&self) -> Vec<Value> {
        let v = treeyard::client::call(&self.url, "changes", json!({})).unwrap();
        v["changes"].as_array().cloned().unwrap_or_default()
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

fn one_line_path(o: &Output) -> PathBuf {
    let out = stdout(o);
    assert!(o.status.success(), "hook failed ({:?}): {}", o.status.code(), stderr(o));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "stdout must be one line, got {out:?}");
    let p = PathBuf::from(lines[0]);
    assert!(p.is_absolute(), "{p:?} is not absolute");
    p
}

#[test]
fn create_then_repeat_then_remove_then_repeat() {
    let s = Setup::new();
    // The orchestrator's worktree already holds an edit and a new file: they seed the change.
    std::fs::write(s.worktree.join("f00.txt"), "seeded edit\n").unwrap();
    std::fs::write(s.worktree.join("new.txt"), "new file\n").unwrap();

    let created = s.run(&["create", "--from-dir", s.worktree.to_str().unwrap(), "--intent", "login form"]);
    let dir = one_line_path(&created);
    assert!(dir.is_dir());
    assert_eq!(dir.file_name().unwrap(), "feat-login", "default name = worktree base name");
    assert_eq!(std::fs::read_to_string(dir.join("f00.txt")).unwrap(), "seeded edit\n");
    assert!(dir.join(".git").exists(), "--git is the default");
    let open = s.open_changes();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["agent"], "hook-feat-login");
    assert_eq!(open[0]["intent"], "login form");
    let mut paths: Vec<String> =
        open[0]["paths"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
    paths.sort();
    assert_eq!(paths, ["f00.txt", "new.txt"], "the seed reached the change");

    // A repeated create (an orchestrator retry) prints the same path and opens no second change.
    let again = s.run(&["create", "--from-dir", s.worktree.to_str().unwrap()]);
    assert_eq!(one_line_path(&again), dir);
    assert_eq!(s.open_changes().len(), 1);

    // An edit in the workspace that no sync sent yet: remove sends it before the directory goes.
    std::fs::write(dir.join("late.txt"), "late\n").unwrap();
    let removed = s.run(&["remove", "--from-dir", s.worktree.to_str().unwrap()]);
    assert_eq!(one_line_path(&removed), dir);
    assert!(!dir.exists());
    let open = s.open_changes();
    assert_eq!(open.len(), 1, "the change stays open");
    assert!(open[0]["paths"].as_array().unwrap().contains(&json!("late.txt")), "{:?}", open[0]["paths"]);
    assert!(s.worktree.join("new.txt").exists(), "the orchestrator's worktree is untouched");

    // A repeated remove is not an error, and prints nothing on stdout.
    let again = s.run(&["remove", "feat-login"]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert!(stdout(&again).is_empty());
    assert!(stderr(&again).contains("not found"));
}

#[test]
fn env_only_create_and_remove_by_path_with_abandon() {
    let s = Setup::new();
    let vars = [
        ("TREEYARD_URL", s.url.as_str()),
        ("TREEYARD_HOME", s.node.to_str().unwrap()),
        ("TREEYARD_HOOK_NAME", "agent-7"),
        ("TREEYARD_AGENT", "codex-7"),
        ("TREEYARD_INTENT", "fix the parser"),
        ("TREEYARD_SETUP", "echo ready > setup.txt"),
    ];
    let o = hook(&s.repo, &s.home(), &vars, &["create", "--no-git"]);
    let dir = one_line_path(&o);
    assert_eq!(dir.file_name().unwrap(), "agent-7");
    assert_eq!(std::fs::read_to_string(dir.join("setup.txt")).unwrap(), "ready\n");
    assert!(!dir.join(".git").exists());
    let open = s.open_changes();
    assert_eq!((open[0]["agent"].as_str(), open[0]["intent"].as_str()), (Some("codex-7"), Some("fix the parser")));

    let o = s.run(&["remove", dir.to_str().unwrap(), "--abandon"]);
    assert_eq!(one_line_path(&o), dir);
    assert!(s.open_changes().is_empty(), "--abandon drops the change");
}

#[test]
fn missing_server_writes_nothing() {
    let s = Setup::new();
    // A port that was free a moment ago: nothing listens there.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dead = format!("http://127.0.0.1:{port}");
    let vars = [("TREEYARD_URL", dead.as_str()), ("TREEYARD_HOME", s.node.to_str().unwrap())];
    let o = hook(&s.worktree, &s.home(), &vars, &["create", "--name", "ghost"]);
    assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
    assert!(stdout(&o).is_empty());
    assert!(stderr(&o).contains("does not answer"), "{}", stderr(&o));
    assert!(!s.node.join("meta").join("ghost.json").exists());
    assert!(!s.node.join("ws").join("ghost").exists());
    assert!(s.open_changes().is_empty());

    // remove refuses too (the last edits could not be sent), unless --force.
    let dir = one_line_path(&s.run(&["create", "--name", "ghost"]));
    let o = hook(&s.worktree, &s.home(), &vars, &["remove", "ghost"]);
    assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
    assert!(dir.exists(), "nothing deleted");
    let o = hook(&s.worktree, &s.home(), &vars, &["remove", "ghost", "--force"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!dir.exists());
}

#[test]
fn no_url_and_no_connect_state_is_exit_2() {
    let s = Setup::new();
    let vars = [("TREEYARD_HOME", s.node.to_str().unwrap())];
    let o = hook(&s.worktree, &s.home(), &vars, &["create"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stdout(&o).is_empty());
    assert!(stderr(&o).contains("TREEYARD_URL"));
    assert!(!s.node.join("meta").exists());

    let o = s.run(&["create", "--name", "a/b"]);
    assert_eq!(o.status.code(), Some(2), "a bad name is a usage error: {}", stderr(&o));
}

#[test]
fn connect_state_gives_the_url_in_a_linked_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let repo = fixture_repo(&root, 2, 5, &[]);
    let (home, node) = (root.join("home"), root.join("node"));
    std::fs::create_dir_all(&home).unwrap();
    let base = |args: &[&str], dir: &Path| {
        let mut c = Command::new(bin());
        c.args(args).current_dir(dir).env_clear().stdin(Stdio::null());
        c.env("PATH", std::env::var_os("PATH").unwrap_or_default()).env("HOME", &home);
        c.env("CODEX_HOME", root.join("codex")).env("XDG_CONFIG_HOME", root.join("xdg"));
        c.output().unwrap()
    };
    let o = base(&["connect", "--harness", "claude", "--no-mirror"], &repo);
    assert!(o.status.success(), "connect: {}", String::from_utf8_lossy(&o.stderr));
    let wt = root.join("wt-a");
    git(&repo, &["worktree", "add", "-q", "-b", "wt-a", wt.to_str().unwrap()]);
    let vars = [("TREEYARD_HOME", node.to_str().unwrap())];
    let created = hook(&wt, &home, &vars, &["create"]);
    let removed = hook(&wt, &home, &vars, &["remove"]);
    let off = base(&["disconnect", "--purge"], &repo);
    let dir = one_line_path(&created);
    assert_eq!(dir.file_name().unwrap(), "wt-a");
    assert_eq!(one_line_path(&removed), dir);
    assert!(off.status.success(), "{}", String::from_utf8_lossy(&off.stderr));
}
