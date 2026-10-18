//! examples/claude-code: the WorktreeCreate hook makes a TreeYard workspace and prints its path
//! as the last stdout line; the WorktreeRemove hook syncs the edits before it deletes the
//! directory, so the change on the server keeps them. The hook input is the JSON that the
//! Claude Code docs give (code.claude.com/docs/en/hooks, opened 2026-10-17); no agent runs here.

mod common;

use common::{fixture_repo, spawn_server};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use treeyard::repo::Repo;

fn hook(script: &str, input: &Value, url: &str, home: &Path) -> (bool, String, String) {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_treeyard"));
    let path = format!("{}:{}", bin.parent().unwrap().display(), std::env::var("PATH").unwrap_or_default());
    let mut child = Command::new("sh")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/claude-code").join(script))
        .env("PATH", path)
        .env("TREEYARD_URL", url)
        .env("TREEYARD_HOME", home)
        .env_remove("TREEYARD_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn worktree_hooks_create_a_workspace_and_keep_its_edits() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 3, 5, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let home = tmp.path().join("node");

    let input =
        json!({"session_id": "abc123", "cwd": src, "hook_event_name": "WorktreeCreate", "name": "bold-oak-a3f2"});
    let (ok, stdout, stderr) = hook("worktree-create.sh", &input, &url, &home);
    assert!(ok, "create hook failed: {stderr}");
    let dir = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap().trim().to_string();
    assert!(Path::new(&dir).join("f00.txt").is_file(), "the last stdout line must be the workspace: {stdout:?}");
    assert!(Path::new(&dir).join(".git").exists(), "the hook asks for --git, so git status works");

    // The session edits a file, then Claude Code removes the worktree.
    std::fs::write(Path::new(&dir).join("f00.txt"), "edited by the session\n").unwrap();
    let input = json!({"session_id": "abc123", "hook_event_name": "WorktreeRemove", "worktree_path": dir});
    let (ok, _, stderr) = hook("worktree-remove.sh", &input, &url, &home);
    assert!(ok, "remove hook failed: {stderr}");
    assert!(!Path::new(&dir).exists(), "the directory is gone");

    let open = treeyard::client::call(&url, "changes", json!({})).unwrap();
    let ch = open["changes"].as_array().unwrap().iter().find(|c| c["agent"] == "claude-bold-oak-a3f2");
    let ch = ch.unwrap_or_else(|| panic!("the change stays open: {open}"));
    let read = treeyard::client::call(&url, "read", json!({"path": "f00.txt", "at": ch["id"]})).unwrap();
    assert_eq!(read["content"], "edited by the session\n", "the edit survives the removal");

    let bad = json!({"hook_event_name": "WorktreeCreate", "name": "../escape"});
    assert!(!hook("worktree-create.sh", &bad, &url, &home).0, "a name with a slash is refused");
}

#[test]
fn prompt_hook_checkpoints_each_turn_of_a_workspace_session() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), 3, 5, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let home = tmp.path().join("node");
    let input = json!({"hook_event_name": "WorktreeCreate", "name": "calm-fir-01"});
    let (ok, stdout, stderr) = hook("worktree-create.sh", &input, &url, &home);
    assert!(ok, "create hook failed: {stderr}");
    let dir = PathBuf::from(stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap().trim());

    let prompt = |n: usize| {
        let input = json!({"session_id": "s-1", "hook_event_name": "UserPromptSubmit", "cwd": dir.join("sub"),
            "prompt": format!("turn {n}")});
        let (ok, stdout, stderr) = hook("checkpoint.sh", &input, &url, &home);
        assert!(ok, "checkpoint hook failed: {stderr}");
        assert_eq!(stdout, "", "stdout would become model context");
    };
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("f00.txt"), "after turn 1\n").unwrap();
    prompt(2);
    std::fs::write(dir.join("f00.txt"), "after turn 2\n").unwrap();
    prompt(3);
    prompt(3);

    let env = std::fs::read_to_string(dir.join(".treeyard/env")).unwrap();
    let change = env.lines().find_map(|l| l.strip_prefix("TREEYARD_CHANGE=")).unwrap();
    let list = treeyard::client::call(&url, "checkpoints", json!({"change": change})).unwrap();
    let cps = list["checkpoints"].as_array().unwrap();
    assert_eq!(cps.len(), 2, "one checkpoint per turn with edits: {list}");
    assert_eq!(cps[0]["label"], "turn of session s-1");

    // Outside a workspace the hook does nothing and never blocks the prompt.
    let input = json!({"hook_event_name": "UserPromptSubmit", "cwd": tmp.path(), "prompt": "hi"});
    assert!(hook("checkpoint.sh", &input, &url, &home).0);
}
