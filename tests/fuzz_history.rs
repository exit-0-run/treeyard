//! Property test of history operations against git as the oracle. Random agents edit, delete,
//! rename (delete + add), flip modes, add binary, empty, symlink and deep files on fresh and
//! stale bases, rebase, resolve, abandon and land. After every land:
//! - a clean server merge must equal `git merge-tree` of the same three commits;
//! - the exported commit's tree must list exactly the server's manifest (`git ls-tree -r`);
//! - `read` must return the bytes `git cat-file` returns.
//!
//! At the end the export repo passes `git fsck --strict`. `TREEYARD_FUZZ_SEEDS=N` runs N
//! seeds (default 12); `TREEYARD_FUZZ_SEED=S` runs one seed (to replay a failure).

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use common::*;
use rand::{Rng, SeedableRng, rngs::StdRng, seq::SliceRandom};
use treeyard::repo::Repo;
use treeyard::rpc;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

const DIRS: &[&str] = &["a", "b", "src", "deep", "x y", "ünï"];
const NAMES: &[&str] = &["one.txt", "two.rs", "data.bin", "empty", "run.sh", "link", "crlf.txt", "nonl.txt"];
const WORDS: &[&str] = &["alpha", "beta", "gamma", "delta", "fn main() {}", "}", "", "    x += 1;", "// note"];

#[derive(Default)]
struct Stats {
    lands: usize,
    merges: usize,
    clean_equal: usize,
    server_conflict: usize,
    server_conflict_git_clean: usize,
    exports_checked: usize,
    reads_checked: usize,
}

fn rand_path(rng: &mut StdRng) -> String {
    let depth = if rng.gen_bool(0.15) { rng.gen_range(6..14) } else { rng.gen_range(0..3) };
    let mut parts: Vec<&str> = (0..depth).map(|_| *DIRS.choose(rng).unwrap()).collect();
    parts.push(NAMES.choose(rng).unwrap());
    parts.join("/")
}

fn text(rng: &mut StdRng, lines: usize, eol: &str, final_eol: bool) -> String {
    let mut s: String = (0..lines).map(|_| format!("{}{eol}", WORDS.choose(rng).unwrap())).collect();
    if !final_eol && s.ends_with(eol) {
        s.truncate(s.len() - eol.len());
    }
    s
}

/// A new body for `path`: the kind follows the file name.
fn body(rng: &mut StdRng, path: &str) -> Value {
    let name = path.rsplit('/').next().unwrap();
    let n = rng.gen_range(1..30);
    match name {
        "empty" => json!({"content": ""}),
        "data.bin" => {
            let n = rng.gen_range(1..300);
            let mut data: Vec<u8> = (0..n).map(|_| rng.r#gen()).collect();
            data[0] = 0;
            json!({"content_b64": B64.encode(data)})
        }
        "link" => json!({"content": format!("../{}", NAMES.choose(rng).unwrap()), "symlink": true}),
        "run.sh" => json!({"content": format!("#!/bin/sh\n{}", text(rng, 4, "\n", true)), "executable": true}),
        "crlf.txt" => json!({"content": text(rng, n.min(12), "\r\n", true)}),
        "nonl.txt" => json!({"content": text(rng, n.min(12), "\n", false)}),
        _ => json!({"content": text(rng, n, "\n", true)}),
    }
}

/// A line-level edit of a text file: change, insert or delete one to three lines.
fn edit_text(rng: &mut StdRng, old: &str) -> String {
    let mut lines: Vec<String> = old.split_inclusive('\n').map(str::to_string).collect();
    for _ in 0..rng.gen_range(1..4) {
        let at = rng.gen_range(0..=lines.len());
        match rng.gen_range(0..3) {
            0 if at < lines.len() => lines[at] = format!("{}\n", WORDS.choose(rng).unwrap()),
            1 if at < lines.len() => {
                lines.remove(at);
            }
            _ => lines.insert(at, format!("{} {}\n", WORDS.choose(rng).unwrap(), rng.gen_range(0..9))),
        }
    }
    lines.concat()
}

struct World<'a> {
    repo: &'a Repo,
    root: &'a Path,
    gd: String,
    stats: Stats,
}

impl World<'_> {
    fn call(&self, op: &str, p: Value) -> Value {
        rpc::call(self.repo, op, p.clone()).unwrap_or_else(|e| panic!("{op} {p} failed: {e:#}"))
    }

    fn files(&self, at: &str) -> BTreeMap<String, (u64, String)> {
        let t = self.call("tree", json!({"at": at}));
        t["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e[0].as_str().unwrap().to_string(), (e[1].as_u64().unwrap(), e[2].as_str().unwrap().to_string())))
            .collect()
    }

    fn git(&self, args: &[&str]) -> (bool, String, String) {
        let mut full = vec!["--git-dir", self.gd.as_str()];
        full.extend_from_slice(args);
        git_status(self.root, &full)
    }

    /// Random edits for a change whose head has `files`.
    fn edits(&self, rng: &mut StdRng, files: &BTreeMap<String, (u64, String)>, at: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let mut touched = std::collections::BTreeSet::new();
        for _ in 0..rng.gen_range(1..5) {
            let existing: Vec<&String> = files.keys().filter(|p| !touched.contains(*p)).collect();
            let op = rng.gen_range(0..10);
            if existing.is_empty() || op < 2 {
                let path = rand_path(rng);
                if files.contains_key(&path) || !touched.insert(path.clone()) {
                    continue;
                }
                let mut e = body(rng, &path);
                e["path"] = json!(path);
                out.push(e);
                continue;
            }
            let path = existing.choose(rng).unwrap().to_string();
            touched.insert(path.clone());
            let (mode, blob) = &files[&path];
            match op {
                2 => out.push(json!({"path": path, "delete": true})),
                3 if *mode != 0o120000 => out.push(json!({"path": path, "executable": *mode != 0o100755})),
                4 => {
                    // Rename = delete + add of the same bytes (git sees no rename either).
                    let to = rand_path(rng);
                    if files.contains_key(&to) || !touched.insert(to.clone()) {
                        continue;
                    }
                    let (_, data, _) = self.git(&["cat-file", "blob", blob]);
                    out.push(json!({"path": path, "delete": true}));
                    let mut e = body(rng, &to);
                    if *mode != 0o120000 && !to.ends_with("link") && !to.ends_with("data.bin") {
                        e = json!({"content": data});
                    }
                    e["path"] = json!(to);
                    out.push(e);
                }
                5 => {
                    let mut e = body(rng, &path);
                    e["path"] = json!(path);
                    out.push(e);
                }
                _ => {
                    let r = self.call("read", json!({"path": path, "at": at}));
                    match (r["content"].as_str(), *mode) {
                        (Some(t), m) if m != 0o120000 => {
                            out.push(json!({"path": path, "content": edit_text(rng, t)}));
                        }
                        _ => {
                            let mut e = body(rng, &path);
                            e["path"] = json!(path);
                            out.push(e);
                        }
                    }
                }
            }
        }
        out
    }

    /// Checks the land reply against git and the export against the manifest.
    fn check_land(&mut self, r: &Value) {
        if let Value::Object(m) = &r["merged_from"] {
            self.stats.merges += 1;
            let (b, o, t) = (m["base"].as_str().unwrap(), m["ours"].as_str().unwrap(), m["theirs"].as_str().unwrap());
            let base = format!("--merge-base={b}");
            let mut args = vec!["-c", "merge.renames=false", "merge-tree", "--write-tree", &base, t, o];
            let (git_clean, out, err) = self.git(&args);
            assert!(git_clean || out.lines().count() > 1, "merge-tree failed: {err}");
            match r["status"].as_str().unwrap() {
                "landed" | "already_on_main" => {
                    if !git_clean {
                        let mut dump = String::new();
                        for l in out.lines().filter(|l| l.contains('\t')) {
                            let id = l.split(' ').nth(1).unwrap();
                            let (_, body, _) = self.git(&["cat-file", "-p", id]);
                            dump.push_str(&format!("--- {l}\n{body:?}\n"));
                        }
                        let (_, ours, _) = self.git(&[
                            "show",
                            &format!(
                                "{}:{}",
                                r["commit"].as_str().unwrap(),
                                out.lines().find(|l| l.contains('\t')).unwrap().split('\t').nth(1).unwrap()
                            ),
                        ]);
                        panic!(
                            "server merged {b} {o} {t} cleanly, git conflicts:\n{out}\n{dump}server result: {ours:?}"
                        );
                    }
                    let commit = r["commit"].as_str().unwrap();
                    let (_, tree, _) = self.git(&["rev-parse", &format!("{commit}^{{tree}}")]);
                    assert_eq!(out.lines().next().unwrap(), tree.trim(), "tree of {commit} != git merge-tree");
                    self.stats.clean_equal += 1;
                }
                "conflict" => {
                    self.stats.server_conflict += 1;
                    if git_clean {
                        self.stats.server_conflict_git_clean += 1;
                    }
                }
                other => panic!("unexpected land status {other}"),
            }
            args.clear();
        }
        if matches!(r["status"].as_str(), Some("landed")) {
            self.stats.lands += 1;
            self.check_export(r["commit"].as_str().unwrap(), r["changeset"].as_str().unwrap());
        }
    }

    fn check_export(&mut self, commit: &str, changeset: &str) {
        let (ok, out, err) = self.git(&["ls-tree", "-r", "-z", "--full-tree", commit]);
        assert!(ok, "{err}");
        let git_files: BTreeMap<String, (u64, String)> = out
            .split('\0')
            .filter(|r| !r.is_empty())
            .map(|r| {
                let (meta, path) = r.split_once('\t').unwrap();
                let f: Vec<&str> = meta.split(' ').collect();
                (path.to_string(), (u64::from_str_radix(f[0], 8).unwrap(), f[2].to_string()))
            })
            .collect();
        let ours = self.files(changeset);
        // The incremental export wrote the same tree as a full write of the manifest.
        let cs = changeset.to_string();
        self.repo.with(|r| r.verify_export(&cs)).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(git_files, ours, "export of {commit} differs from the manifest of {changeset}");
        self.stats.exports_checked += 1;
        if let Some((path, (mode, blob))) = ours.iter().nth(ours.len() / 2) {
            let r = self.call("read", json!({"path": path, "at": changeset}));
            let got = match (r["content"].as_str(), r["content_b64"].as_str()) {
                (Some(t), _) => t.as_bytes().to_vec(),
                (_, Some(b)) => B64.decode(b).unwrap(),
                _ => panic!("read gave no content: {r}"),
            };
            let out = std::process::Command::new("git")
                .args(["--git-dir", &self.gd, "cat-file", "blob", blob])
                .output()
                .unwrap();
            assert_eq!(got, out.stdout, "read {path} (mode {mode:o}) differs from git");
            self.stats.reads_checked += 1;
        }
    }
}

fn run_seed(seed: u64) -> Stats {
    let mut rng = StdRng::seed_from_u64(seed);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    for i in 0..rng.gen_range(3..12) {
        let path = rand_path(&mut rng);
        let p = src.join(&path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        if path.ends_with("link") {
            let _ = std::os::unix::fs::symlink(format!("../t{i}"), &p);
        } else {
            std::fs::write(&p, text(&mut rng, 20, "\n", true)).unwrap();
        }
    }
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "fixture"]);
    let data = root.join("data");
    Repo::init(&data, &src, "main", None).unwrap();
    let repo = Repo::open(&data).unwrap();
    let mut w =
        World { repo: &repo, root, gd: data.join("git").to_string_lossy().into_owned(), stats: Stats::default() };

    let mut old_mains = vec![w.call("status", json!({}))["main"].as_str().unwrap().to_string()];
    for round in 0..8 {
        let mut open = Vec::new();
        for a in 0..rng.gen_range(2..6) {
            // Some agents start on an older main (a stale base): its merge is bigger.
            let base = if rng.gen_bool(0.3) { old_mains.choose(&mut rng).unwrap().clone() } else { "main".into() };
            let ch = w.call(
                "open_change",
                json!({"agent": format!("s{seed}-r{round}-a{a}"), "intent": "fuzz", "base": base}),
            );
            let id = ch["id"].as_str().unwrap().to_string();
            for _ in 0..rng.gen_range(1..3) {
                let head = w.call("change", json!({"change": id}))["head"].as_str().unwrap().to_string();
                let files = w.files(&head);
                let edits = w.edits(&mut rng, &files, &head);
                if edits.is_empty() {
                    continue;
                }
                rpc::call(&repo, "apply", json!({"change": id, "edits": edits}))
                    .unwrap_or_else(|e| panic!("seed {seed}: apply {edits:?}: {e:#}"));
            }
            open.push(id);
        }
        open.shuffle(&mut rng);
        for id in open {
            if rng.gen_bool(0.2) {
                let _ = rpc::call(&repo, "rebase", json!({"change": id}));
            }
            for attempt in 0..4 {
                let r = match rpc::call(&repo, "land", json!({"change": id})) {
                    Ok(r) => r,
                    Err(e) if format!("{e:#}").contains("is empty") => break,
                    Err(e) => panic!("seed {seed}: land {id}: {e:#}"),
                };
                w.check_land(&r);
                if r["status"] != "conflict" {
                    break;
                }
                if attempt == 3 || rng.gen_bool(0.15) {
                    w.call("abandon", json!({"change": id}));
                    break;
                }
                for c in r["change"]["conflicts"].as_array().unwrap() {
                    let take = ["ours", "theirs", "base", "delete"].choose(&mut rng).unwrap();
                    let p = c["path"].as_str().unwrap();
                    w.call("resolve", json!({"change": id, "path": p, "take": take}));
                }
            }
        }
        old_mains.push(w.call("status", json!({}))["main"].as_str().unwrap().to_string());
    }
    let (ok, out, err) = w.git(&["fsck", "--strict", "--no-progress"]);
    assert!(ok, "seed {seed}: fsck: {out}{err}");
    w.stats
}

#[test]
fn random_histories_match_git() {
    let seeds: Vec<u64> = match std::env::var("TREEYARD_FUZZ_SEED") {
        Ok(s) => vec![s.parse().unwrap()],
        Err(_) => {
            let n = std::env::var("TREEYARD_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
            (1..=n).collect()
        }
    };
    let mut t = Stats::default();
    for seed in &seeds {
        let s = run_seed(*seed);
        t.lands += s.lands;
        t.merges += s.merges;
        t.clean_equal += s.clean_equal;
        t.server_conflict += s.server_conflict;
        t.server_conflict_git_clean += s.server_conflict_git_clean;
        t.exports_checked += s.exports_checked;
        t.reads_checked += s.reads_checked;
    }
    eprintln!(
        "fuzz: {} seeds, {} lands, {} merges: {} clean and equal to git merge-tree, {} server conflicts \
         ({} of them git merged cleanly); {} exports equal to the manifest, {} reads equal to git",
        seeds.len(),
        t.lands,
        t.merges,
        t.clean_equal,
        t.server_conflict,
        t.server_conflict_git_clean,
        t.exports_checked,
        t.reads_checked
    );
    assert!(t.merges > 0 && t.clean_equal > 0);
}
