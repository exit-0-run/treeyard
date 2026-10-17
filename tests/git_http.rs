//! People use plain git against the server: clone, fetch and push over smart HTTP, and a
//! mirror to a read-only remote. Pushes land through the same path as agents' changes.

mod common;

use common::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use treeyard::repo::Repo;

struct Setup {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    url: String,
}

fn setup(files: usize, lines: usize) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let src = fixture_repo(&root, files, lines, &[]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    Setup { _tmp: tmp, root, data, url }
}

/// git with a human identity, no prompts; (success, stdout, stderr).
fn hgit(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "user.name=Human Dev", "-c", "user.email=human@example.com", "-c", "commit.gpgsign=false"])
        .args(["-c", "pull.rebase=true", "-c", "advice.pushFetchFirst=false"])
        .args(args)
        .output()
        .expect("git runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn hgit_ok(dir: &Path, args: &[&str]) -> String {
    let (ok, out, err) = hgit(dir, args);
    assert!(ok, "git {} failed: {err}", args.join(" "));
    format!("{out}{err}")
}

impl Setup {
    fn clone(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        hgit_ok(&self.root, &["clone", "-q", &format!("{}/git", self.url), dir.to_str().unwrap()]);
        dir
    }
    fn export(&self, args: &[&str]) -> String {
        let gd = self.data.join("git");
        let mut full = vec!["--git-dir", gd.to_str().unwrap()];
        full.extend_from_slice(args);
        git(&self.root, &full).trim().to_string()
    }
    fn fsck(&self) {
        let gd = self.data.join("git");
        let (ok, _, err) =
            git_status(&self.root, &["--git-dir", gd.to_str().unwrap(), "fsck", "--strict", "--no-progress"]);
        assert!(ok, "fsck: {err}");
    }
    fn agent_change(&self, agent: &str, edits: serde_json::Value) -> String {
        let id = call(&self.url, "open_change", json!({"agent": agent, "intent": format!("{agent} work")}))["id"]
            .as_str()
            .unwrap()
            .to_string();
        call(&self.url, "apply", json!({"change": id, "edits": edits}));
        id
    }
}

fn write(dir: &Path, path: &str, body: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[test]
fn clone_then_push_lands_and_agents_read_it() {
    let s = setup(3, 5);
    let work = s.clone("work");
    assert_eq!(hgit_ok(&work, &["rev-parse", "HEAD"]).trim(), s.export(&["rev-parse", "main"]));
    assert!(work.join("f02.txt").exists());

    write(&work, "docs/human.md", "written by a person\n");
    hgit_ok(&work, &["add", "docs/human.md"]);
    hgit_ok(&work, &["commit", "-q", "-m", "Add human doc"]);
    write(&work, "f00.txt", "rewritten by a person\n");
    hgit_ok(&work, &["commit", "-q", "-am", "Rewrite f00\n\nWith a body."]);
    let out = hgit_ok(&work, &["push", "origin", "main"]);
    assert!(out.contains("treeyard: landed 2 commit(s) on main"), "{out}");

    // The pushed commits are main as they are (no rewrite), so the person stays in sync.
    let head = hgit_ok(&work, &["rev-parse", "HEAD"]);
    assert_eq!(s.export(&["rev-parse", "main"]), head.trim());
    let status = call(&s.url, "status", json!({}));
    assert_eq!(status["commit"], head.trim());
    assert_eq!(call(&s.url, "read", json!({"path": "docs/human.md"}))["content"], "written by a person\n");
    let log = call(&s.url, "log", json!({"limit": 2}));
    assert_eq!(log["entries"][0]["agent"], "git:Human Dev");
    assert_eq!(log["entries"][0]["intent"], "Rewrite f00");
    assert_eq!(log["entries"][0]["message"], "Rewrite f00\n\nWith a body.");
    assert_eq!(log["entries"][1]["intent"], "Add human doc");
    let ch = log["entries"][0]["change"].as_str().unwrap();
    assert_eq!(call(&s.url, "change", json!({"change": ch}))["state"], "landed");
    assert_eq!(call(&s.url, "grep", json!({"pattern": "by a person"}))["matches"].as_array().unwrap().len(), 2);

    // A second clone fetches what the first one pushed.
    let other = s.clone("other");
    assert_eq!(std::fs::read_to_string(other.join("f00.txt")).unwrap(), "rewritten by a person\n");
    // An empty push is a no-op.
    assert!(hgit_ok(&work, &["push", "origin", "main"]).contains("Everything up-to-date"));
    s.fsck();
}

#[test]
fn non_fast_forward_push_is_rejected_with_a_hint() {
    let s = setup(2, 5);
    let work = s.clone("work");
    let id = s.agent_change("agent-a", json!([{"path": "agent.txt", "content": "agent\n"}]));
    assert_eq!(call(&s.url, "land", json!({"change": id}))["status"], "landed");
    let main_before = s.export(&["rev-parse", "main"]);

    write(&work, "f01.txt", "stale edit\n");
    hgit_ok(&work, &["commit", "-q", "-am", "Stale edit"]);
    // git itself refuses: the advertised main is unknown to this clone.
    let (ok, _, err) = hgit(&work, &["push", "origin", "main"]);
    assert!(!ok && err.contains("[rejected]"), "{err}");
    // A forced push reaches the server, which refuses to rewrite main.
    let (ok, _, err) = hgit(&work, &["push", "--force", "origin", "main"]);
    assert!(!ok, "forced push must fail");
    println!("forced push stderr:\n{err}");
    assert!(err.contains("non-fast-forward") && err.contains("git pull --rebase"), "{err}");
    assert!(err.contains("[remote rejected]"), "{err}");
    assert_eq!(s.export(&["rev-parse", "main"]), main_before, "main is untouched");

    // Other refs and deleting main are refused too.
    let (ok, _, err) = hgit(&work, &["push", "origin", "HEAD:refs/heads/feature"]);
    assert!(!ok && err.contains("accepts pushes to refs/heads/main only"), "{err}");
    let (ok, _, err) = hgit(&work, &["push", "origin", ":main"]);
    assert!(!ok && err.contains("not allowed"), "{err}");

    // Fetch and rebase, then the push lands on top of the agent's work.
    hgit_ok(&work, &["pull", "-q", "--rebase", "origin", "main"]);
    let out = hgit_ok(&work, &["push", "origin", "main"]);
    assert!(out.contains("landed 1 commit(s)"), "{out}");
    assert_eq!(s.export(&["rev-parse", "main^"]), main_before);
    assert_eq!(call(&s.url, "read", json!({"path": "agent.txt"}))["content"], "agent\n");
    s.fsck();
}

#[test]
fn agent_land_after_a_human_push_stays_consistent() {
    let s = setup(2, 8);
    let work = s.clone("work");
    // The agent starts before the push, on the old main, and edits the top of f00.
    let id = s
        .agent_change("agent-a", json!([{"path": "f00.txt", "replace": {"old": "file 0 line 0\n", "new": "AGENT\n"}}]));
    let base_commit = s.export(&["rev-parse", "main"]);
    let ours = call(&s.url, "export_change", json!({"change": id}))["commit"].as_str().unwrap().to_string();

    // The person edits the bottom of the same file and adds a file, then pushes.
    let body = std::fs::read_to_string(work.join("f00.txt")).unwrap().replace("file 0 line 7\n", "HUMAN\n");
    write(&work, "f00.txt", &body);
    write(&work, "human.txt", "h\n");
    hgit_ok(&work, &["add", "-A"]);
    hgit_ok(&work, &["commit", "-q", "-m", "Human edit"]);
    hgit_ok(&work, &["push", "-q", "origin", "main"]);
    let theirs = s.export(&["rev-parse", "main"]);
    // Agents see the push at once.
    assert_eq!(call(&s.url, "read", json!({"path": "human.txt"}))["content"], "h\n");

    let landed = call(&s.url, "land", json!({"change": id}));
    assert_eq!(landed["status"], "landed", "{landed}");
    assert_eq!(landed["merged_from"]["theirs"], theirs.as_str());
    let (_, expect, _) = git_status(
        &s.root,
        &[
            "--git-dir",
            s.data.join("git").to_str().unwrap(),
            "-c",
            "merge.renames=false",
            "merge-tree",
            "--write-tree",
            "--merge-base",
            &base_commit,
            &theirs,
            &ours,
        ],
    );
    assert_eq!(s.export(&["rev-parse", "main^{tree}"]), expect.lines().next().unwrap(), "same tree as git merge");
    assert_eq!(s.export(&["rev-parse", "main^"]), theirs, "the agent's commit sits on the human's");

    // The person pulls with a fast-forward and has both edits.
    let out = hgit_ok(&work, &["pull", "--ff-only", "origin", "main"]);
    assert!(out.contains("Fast-forward"), "{out}");
    let f00 = std::fs::read_to_string(work.join("f00.txt")).unwrap();
    assert!(f00.starts_with("AGENT\n") && f00.contains("HUMAN\n"), "{f00}");
    s.fsck();
}

#[test]
fn push_of_a_merge_from_a_side_branch_lands() {
    let s = setup(2, 5);
    let work = s.clone("work");
    write(&work, "side.txt", "side\n");
    hgit_ok(&work, &["add", "side.txt"]);
    hgit_ok(&work, &["commit", "-q", "-m", "Side work"]);
    let id = s.agent_change("agent-a", json!([{"path": "agent.txt", "content": "agent\n"}]));
    call(&s.url, "land", json!({"change": id}));
    // Merge main into the local work: the merge's first parent is not on main.
    hgit_ok(&work, &["fetch", "-q", "origin"]);
    hgit_ok(&work, &["merge", "-q", "--no-edit", "--no-ff", "origin/main"]);
    let out = hgit_ok(&work, &["push", "origin", "main"]);
    assert!(out.contains("landed 1 commit(s)"), "{out}");
    assert_eq!(s.export(&["rev-parse", "main"]), hgit_ok(&work, &["rev-parse", "HEAD"]).trim());
    assert_eq!(call(&s.url, "read", json!({"path": "side.txt"}))["content"], "side\n");
    assert_eq!(call(&s.url, "read", json!({"path": "agent.txt"}))["content"], "agent\n");
    // A later agent change lands on top of the merge.
    let id = s.agent_change("agent-b", json!([{"path": "f00.txt", "content": "b\n"}]));
    assert_eq!(call(&s.url, "land", json!({"change": id}))["status"], "landed");
    s.fsck();
}

#[test]
fn agents_and_people_land_together() {
    let s = setup(1, 3);
    let url = s.url.clone();
    let agents: Vec<_> = (0..8)
        .map(|a| {
            let url = url.clone();
            std::thread::spawn(move || {
                for k in 0..3 {
                    let id = call(
                        &url,
                        "open_change",
                        json!({"agent": format!("agent-{a}"), "intent": format!("a{a} k{k}")}),
                    )["id"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    let edit = json!([{"path": format!("agents/{a}/{k}.txt"), "content": format!("{a} {k}\n")}]);
                    call(&url, "apply", json!({"change": id, "edits": edit}));
                    assert_eq!(call(&url, "land", json!({"change": id}))["status"], "landed");
                }
            })
        })
        .collect();
    let work = s.clone("work");
    for k in 0..3 {
        write(&work, &format!("people/{k}.txt"), &format!("person {k}\n"));
        hgit_ok(&work, &["add", "-A"]);
        hgit_ok(&work, &["commit", "-q", "-m", &format!("Person {k}")]);
        let mut tries = 0;
        while !hgit(&work, &["push", "-q", "origin", "main"]).0 {
            tries += 1;
            assert!(tries < 50, "push never landed");
            hgit_ok(&work, &["pull", "-q", "--rebase", "origin", "main"]);
        }
    }
    agents.into_iter().for_each(|t| t.join().unwrap());
    let files = s.export(&["ls-tree", "-r", "--name-only", "main"]);
    assert_eq!(files.lines().filter(|l| l.starts_with("agents/")).count(), 24);
    assert_eq!(files.lines().filter(|l| l.starts_with("people/")).count(), 3);
    let subjects = s.export(&["log", "--first-parent", "--format=%s", "main"]);
    assert_eq!(subjects.lines().filter(|l| l.starts_with("Person ")).count(), 3);
    assert_eq!(call(&s.url, "status", json!({}))["commit"].as_str().unwrap(), s.export(&["rev-parse", "main"]));
    s.fsck();
}

#[test]
fn mirror_pushes_main_to_a_bare_remote() {
    let s = setup(1, 3);
    let mirror = s.root.join("mirror.git");
    git(&s.root, &["init", "-q", "--bare", "-b", "main", mirror.to_str().unwrap()]);
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_treeyard"))
            .args(["mirror", "--data", s.data.to_str().unwrap(), "--remote", mirror.to_str().unwrap()])
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap()
    };
    let mirror_main = || git(&s.root, &["--git-dir", mirror.to_str().unwrap(), "rev-parse", "main"]).trim().to_string();

    let out = run();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(mirror_main(), s.export(&["rev-parse", "main"]));
    let id = s.agent_change("agent-a", json!([{"path": "new.txt", "content": "n\n"}]));
    call(&s.url, "land", json!({"change": id}));
    let out = run();
    assert!(String::from_utf8_lossy(&out.stdout).contains("mirrored"), "{out:?}");
    assert_eq!(mirror_main(), s.export(&["rev-parse", "main"]));
    let (ok, _, err) =
        git_status(&s.root, &["--git-dir", mirror.to_str().unwrap(), "fsck", "--strict", "--no-progress"]);
    assert!(ok, "mirror fsck: {err}");

    // Someone writes to the mirror directly: the next mirror push fails and says why.
    let side = s.root.join("side");
    git(&s.root, &["clone", "-q", mirror.to_str().unwrap(), side.to_str().unwrap()]);
    write(&side, "x.txt", "x\n");
    git(&side, &["add", "x.txt"]);
    git(&side, &["commit", "-q", "-m", "direct"]);
    git(&side, &["push", "-q", "origin", "main"]);
    let id = s.agent_change("agent-b", json!([{"path": "new2.txt", "content": "n\n"}]));
    call(&s.url, "land", json!({"change": id}));
    let out = run();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not read-only"), "{out:?}");
}

#[test]
fn dumb_http_is_refused() {
    let s = setup(1, 1);
    let r = ureq::get(&format!("{}/git/info/refs", s.url)).call();
    match r {
        Err(ureq::Error::Status(403, resp)) => assert!(resp.into_string().unwrap().contains("smart HTTP")),
        other => panic!("expected 403, got {other:?}"),
    }
}

/// Round trip on a real repository: `TREEYARD_REAL_REPO=<git repo> cargo test --release
/// --test git_http -- --ignored --nocapture`. Times clone over TreeYard HTTP against a plain
/// `git clone --no-local` of the same export repo, then a push of one commit.
#[test]
#[ignore]
fn real_repo_round_trip() {
    let Ok(src) = std::env::var("TREEYARD_REAL_REPO") else { return };
    let tmp = tempfile::tempdir_in(std::env::var("TREEYARD_SCRATCH").unwrap_or_else(|_| "/tmp".into())).unwrap();
    let root = tmp.path().to_path_buf();
    let data = root.join("data");
    let branch = git(Path::new(&src), &["symbolic-ref", "--short", "HEAD"]).trim().to_string();
    Repo::init(&data, Path::new(&src), &branch, None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));
    let gd = data.join("git");
    let t = std::time::Instant::now();
    hgit_ok(&root, &["clone", "-q", "--no-local", gd.to_str().unwrap(), "plain"]);
    let plain = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    hgit_ok(&root, &["clone", "-q", &format!("{url}/git"), "http"]);
    let http = t.elapsed().as_secs_f64();
    let work = root.join("http");
    assert_eq!(hgit_ok(&work, &["rev-parse", "HEAD"]), hgit_ok(&root.join("plain"), &["rev-parse", "HEAD"]));
    write(&work, "TREEYARD_PUSH_TEST.md", "pushed over http\n");
    hgit_ok(&work, &["add", "TREEYARD_PUSH_TEST.md"]);
    hgit_ok(&work, &["commit", "-q", "-m", "Push test"]);
    let t = std::time::Instant::now();
    hgit_ok(&work, &["push", "-q", "origin", &branch]);
    let push = t.elapsed().as_secs_f64();
    assert_eq!(call(&url, "read", json!({"path": "TREEYARD_PUSH_TEST.md"}))["content"], "pushed over http\n");
    let (ok, _, err) = git_status(&root, &["--git-dir", gd.to_str().unwrap(), "fsck", "--no-progress"]);
    let fsck = if ok { "clean".to_string() } else { format!("errors: {}", err.lines().count()) };
    println!("repo={src} clone_plain_s={plain:.3} clone_http_s={http:.3} push_one_commit_s={push:.3} fsck={fsck}");
}
