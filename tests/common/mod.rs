#![allow(dead_code)]

use treeyard::repo::Repo;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// Runs git in `dir` with a fixed identity; panics on failure.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_status(dir, args);
    assert!(out.0, "git {} failed: {}", args.join(" "), out.2);
    out.1
}

/// (success, stdout, stderr)
pub fn git_status(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A source repo on branch main with `files` text files of `lines` lines, plus `extra` files.
pub fn fixture_repo(root: &Path, files: usize, lines: usize, extra: &[(&str, &str)]) -> PathBuf {
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    for i in 0..files {
        let body: String = (0..lines).map(|j| format!("file {i} line {j}\n")).collect();
        std::fs::write(src.join(format!("f{i:02}.txt")), body).unwrap();
    }
    for (path, body) in extra {
        let p = src.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "fixture"]);
    src
}

/// Serves `repo` on a free local port in a background thread; returns the base URL.
pub fn spawn_server(repo: Arc<Repo>) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, treeyard::server::router(repo)).await.unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

pub fn call(url: &str, op: &str, params: Value) -> Value {
    treeyard::client::call(url, op, params).unwrap_or_else(|e| panic!("{op} failed: {e:#}"))
}
