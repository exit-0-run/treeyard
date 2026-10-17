//! Phase-1 gate: 50 agents open changes on the same main at once, edit, and land through the
//! API. Every server-side merge is checked against `git merge-tree` on the exported commits.

mod common;

use common::*;
use rand::{Rng, SeedableRng, rngs::StdRng};
use rootstock::repo::Repo;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};

const AGENTS: usize = 50;
const FILES: usize = 12;
const LINES: usize = 80;

struct Merge {
    base: String,
    ours: String,
    theirs: String,
    /// The landed commit when the server merged cleanly.
    landed: Option<String>,
}

struct Report {
    file: String,
    attempts: usize,
    merges: Vec<Merge>,
}

fn tag(text: &str, line: usize, agent: usize) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[line] = format!("agent-{agent:02}: {}", lines[line]);
    lines.join("\n") + "\n"
}

fn agent(url: String, i: usize, start: Arc<Barrier>) -> Report {
    let mut rng = StdRng::seed_from_u64(7 + i as u64);
    let file = format!("f{:02}.txt", rng.gen_range(0..FILES));
    let line = rng.gen_range(0..LINES);
    let ch = call(
        &url,
        "open_change",
        json!({"agent": format!("agent-{i:02}"), "intent": format!("tag line {line} of {file}")}),
    );
    let id = ch["id"].as_str().unwrap().to_string();
    // Everyone opens on the same main, so all but the first land on a stale base.
    start.wait();
    let text = call(&url, "read", json!({"path": file, "at": id}))["content"].as_str().unwrap().to_string();
    call(&url, "apply", json!({"change": id, "edits": [{"path": file, "content": tag(&text, line, i)}]}));

    let mut merges = Vec::new();
    for attempt in 1..=20 {
        let r = call(&url, "land", json!({"change": id}));
        let landed = r["status"] == "landed";
        if let Value::Object(m) = &r["merged_from"] {
            merges.push(Merge {
                base: m["base"].as_str().unwrap().into(),
                ours: m["ours"].as_str().unwrap().into(),
                theirs: m["theirs"].as_str().unwrap().into(),
                landed: landed.then(|| r["commit"].as_str().unwrap().to_string()),
            });
        }
        if landed {
            return Report { file, attempts: attempt, merges };
        }
        assert_eq!(r["status"], "conflict", "{r}");
        // The resolver: take their side and re-apply our own tag on top, so both survive.
        let task = &r["change"]["conflicts"][0];
        assert_eq!(task["path"], file.as_str());
        assert_eq!(task["kind"], "content");
        assert!(!task["theirs_by"].as_array().unwrap().is_empty(), "the task names who touched the path");
        let theirs = task["text"]["theirs"].as_str().unwrap();
        call(&url, "apply", json!({"change": id, "edits": [{"path": file, "content": tag(theirs, line, i)}]}));
    }
    panic!("agent {i} did not land in 20 attempts");
}

#[test]
fn fifty_agents_land_and_git_agrees_with_every_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let src = fixture_repo(tmp.path(), FILES, LINES, &[]);
    let data = tmp.path().join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let url = spawn_server(Arc::new(Repo::open(&data).unwrap()));

    let start = Arc::new(Barrier::new(AGENTS));
    let handles: Vec<_> = (0..AGENTS)
        .map(|i| {
            let (url, start) = (url.clone(), start.clone());
            std::thread::spawn(move || agent(url, i, start))
        })
        .collect();
    let reports: Vec<Report> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let gd = data.join("git");
    let gd_s = gd.to_str().unwrap();
    let dir = tmp.path();
    let (ok, _, err) = git_status(dir, &["--git-dir", gd_s, "fsck", "--strict", "--no-progress"]);
    assert!(ok, "fsck: {err}");
    let count = git(dir, &["--git-dir", gd_s, "rev-list", "--first-parent", "--count", "main"]);
    assert_eq!(count.trim().parse::<usize>().unwrap(), 1 + AGENTS, "one commit per landed change on top of the import");

    // Differential check: each server rebase against git's own merge of the same three commits.
    let (mut clean, mut both_clean, mut server_conflicts, mut git_cleaner) = (0, 0, 0, 0);
    for m in reports.iter().flat_map(|r| &r.merges) {
        let base = format!("--merge-base={}", m.base);
        let (git_clean, out, _) =
            git_status(dir, &["--git-dir", gd_s, "merge-tree", "--write-tree", &base, &m.theirs, &m.ours]);
        match &m.landed {
            Some(landed) => {
                clean += 1;
                assert!(git_clean, "server merged cleanly but git reports a conflict for {landed}");
                let tree = git(dir, &["--git-dir", gd_s, "rev-parse", &format!("{landed}^{{tree}}")]);
                assert_eq!(out.lines().next().unwrap(), tree.trim(), "tree of {landed} differs from git merge-tree");
                both_clean += 1;
            }
            None => {
                server_conflicts += 1;
                if git_clean {
                    git_cleaner += 1;
                }
            }
        }
    }

    // Every agent's tag survived, including the ones that went through a conflict.
    let clone = dir.join("clone");
    git(dir, &["clone", "-q", gd_s, clone.to_str().unwrap()]);
    for (i, r) in reports.iter().enumerate() {
        let text = std::fs::read_to_string(clone.join(&r.file)).unwrap();
        assert!(text.contains(&format!("agent-{i:02}: ")), "agent {i}'s edit is missing from {}", r.file);
        assert!(!text.contains("<<<<<<<"), "conflict markers landed in {}", r.file);
    }

    let attempts: usize = reports.iter().map(|r| r.attempts).sum();
    eprintln!(
        "swarm: {AGENTS} agents landed in {attempts} land calls; {clean} clean server merges, all {both_clean} identical to git merge-tree; \
         {server_conflicts} conflicts handed back as tasks ({git_cleaner} of them git would have merged)"
    );
    assert!(clean > 0 && server_conflicts > 0, "the run should exercise both merges and conflicts");
}
