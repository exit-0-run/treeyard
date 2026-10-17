//! Server-side blame along the first-parent history, with the result cached per
//! (changeset, path) and computed from the parent's cached blame (one line diff per step).
//!
//! Lines that reach the import changeset are blamed by `git blame --first-parent` on the
//! import commit, so blame continues into the history git holds. A path that a changeset
//! adds is followed through a rename the way `git blame` does (`git diff-tree -M`).

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

use crate::repo::Inner;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS blame_cache (changeset TEXT NOT NULL, path TEXT NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY (changeset, path)) WITHOUT ROWID;
";

/// Blame of one file version: per line, an origin and the line number in that origin.
/// An origin is a changeset id, or `git:<commit>` for a commit before the import.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Blamed {
    #[serde(rename = "o")]
    pub origins: Vec<String>,
    /// Runs of lines: [origin index, first original line (1-based), count].
    #[serde(rename = "r")]
    pub runs: Vec<[u32; 3]>,
    /// Author, time and summary of `git:` origins, from `git blame --porcelain`.
    #[serde(rename = "g", default, skip_serializing_if = "HashMap::is_empty")]
    pub git: HashMap<String, Value>,
}

impl Blamed {
    fn from_lines(origins: Vec<String>, lines: &[(u32, u32)], git: HashMap<String, Value>) -> Self {
        // Keep only the origins that some line still uses.
        let mut used: Vec<Option<u32>> = vec![None; origins.len()];
        let mut kept = Vec::new();
        let mut runs: Vec<[u32; 3]> = Vec::new();
        for &(o, l) in lines {
            let idx = *used[o as usize].get_or_insert_with(|| {
                kept.push(origins[o as usize].clone());
                kept.len() as u32 - 1
            });
            match runs.last_mut() {
                Some(r) if r[0] == idx && r[1] + r[2] == l => r[2] += 1,
                _ => runs.push([idx, l, 1]),
            }
        }
        let git = git.into_iter().filter(|(k, _)| kept.contains(k)).collect();
        Blamed { origins: kept, runs, git }
    }

    pub fn lines(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for r in &self.runs {
            out.extend((0..r[2]).map(|k| (r[0], r[1] + k)));
        }
        out
    }
}

/// Line hunks from `a` to `b`, the ones `git diff` finds (see `xdiff`).
pub fn line_hunks(a: &[u8], b: &[u8]) -> Vec<crate::xdiff::Hunk> {
    crate::xdiff::diff(a, b)
}

fn line_count(data: &[u8]) -> usize {
    crate::xdiff::lines(data).len()
}

/// Carries `prev` (the blame of `a`) over to `b`; lines that the diff adds get origin `me`.
fn carry(prev: &Blamed, a: &[u8], b: &[u8], me: &str) -> Blamed {
    let old = prev.lines();
    let mut origins = prev.origins.clone();
    let mine = origins.iter().position(|o| o == me).unwrap_or_else(|| {
        origins.push(me.to_string());
        origins.len() - 1
    }) as u32;
    let mut out = Vec::with_capacity(line_count(b));
    let (mut ia, mut ib) = (0usize, 0usize);
    for h in line_hunks(a, b) {
        while ib < h.after.start {
            out.push(old[ia]);
            ia += 1;
            ib += 1;
        }
        out.extend(h.after.clone().map(|k| (mine, k as u32 + 1)));
        (ia, ib) = (h.before.end, h.after.end);
    }
    out.extend_from_slice(&old[ia..]);
    Blamed::from_lines(origins, &out, prev.git.clone())
}

/// `git blame --first-parent --porcelain` of `path` at `commit`; lines from `commit` itself
/// get origin `root` (the import changeset).
fn git_blame(gd: &std::path::Path, commit: &str, path: &str, root: &str) -> Result<Blamed> {
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(gd)
        .args(["blame", "--first-parent", "--porcelain", commit, "--", path])
        .output()
        .context("running git blame")?;
    if !out.status.success() {
        bail!("git blame {commit} -- {path} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut origins: Vec<String> = Vec::new();
    let mut index: HashMap<String, u32> = HashMap::new();
    let mut lines = Vec::new();
    let mut meta: HashMap<String, Map<String, Value>> = HashMap::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        if line.starts_with('\t') {
            continue;
        }
        let mut parts = line.split(' ');
        let first = parts.next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let orig: u32 = parts.next().and_then(|s| s.parse().ok()).ok_or_else(|| anyhow!("bad porcelain line"))?;
            let key = if first == commit { root.to_string() } else { format!("git:{first}") };
            let n = origins.len() as u32;
            let idx = *index.entry(key.clone()).or_insert_with(|| {
                origins.push(key.clone());
                n
            });
            lines.push((idx, orig));
            cur = Some(key);
        } else if let Some(k) = cur.as_ref().filter(|k| k.starts_with("git:")) {
            let (field, value) = (first, line.get(first.len() + 1..).unwrap_or(""));
            let m = meta.entry(k.clone()).or_default();
            match field {
                "author" => drop(m.insert("author".into(), json!(value))),
                "author-time" => drop(m.insert("time".into(), json!(value.parse::<i64>().unwrap_or(0)))),
                "summary" => drop(m.insert("summary".into(), json!(value))),
                _ => {}
            }
        }
    }
    let git = meta.into_iter().map(|(k, v)| (k, Value::Object(v))).collect();
    Ok(Blamed::from_lines(origins, &lines, git))
}

/// The path that `cs` renamed to `path`, found the way `git blame` does it (`find_rename`):
/// rename detection against the first parent where the sources are the files `cs` deleted
/// and the only destination is `path` (git's `single_follow`). A pathspec of `path` plus the
/// deleted files gives `git diff-tree -M` exactly that set.
fn rename_source(r: &mut Inner, parent: &str, cs: &str, path: &str) -> Result<Option<String>> {
    let deleted: Vec<String> =
        r.changeset(cs)?.changes.into_iter().filter(|(_, e)| e.is_none()).map(|(p, _)| p).collect();
    if deleted.is_empty() {
        return Ok(None);
    }
    let (pc, cc) = (r.commit_for(parent)?, r.commit_for(cs)?);
    let mut args = vec!["--literal-pathspecs", "--git-dir"];
    let gd = r.odb.dir().to_string_lossy().into_owned();
    args.push(&gd);
    args.extend(["diff-tree", "-r", "-M", "-z", "--diff-filter=R", &pc, &cc, "--", path]);
    // Very large deletions would pass the argument limit: then all paths are candidates.
    if deleted.len() <= 4000 {
        args.extend(deleted.iter().map(String::as_str));
    }
    let out = std::process::Command::new("git").args(&args).output().context("running git diff-tree")?;
    if !out.status.success() {
        bail!("git diff-tree -M failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let out = String::from_utf8_lossy(&out.stdout);
    let mut f = out.split('\0').filter(|s| !s.is_empty());
    while let (Some(_meta), Some(src), Some(dst)) = (f.next(), f.next(), f.next()) {
        if dst == path {
            return Ok(Some(src.to_string()));
        }
    }
    Ok(None)
}

impl Inner {
    fn blame_cached(&self, cs: &str, path: &str) -> Result<Option<Blamed>> {
        let data: Option<String> = self
            .store
            .conn()
            .query_row("SELECT data FROM blame_cache WHERE changeset = ?1 AND path = ?2", params![cs, path], |r| {
                r.get(0)
            })
            .optional()?;
        data.map(|d| serde_json::from_str(&d).map_err(Into::into)).transpose()
    }

    fn blame_store(&self, cs: &str, path: &str, b: &Blamed) -> Result<()> {
        self.store.conn().execute(
            "INSERT OR REPLACE INTO blame_cache (changeset, path, data) VALUES (?1, ?2, ?3)",
            params![cs, path, serde_json::to_string(b)?],
        )?;
        Ok(())
    }

    fn file_bytes(&mut self, cs: &str, path: &str) -> Result<Vec<u8>> {
        let e = self.manifest(cs)?.get(path).cloned().with_context(|| format!("no such file {path} at {cs}"))?;
        self.blob(&e.blob)
    }

    /// Blame of `path` as of changeset `at`: (the changeset that last touched it, its blame).
    pub fn blame_of(&mut self, path: &str, at: &str) -> Result<(String, Blamed)> {
        if !self.manifest(at)?.contains_key(path) {
            bail!("no such file {path} at {}", &at[..12.min(at.len())]);
        }
        let top = self.last_touch(path, at)?.with_context(|| format!("no changeset touched {path}"))?;
        // Walk back to a cached blame or the file's start; then compute forward.
        let mut steps: Vec<(String, String, bool)> = Vec::new(); // (changeset, path, has a source)
        let (mut cs, mut p) = (top.clone(), path.to_string());
        let mut base: Option<Blamed> = loop {
            if let Some(b) = self.blame_cached(&cs, &p)? {
                break Some(b);
            }
            let parents = self.changeset(&cs)?.parents;
            let Some(parent) = parents.first().cloned() else {
                let commit = self.commit_for(&cs)?;
                let gd = self.odb.dir().to_path_buf();
                let b = git_blame(&gd, &commit, &p, &cs)?;
                self.blame_store(&cs, &p, &b)?;
                break Some(b);
            };
            let src = if self.manifest(&parent)?.contains_key(&p) {
                Some(p.clone())
            } else {
                rename_source(self, &parent, &cs, &p)?
            };
            steps.push((cs.clone(), p.clone(), src.is_some()));
            let Some(src) = src else { break None };
            cs = self.last_touch(&src, &parent)?.with_context(|| format!("no changeset touched {src}"))?;
            p = src;
        };
        let mut prev = (cs, p);
        for (cs, p, has_src) in steps.into_iter().rev() {
            let cur = self.file_bytes(&cs, &p)?;
            let b = match (&base, has_src) {
                (Some(old), true) => {
                    let before = self.file_bytes(&prev.0, &prev.1)?;
                    carry(old, &before, &cur, &cs)
                }
                _ => {
                    let n = line_count(&cur) as u32;
                    Blamed {
                        origins: vec![cs.clone()],
                        runs: if n > 0 { vec![[0, 1, n]] } else { vec![] },
                        ..Default::default()
                    }
                }
            };
            self.blame_store(&cs, &p, &b)?;
            base = Some(b);
            prev = (cs, p);
        }
        Ok((top, base.unwrap_or_default()))
    }

    /// The `blame` operation: hunks of lines with the same origin, and each origin's metadata.
    pub fn blame(&mut self, path: &str, at: Option<&str>, range: Option<(u32, u32)>) -> Result<Value> {
        let at = self.resolve_at(at)?;
        let (top, b) = self.blame_of(path, &at)?;
        let mut hunks = Vec::new();
        let mut line = 1u32;
        let mut used = std::collections::BTreeSet::new();
        for r in &b.runs {
            let (start, end) = (line, line + r[2] - 1);
            line += r[2];
            if let Some((lo, hi)) = range
                && (end < lo || start > hi)
            {
                continue;
            }
            used.insert(r[0]);
            hunks.push(json!({"start": start, "count": r[2], "orig_start": r[1], "origin": b.origins[r[0] as usize]}));
        }
        let mut origins = Map::new();
        for i in used {
            let key = &b.origins[i as usize];
            let v = if let Some(c) = key.strip_prefix("git:") {
                let mut m = b.git.get(key).cloned().unwrap_or_else(|| json!({}));
                m["commit"] = json!(c);
                m["changeset"] = Value::Null;
                m
            } else {
                let m = self.changeset(key)?.meta;
                json!({
                    "changeset": key, "commit": self.store.commit_of(key)?, "change": m.change, "agent": m.agent,
                    "model": m.model, "session": m.session, "intent": m.intent, "time": m.time,
                })
            };
            origins.insert(key.clone(), v);
        }
        Ok(json!({"path": path, "at": at, "last_changed": top, "lines": line - 1, "hunks": hunks, "origins": origins}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carry_keeps_old_lines_and_claims_new_ones() {
        let a = b"one\ntwo\nthree\n";
        let b = b"one\nTWO\nthree\nfour\n";
        let prev = Blamed { origins: vec!["A".into()], runs: vec![[0, 1, 3]], ..Default::default() };
        let got = carry(&prev, a, b, "B").lines();
        assert_eq!(got, vec![(0, 1), (1, 2), (0, 3), (1, 4)]);
        let out = carry(&prev, a, b, "B");
        assert_eq!(out.origins, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn unused_origins_are_dropped() {
        let prev = Blamed { origins: vec!["A".into()], runs: vec![[0, 1, 2]], ..Default::default() };
        let out = carry(&prev, b"x\ny\n", b"z\n", "B");
        assert_eq!(out.origins, vec!["B".to_string()]);
        assert_eq!(out.runs, vec![[0, 1, 1]]);
    }
}
