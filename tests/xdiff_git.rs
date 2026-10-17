//! The xdiff port gives the same hunks as `git diff` on every modified file of a real
//! history. Ignored by default (needs a clone): `TREEYARD_REAL_REPO=/path/to/clone
//! cargo test --release --test xdiff_git -- --ignored --nocapture`.

use std::process::Command;

fn git(repo: &str, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git").arg("-C").arg(repo).args(args).output().expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// Hunks of `git diff -U0` as (old start, old count, new start, new count), 0-based starts
/// as xdiff reports them (git prints start-1 for an empty side).
fn git_hunks(patch: &str) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    for l in patch.lines().filter(|l| l.starts_with("@@ ")) {
        let mut parts = l.split(' ');
        let (a, b) = (parts.nth(1).unwrap(), parts.next().unwrap());
        let parse = |s: &str| {
            let s = &s[1..];
            let (st, n) = s.split_once(',').map_or((s, 1), |(x, y)| (x, y.parse().unwrap()));
            let st: usize = st.parse().unwrap();
            if n == 0 { (st, 0) } else { (st - 1, n) }
        };
        let (a0, an) = parse(a);
        let (b0, bn) = parse(b);
        out.push((a0, an, b0, bn));
    }
    out
}

#[test]
#[ignore]
fn xdiff_matches_git_diff_on_real_history() {
    let repo = std::env::var("TREEYARD_REAL_REPO").unwrap_or("/private/tmp/rootstock-harness/oss/ripgrep".into());
    let n: usize = std::env::var("TREEYARD_COMMITS").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    let log = String::from_utf8(git(&repo, &["rev-list", "--first-parent", &format!("-n{n}"), "HEAD"])).unwrap();
    let (mut pairs, mut bad) = (0, Vec::new());
    for c in log.lines() {
        let raw =
            String::from_utf8(git(&repo, &["diff-tree", "-r", "--no-renames", "--diff-filter=M", &format!("{c}^"), c]))
                .unwrap();
        for l in raw.lines() {
            let meta: Vec<&str> = l.split('\t').next().unwrap().split(' ').collect();
            let path = l.split('\t').nth(1).unwrap();
            let (old, new) = (meta[2], meta[3]);
            let (a, b) = (git(&repo, &["cat-file", "blob", old]), git(&repo, &["cat-file", "blob", new]));
            if a.contains(&0) || b.contains(&0) {
                continue; // binary
            }
            let patch = String::from_utf8_lossy(&git(&repo, &["diff", "-U0", "--no-color", "--no-ext-diff", old, new]))
                .into_owned();
            let want = git_hunks(&patch);
            let got: Vec<_> = treeyard::xdiff::diff(&a, &b)
                .into_iter()
                .map(|h| (h.before.start, h.before.len(), h.after.start, h.after.len()))
                .collect();
            pairs += 1;
            if got != want {
                bad.push(format!("{c} {path}: got {got:?} want {want:?}"));
            }
        }
    }
    println!("{pairs} file pairs, {} differ", bad.len());
    for b in bad.iter().take(5) {
        println!("{}", &b[..b.len().min(600)]);
    }
    assert!(bad.is_empty(), "{} of {pairs} file pairs differ from git diff", bad.len());
}
