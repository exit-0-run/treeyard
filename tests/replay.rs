//! Replay on a synthetic linear history: every merge must match git, and stacked landing must
//! reproduce the original tree after every wave.

mod common;

use common::*;
use rand::{Rng, SeedableRng, rngs::StdRng};
use rootstock::replay::{Options, run};

const COMMITS: usize = 60;

fn history(root: &std::path::Path) -> std::path::PathBuf {
    let src = fixture_repo(root, 6, 30, &[]);
    let mut rng = StdRng::seed_from_u64(3);
    for c in 0..COMMITS {
        // Each commit rewrites a few nearby lines of one file, sometimes the line a recent
        // commit touched, so later commits depend on earlier ones.
        let file = src.join(format!("f{:02}.txt", rng.gen_range(0..6)));
        let text = std::fs::read_to_string(&file).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        let at = rng.gen_range(0..lines.len() - 2);
        for l in &mut lines[at..at + rng.gen_range(1..3)] {
            *l = format!("{l} c{c}");
        }
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        git(&src, &["commit", "-q", "-am", &format!("commit {c}")]);
    }
    src
}

fn options(src: &std::path::Path, data: std::path::PathBuf, stacked: bool) -> Options {
    Options {
        src: src.to_path_buf(),
        from: format!("HEAD~{COMMITS}"),
        to: "HEAD".into(),
        agents: 8,
        limit: None,
        seed: 1,
        data,
        check_git: true,
        stacked,
        config: None,
        intents: false,
        wave_base: false,
    }
}

#[test]
fn stale_replay_merges_match_git() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history(tmp.path());
    let r = run(&options(&src, tmp.path().join("stale"), false)).unwrap();
    let g = &r["git_check"];
    assert_eq!(r["commits"], COMMITS);
    assert!(r["clean_merges"].as_u64().unwrap() > 0 && r["conflicts"].as_u64().unwrap() > 0, "{r}");
    assert_eq!(g["tree_differs"].as_array().unwrap().len(), 0, "{r}");
    assert_eq!(g["server_clean_git_conflict"].as_array().unwrap().len(), 0, "{r}");
    assert_eq!(r["fsck_ok"], true, "{r}");
    // A stale-base conflict needs a second change on the same path, so overlaps flags every one.
    assert!(r["first_conflicts"].as_u64().unwrap() > 0, "{r}");
    assert_eq!(r["flagged_early"], r["first_conflicts"], "{r}");
}

#[test]
fn stacked_replay_reproduces_history() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history(tmp.path());
    let r = run(&options(&src, tmp.path().join("stacked"), true)).unwrap();
    assert_eq!(r["faithful_waves"], r["waves"], "{r}");
    assert_eq!(r["final_tree_equal"], true);
    assert!(r["waits"].as_u64().unwrap() > 0, "random landing order must hit the waiting rule: {r}");
    assert_eq!(r["conflicts"], 0);
}

#[test]
fn intents_replay_throws_nothing_away_and_keeps_history() {
    let tmp = tempfile::tempdir().unwrap();
    let src = history(tmp.path());
    let stale = run(&options(&src, tmp.path().join("stale"), false)).unwrap();
    let o = Options { intents: true, ..options(&src, tmp.path().join("intents"), false) };
    let r = run(&o).unwrap();
    assert_eq!(r["conflicts"], 0, "{r}");
    assert_eq!(r["forced"], 0);
    assert_eq!(r["faithful_waves"], r["waves"], "{r}");
    assert!(r["coordination"]["postponed"].as_u64().unwrap() > 0, "{r}");
    assert_eq!(r["git_check"]["server_clean_git_conflict"].as_array().unwrap().len(), 0);
    // Precision of the early warnings is reported next to their recall.
    let w = &stale["warnings"];
    assert!(w["flagged_by_overlaps"].as_u64().unwrap() >= w["flagged_and_conflicted"].as_u64().unwrap());
    assert!(w["trial_merge_precision"].is_number(), "{stale}");
}
