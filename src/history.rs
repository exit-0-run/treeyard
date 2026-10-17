//! History queries on derived data: the mainline (main's first-parent chain with heights) and
//! a changed-path index (path -> heights of the mainline changesets that touched it).
//!
//! The index holds each changed file and each of its parent directories (`dir/`), so the
//! history of a directory is one ordered index range, like the history of a file.
//!
//! Both tables are computed from the changesets and grow incrementally: `sync` walks from
//! main back to the last indexed changeset and appends only what is new. Main only moves to a
//! descendant, so the mainline is append-only; a rewind (never expected) truncates and rebuilds.

use anyhow::{Context, Result};
use globset::GlobSet;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::BTreeSet;

use crate::config::globs;
use crate::git;
use crate::model::{Changeset, Meta};
use crate::repo::Inner;

/// Bump when the rows of `touch` change meaning; `sync_history` then rebuilds both tables.
const INDEX_VERSION: &str = "2";

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS mainline (height INTEGER PRIMARY KEY, changeset TEXT UNIQUE NOT NULL);
CREATE TABLE IF NOT EXISTS touch (path TEXT NOT NULL, height INTEGER NOT NULL, PRIMARY KEY (path, height)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS touch_height ON touch (height);
";

/// Filters of `log`: a path or directory, path globs, agent, time window.
#[derive(Default)]
pub struct Query {
    pub path: Option<String>,
    pub globs: Vec<String>,
    pub agent: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub limit: usize,
    /// Walk the changesets instead of the index (the baseline for benchmarks and tests).
    pub scan: bool,
}

fn under(p: &str, dir: &str) -> bool {
    p == dir || (p.len() > dir.len() && p.starts_with(dir) && p.as_bytes()[dir.len()] == b'/')
}

/// The fixed prefix of a glob, before its first wildcard (for an index range scan).
fn literal_prefix(g: &str) -> String {
    let g = g.trim().trim_start_matches("./").trim_start_matches('/');
    if !g.contains('/') && g.contains(['*', '?', '[']) {
        return String::new();
    }
    if !g.contains('/') {
        return String::new(); // a bare name matches at any depth
    }
    g.chars().take_while(|c| !matches!(c, '*' | '?' | '[' | '{')).collect()
}

impl Inner {
    /// Brings the mainline and the changed-path index up to main. Returns the main height.
    pub fn sync_history(&mut self) -> Result<i64> {
        let main = self.main()?;
        if self.store.get_ref("history_index")?.as_deref() != Some(INDEX_VERSION) {
            self.store.tx(|s| {
                s.conn().execute_batch("DELETE FROM mainline; DELETE FROM touch;")?;
                s.set_ref("history_index", INDEX_VERSION)
            })?;
        }
        let conn = self.store.conn();
        let tip: Option<(i64, String)> = conn
            .query_row("SELECT height, changeset FROM mainline ORDER BY height DESC LIMIT 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        if let Some((h, cs)) = &tip
            && *cs == main
        {
            return Ok(*h);
        }
        let mut new: Vec<(String, Changeset)> = Vec::new();
        let mut cur = Some(main);
        let mut at = -1i64;
        while let Some(id) = cur {
            let known: Option<i64> = self
                .store
                .conn()
                .query_row("SELECT height FROM mainline WHERE changeset = ?1", [&id], |r| r.get(0))
                .optional()?;
            if let Some(h) = known {
                at = h;
                break;
            }
            let cs = self.changeset(&id)?;
            cur = cs.parents.first().cloned();
            new.push((id, cs));
        }
        self.store.tx(|s| {
            let c = s.conn();
            if tip.as_ref().is_some_and(|(h, _)| *h != at) {
                c.execute("DELETE FROM mainline WHERE height > ?1", [at])?;
                c.execute("DELETE FROM touch WHERE height > ?1", [at])?;
            }
            let mut ins = c.prepare_cached("INSERT INTO mainline (height, changeset) VALUES (?1, ?2)")?;
            let mut t = c.prepare_cached("INSERT OR IGNORE INTO touch (path, height) VALUES (?1, ?2)")?;
            for (i, (id, cs)) in new.iter().rev().enumerate() {
                let h = at + 1 + i as i64;
                ins.execute(params![h, id])?;
                let mut keys: BTreeSet<&str> = BTreeSet::new();
                for path in cs.changes.keys() {
                    keys.insert(path);
                    keys.extend(path.match_indices('/').map(|(j, _)| &path[..=j]));
                }
                for k in keys {
                    t.execute(params![k, h])?;
                }
            }
            Ok(())
        })?;
        Ok(at + new.len() as i64)
    }

    /// The mainline height of `cs`, if it is on main's first-parent chain.
    pub fn mainline_height(&mut self, cs: &str) -> Result<Option<i64>> {
        self.sync_history()?;
        Ok(self
            .store
            .conn()
            .query_row("SELECT height FROM mainline WHERE changeset = ?1", [cs], |r| r.get(0))
            .optional()?)
    }

    pub fn mainline_at(&self, height: i64) -> Result<String> {
        self.store
            .conn()
            .query_row("SELECT changeset FROM mainline WHERE height = ?1", [height], |r| r.get(0))
            .with_context(|| format!("no mainline changeset at height {height}"))
    }

    /// Changesets from `at` back to its first mainline ancestor (excluded), newest first, and
    /// the height of that ancestor (-1 if none).
    fn off_main(&mut self, at: &str) -> Result<(Vec<String>, i64)> {
        let mut out = Vec::new();
        let mut cur = Some(at.to_string());
        while let Some(id) = cur {
            if let Some(h) = self.mainline_height(&id)? {
                return Ok((out, h));
            }
            cur = self.changeset(&id)?.parents.first().cloned();
            out.push(id);
        }
        Ok((out, -1))
    }

    /// Mainline heights at or below `top` that touched `dir` (a file or a directory), newest
    /// first, up to `limit`, from the index (two ordered range reads, merged).
    pub fn touched_heights(&mut self, dir: &str, top: i64, limit: usize) -> Result<Vec<i64>> {
        let mut stmt = self.store.conn().prepare_cached(
            "SELECT height FROM touch WHERE path = ?1 AND height <= ?2 ORDER BY height DESC LIMIT ?3",
        )?;
        let mut hs = BTreeSet::new();
        for key in [dir.to_string(), format!("{dir}/")] {
            let rows = stmt.query_map(params![key, top, limit as i64], |r| r.get::<_, i64>(0))?;
            for h in rows {
                hs.insert(h?);
            }
        }
        Ok(hs.into_iter().rev().take(limit).collect())
    }

    /// The newest changeset at or before `at` (first-parent ancestry) that touched `path`.
    pub fn last_touch(&mut self, path: &str, at: &str) -> Result<Option<String>> {
        let (off, top) = self.off_main(at)?;
        for id in off {
            if self.changeset(&id)?.changes.contains_key(path) {
                return Ok(Some(id));
            }
        }
        if top < 0 {
            return Ok(None);
        }
        let h: Option<i64> = self
            .store
            .conn()
            .query_row(
                "SELECT height FROM touch WHERE path = ?1 AND height <= ?2 ORDER BY height DESC LIMIT 1",
                params![path, top],
                |r| r.get(0),
            )
            .optional()?;
        h.map(|h| self.mainline_at(h)).transpose()
    }

    /// History of main (or of `at`), newest first, with changed-path and other filters.
    pub fn history(&mut self, at: Option<&str>, q: &Query) -> Result<Value> {
        let start = self.resolve_at(at)?;
        let dir = q.path.as_deref().map(|p| p.trim_matches('/')).filter(|p| !p.is_empty()).map(str::to_string);
        let set: Option<GlobSet> = if q.globs.is_empty() { None } else { Some(globs(&q.globs)?) };
        let matches =
            |p: &str| dir.as_deref().is_none_or(|d| under(p, d)) && set.as_ref().is_none_or(|g| g.is_match(p));
        let filtered = dir.is_some() || set.is_some();
        let mut out = Vec::new();
        let mut scanned = 0usize;
        let push = |r: &mut Inner, id: &str, cs: &Changeset, out: &mut Vec<Value>| -> Result<bool> {
            let m = &cs.meta;
            if q.agent.as_ref().is_some_and(|a| *a != m.agent)
                || q.since.is_some_and(|s| m.time < s)
                || q.until.is_some_and(|u| m.time > u)
            {
                return Ok(false);
            }
            let touched: Vec<&String> = cs.changes.keys().filter(|k| matches(k)).collect();
            if touched.is_empty() && (filtered || !cs.parents.is_empty()) {
                return Ok(false);
            }
            out.push(entry(r, id, m, &touched)?);
            Ok(true)
        };
        if q.scan {
            let mut cur = Some(start.clone());
            while let Some(id) = cur {
                if out.len() >= q.limit {
                    break;
                }
                let cs = self.changeset(&id)?;
                scanned += 1;
                push(self, &id, &cs, &mut out)?;
                cur = cs.parents.first().cloned();
            }
            return Ok(json!({"at": start, "entries": out, "scanned": scanned, "index": false}));
        }
        let (off, top) = self.off_main(&start)?;
        for id in off {
            if out.len() >= q.limit {
                break;
            }
            let cs = self.changeset(&id)?;
            scanned += 1;
            push(self, &id, &cs, &mut out)?;
        }
        // Candidate heights from the index: the directory, the fixed prefixes of the globs, or all.
        let mut prefixes: Vec<String> = Vec::new();
        if let Some(d) = &dir {
            prefixes.push(d.clone());
        } else if !q.globs.is_empty() {
            let ps: Vec<String> = q.globs.iter().map(|g| literal_prefix(g)).collect();
            if ps.iter().all(|p| !p.is_empty()) {
                prefixes = ps.into_iter().map(|p| p.trim_end_matches('/').to_string()).collect();
            }
        }
        let mut top = top;
        while out.len() < q.limit && top >= 0 {
            let batch = (q.limit - out.len()).max(64) * 2;
            let heights: Vec<i64> = if prefixes.is_empty() {
                (0..=top).rev().take(batch).collect()
            } else {
                let mut hs = BTreeSet::new();
                for p in &prefixes {
                    hs.extend(self.touched_heights(p, top, batch)?);
                }
                hs.into_iter().rev().take(batch).collect()
            };
            if heights.is_empty() {
                break;
            }
            for &h in &heights {
                if out.len() >= q.limit {
                    break;
                }
                let id = self.mainline_at(h)?;
                let cs = self.changeset(&id)?;
                scanned += 1;
                push(self, &id, &cs, &mut out)?;
            }
            top = heights.last().copied().unwrap_or(0) - 1;
        }
        Ok(json!({"at": start, "entries": out, "scanned": scanned, "index": true}))
    }
}

fn entry(r: &mut Inner, id: &str, m: &Meta, touched: &[&String]) -> Result<Value> {
    Ok(json!({
        "changeset": id, "commit": r.store.commit_of(id)?, "change": m.change,
        "agent": m.agent, "model": m.model, "session": m.session, "intent": m.intent, "message": m.message,
        "time": m.time, "paths": touched.iter().take(50).collect::<Vec<_>>(), "path_count": touched.len(),
    }))
}

/// First-parent commits after `root` up to `tip` (oldest first) as changesets, each mapped to
/// its own git commit; used by `init --history`. Merge commits diff against their first parent.
pub fn import_log(git_dir: &std::path::Path, root: &str, tip: &str) -> Result<Vec<(String, Changeset)>> {
    let range = format!("{root}..{tip}");
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args([
            "log",
            "--first-parent",
            "--diff-merges=first-parent",
            "--no-renames",
            "--raw",
            "-z",
            "--no-abbrev",
            "--reverse",
            "--format=%x01%H%x02%ct%x02%an%x02%B%x02",
            &range,
        ])
        .output()
        .context("running git log")?;
    if !out.status.success() {
        anyhow::bail!("git log {range} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut result = Vec::new();
    for chunk in out.stdout.split(|b| *b == 1).filter(|c| !c.is_empty()) {
        let mut f = chunk.splitn(5, |b| *b == 2);
        let (Some(h), Some(ct), Some(an), Some(body), raw) = (f.next(), f.next(), f.next(), f.next(), f.next()) else {
            anyhow::bail!("bad git log record");
        };
        let commit = String::from_utf8(h.to_vec())?;
        let time: i64 = std::str::from_utf8(ct)?.trim().parse()?;
        let author = String::from_utf8_lossy(an).split_whitespace().collect::<Vec<_>>().join(" ");
        let message = String::from_utf8_lossy(body).trim_end().to_string();
        let raw = raw.unwrap_or_default();
        let raw: &[u8] = &raw[raw.iter().position(|b| *b == b':').unwrap_or(raw.len())..];
        let changes = git::parse_raw(raw)?.into_iter().collect();
        let subject = message.lines().next().unwrap_or("").trim().to_string();
        let meta = Meta {
            agent: format!("git:{author}"),
            intent: if subject.is_empty() { format!("commit {}", &commit[..12]) } else { subject },
            message: Some(message),
            session: Some("import".into()),
            time,
            ..Meta::default()
        };
        result.push((commit, Changeset { parents: vec![], changes, meta }));
    }
    Ok(result)
}
