//! The in-process merge (`treeyard::xmerge`) gives the same text and the same clean/conflict
//! answer as `git merge-file --diff-algorithm histogram` on random three-way edits. Default 600
//! short cases and 150 long ones; `TREEYARD_XMERGE_CASES=100000 cargo test --release --test xmerge_git` for a long run,
//! `TREEYARD_XMERGE_SEED` to start elsewhere. Skips when git is older than 2.44 (no
//! `merge-file --diff-algorithm`).

use std::process::Command;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Few distinct lines, so the diffs meet repeated lines, blank lines and lines without letters.
const POOL: &[&str] =
    &["a", "}", "", "b", "c", "d", "{", "fn x() {", "    return 1;", "    return 2;", "x = 1", "y = 2", "// note", ")"];

fn line(r: &mut Rng, crlf: bool, pool: usize) -> String {
    let l = POOL[r.below(pool)];
    if crlf { format!("{l}\r\n") } else { format!("{l}\n") }
}

fn edit(r: &mut Rng, base: &[String], crlf: bool, pool: usize, edits: usize) -> Vec<String> {
    let mut v = base.to_vec();
    for _ in 0..1 + r.below(edits) {
        let at = r.below(v.len() + 1);
        match r.below(6) {
            0 | 1 => {
                for _ in 0..1 + r.below(3) {
                    v.insert(at, line(r, crlf, pool));
                }
            }
            2 if !v.is_empty() => {
                let end = (at + 1 + r.below(3)).min(v.len());
                v.drain(at.min(v.len() - 1)..end);
            }
            3 if !v.is_empty() => {
                let i = at.min(v.len() - 1);
                v[i] = line(r, crlf, pool);
            }
            4 => v.push(line(r, crlf, pool)),
            _ if v.len() > 2 => {
                let i = r.below(v.len() - 1);
                let block = v[i..i + 2].to_vec();
                for (k, l) in block.into_iter().enumerate() {
                    v.insert((at + k).min(v.len()), l);
                }
            }
            _ => {}
        }
    }
    v
}

fn text(r: &mut Rng, v: &[String]) -> String {
    let mut s: String = v.concat();
    if !s.is_empty() && r.below(8) == 0 {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

fn git_merge(dir: &std::path::Path, base: &str, ours: &str, theirs: &str) -> Option<(bool, String)> {
    let files = [dir.join("main"), dir.join("base"), dir.join("change")];
    for (f, body) in files.iter().zip([theirs, base, ours]) {
        std::fs::write(f, body).unwrap();
    }
    let out = Command::new("git")
        .args(["merge-file", "-p", "--diff-algorithm", "histogram", "-L", "main", "-L", "base", "-L", "change"])
        .args(&files)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    if out.status.code() == Some(129) {
        return None;
    }
    let code = out.status.code().unwrap();
    assert!(code < 128, "git merge-file failed: {}", String::from_utf8_lossy(&out.stderr));
    Some((code == 0, String::from_utf8(out.stdout).unwrap()))
}

/// Runs `cases` random merges: bases of up to `max_lines` lines from the first `pool` lines of POOL.
fn run(cases: usize, seed: u64, max_lines: usize, pool: usize, edits: usize) {
    let dir = tempfile::tempdir().unwrap();
    let (mut clean, mut conflicted) = (0, 0);
    for case in 0..cases {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ (seed + case as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
        r.next();
        let crlf = r.below(10) == 0;
        let base: Vec<String> = (0..r.below(max_lines)).map(|_| line(&mut r, crlf, pool)).collect();
        let ours = edit(&mut r, &base, crlf, pool, edits);
        let theirs = if r.below(5) == 0 { ours.clone() } else { edit(&mut r, &base, crlf, pool, edits) };
        let (b, o, t) = (text(&mut r, &base), text(&mut r, &ours), text(&mut r, &theirs));
        let Some((git_clean, git_text)) = git_merge(dir.path(), &b, &o, &t) else {
            eprintln!("git has no merge-file --diff-algorithm; skipped");
            return;
        };
        let got = treeyard::xmerge::merge(&b, &o, &t).expect("supported");
        let (ok, txt) = match &got {
            Ok(s) => (true, s.as_str()),
            Err(s) => (false, s.as_str()),
        };
        assert!(
            ok == git_clean && txt == git_text,
            "case {case} (seed {seed}) differs from git merge-file\n--- base\n{b:?}\n--- ours (change)\n{o:?}\n--- theirs (main)\n{t:?}\n--- git ({git_clean})\n{git_text:?}\n--- xmerge ({ok})\n{txt:?}"
        );
        if ok { clean += 1 } else { conflicted += 1 }
    }
    eprintln!("{cases} cases: {clean} clean, {conflicted} with conflicts, all equal to git merge-file");
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[test]
fn xmerge_matches_git_merge_file() {
    run(env("TREEYARD_XMERGE_CASES", 600) as usize, env("TREEYARD_XMERGE_SEED", 1), 25, POOL.len(), 4);
}

/// Long files of three distinct lines: lines repeat more than 64 times, so histogram falls back to
/// Myers, and compaction slides groups across many equal lines.
#[test]
fn xmerge_matches_git_on_long_repetitive_files() {
    run(env("TREEYARD_XMERGE_CASES", 600) as usize / 4, env("TREEYARD_XMERGE_SEED", 1), 400, 3, 12);
}
