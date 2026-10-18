//! Hunk ownership for agents that share one working directory (no workspaces). A share is a
//! base changeset, a list of open changes and the current text of the directory's changed files.
//! Each file's diff against the base splits into hunks with stable ids; each hunk gets one owner
//! (claim, declared intent, earlier owner), and `apply_assigned` writes only the owner's hunks
//! into its change, so two agents' edits to one file become two changes.

use anyhow::{Result, anyhow, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

use crate::config::globs;
use crate::coord::symbol_range;
use crate::git::object_id;
use crate::merge::{Merged, merge3};
use crate::model::{Change, ChangeState, Entry, MODE_FILE, MODE_GITLINK};
use crate::repo::{Edit, Inner, as_text};
use crate::xdiff;

pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS shares (name TEXT PRIMARY KEY, body TEXT NOT NULL);";

/// Base lines before and after a hunk that go into its id.
const CONTEXT: usize = 2;
/// Diff lines shown per hunk in a reply.
const SHOWN: usize = 40;

/// The server side of one shared directory.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Share {
    pub base: String,
    pub changes: Vec<String>,
    /// Hunk id -> change, from `claim`.
    #[serde(default)]
    pub claims: BTreeMap<String, String>,
    /// Text of each changed path in the directory (`None` = the file is gone). Other paths equal the base.
    #[serde(default)]
    pub dir: BTreeMap<String, Option<String>>,
    /// Hunks of the last plan: the memory behind stickiness and dependencies.
    #[serde(default)]
    pub last: Vec<Prior>,
    /// Per change, the base-relative text of each path `apply_assigned` wrote last.
    #[serde(default)]
    pub written: BTreeMap<String, BTreeMap<String, Option<String>>>,
}

/// A hunk as the last plan saw it, in base lines (0-based, half-open).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prior {
    pub path: String,
    pub id: String,
    pub owner: Option<String>,
    pub why: String,
    pub start: usize,
    pub end: usize,
    pub added: Vec<String>,
    /// An older version that its owner keeps, because the hunk that replaced it depends on it.
    #[serde(default)]
    pub kept: bool,
}

/// One hunk of the current directory against the base.
struct Planned {
    path: String,
    id: String,
    start: usize,
    end: usize,
    new_start: usize,
    removed: Vec<String>,
    added: Vec<String>,
    owner: Option<String>,
    why: &'static str,
    depends_on: Vec<String>,
    candidates: Vec<String>,
    /// The hunk belonged to this change, which has landed since.
    landed_in: Option<String>,
}

impl Planned {
    fn ready(&self) -> bool {
        self.owner.is_some() && self.depends_on.is_empty()
    }

    fn view(&self) -> Value {
        let mut diff: Vec<String> = self.removed.iter().map(|l| format!("-{}", l.trim_end_matches('\n'))).collect();
        diff.extend(self.added.iter().map(|l| format!("+{}", l.trim_end_matches('\n'))));
        let more = diff.len().saturating_sub(SHOWN);
        diff.truncate(SHOWN);
        if more > 0 {
            diff.push(format!("... {more} more lines"));
        }
        let mut v = json!({
            "id": self.id, "path": self.path, "owner": self.owner, "why": self.why,
            "old": [self.start + 1, self.end + 1], "new": [self.new_start + 1, self.new_start + self.added.len() + 1],
            "diff": diff,
        });
        if !self.depends_on.is_empty() {
            v["depends_on"] = json!(self.depends_on);
        }
        if !self.candidates.is_empty() {
            v["candidates"] = json!(self.candidates);
        }
        if let Some(l) = &self.landed_in {
            v["landed_in"] = json!(l);
        }
        v
    }
}

/// Two hunks (base line ranges) overlap or touch; a pure insertion is an empty range.
fn meet(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// A hunk inside a definition `[s, e)`: a pure insertion must sit strictly inside it.
fn inside(h: (usize, usize), r: (usize, usize)) -> bool {
    if h.0 == h.1 { r.0 < h.0 && h.0 < r.1 } else { h.0 < r.1 && r.0 < h.1 }
}

/// Lines with their newline, as owned strings.
fn split(text: &str) -> Vec<String> {
    xdiff::lines(text.as_bytes()).into_iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect()
}

/// The id of a hunk: path, removed and added lines and up to `CONTEXT` base lines on each side.
/// Line numbers are left out, so an edit elsewhere in the file keeps the id.
fn hunk_id(path: &str, base: &[String], h: &xdiff::Hunk, added: &[String]) -> String {
    let mut buf = format!("{path}\0");
    for l in &base[h.before.start.saturating_sub(CONTEXT)..h.before.start] {
        buf.push_str(l);
    }
    buf.push('\0');
    for l in &base[h.before.clone()] {
        buf.push_str(l);
    }
    buf.push('\0');
    for l in added {
        buf.push_str(l);
    }
    buf.push('\0');
    for l in &base[h.before.end..(h.before.end + CONTEXT).min(base.len())] {
        buf.push_str(l);
    }
    format!("h{}", &object_id("hunk", buf.as_bytes())[..12])
}

/// 1-based `[start, end)` of a definition to 0-based.
fn zero(r: Option<(usize, usize)>) -> Option<(usize, usize)> {
    r.map(|(s, e)| (s.saturating_sub(1), e.saturating_sub(1)))
}

/// The lines of `name`'s definition in `text`: tree-sitter, else the keyword heuristic.
fn definition(path: &str, text: &str, name: &str) -> Option<(usize, usize)> {
    zero(crate::symbols::definition_range(path, text, name).or_else(|| symbol_range(text, name)))
}

/// The only element of a set, if it has exactly one.
fn single(set: &BTreeSet<String>) -> Option<String> {
    if set.len() == 1 { set.iter().next().cloned() } else { None }
}

impl Inner {
    pub fn share_load(&self, name: &str) -> Result<Option<Share>> {
        let body: Option<String> = self
            .store
            .conn()
            .query_row("SELECT body FROM shares WHERE name = ?1", params![name], |r| r.get(0))
            .optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into)).transpose()
    }

    fn share_save(&self, name: &str, sh: &Share) -> Result<()> {
        self.store.conn().execute(
            "INSERT OR REPLACE INTO shares (name, body) VALUES (?1, ?2)",
            params![name, serde_json::to_string(sh)?],
        )?;
        Ok(())
    }

    /// The share named `name`, or the only share that lists `change`.
    pub fn share_of(&self, name: Option<&str>, change: &str) -> Result<(String, Share)> {
        if let Some(n) = name {
            let sh = self.share_load(n)?.ok_or_else(|| anyhow!("no share {n:?}; call assign first"))?;
            return Ok((n.to_string(), sh));
        }
        let mut stmt = self.store.conn().prepare("SELECT name, body FROM shares")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut hits = Vec::new();
        for row in rows {
            let (n, body) = row?;
            let sh: Share = serde_json::from_str(&body)?;
            if sh.changes.iter().any(|c| c == change) {
                hits.push((n, sh));
            }
        }
        match hits.len() {
            1 => Ok(hits.remove(0)),
            0 => bail!("change {change} is in no share; call assign with it first"),
            _ => bail!("change {change} is in several shares; pass share"),
        }
    }

    /// Text of `path` at the share base: `Ok(None)` when absent, `Err` when binary or a submodule.
    fn base_text(&mut self, base: &str, path: &str) -> Result<Option<String>> {
        let m = self.manifest(base)?;
        match m.get(path) {
            None => Ok(None),
            Some(e) if e.mode == MODE_GITLINK => bail!("{path} is a submodule"),
            Some(e) => {
                let data = self.blob(&e.blob)?;
                as_text(&data).map(Some).ok_or_else(|| anyhow!("{path} is not text"))
            }
        }
    }

    /// `assign`: records the directory's changed files (`files`: path -> text, `None` = gone;
    /// every other path equals the base), the share's changes and base, then plans ownership.
    pub fn assign(
        &mut self,
        name: &str,
        base: Option<&str>,
        changes: Option<Vec<String>>,
        files: BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        for p in files.keys() {
            crate::safety::check_path(p)?;
        }
        let mut sh = match self.share_load(name)? {
            Some(sh) => sh,
            None => Share { base: self.resolve_at(base)?, ..Share::default() },
        };
        if let Some(list) = changes {
            let mut seen = BTreeSet::new();
            sh.changes = list.into_iter().filter(|c| seen.insert(c.clone())).collect();
            for c in &sh.changes {
                self.change(c)?;
            }
        }
        if let Some(b) = base {
            let b = self.resolve_at(Some(b))?;
            if b != sh.base {
                self.share_rebase(&mut sh, b)?;
            }
        }
        sh.dir = files;
        let v = self.plan_view(&mut sh)?;
        self.share_save(name, &sh)?;
        Ok(
            json!({"share": name, "base": sh.base, "changes": v["changes"], "unassigned": v["unassigned"], "landed": v["landed"], "skipped": v["skipped"]}),
        )
    }

    /// Moves the share onto `base`. Every open change of the share must sit on it already (after
    /// `rebase`), so its head is the new reference for its text; earlier owners are forgotten.
    fn share_rebase(&mut self, sh: &mut Share, base: String) -> Result<()> {
        let mut written = BTreeMap::new();
        for id in &sh.changes {
            let ch = self.change(id)?;
            if ch.state != ChangeState::Open {
                continue;
            }
            if ch.base != base {
                bail!("change {id} is not on {}: rebase it first", &base[..12.min(base.len())]);
            }
            let mut texts = BTreeMap::new();
            for (path, e) in self.own_changes(&ch)? {
                let t = match e {
                    Some(e) => as_text(&self.blob(&e.blob)?),
                    None => None,
                };
                texts.insert(path, t);
            }
            written.insert(id.clone(), texts);
        }
        sh.base = base;
        sh.last.clear();
        sh.written = written;
        Ok(())
    }

    /// `claim`: the hunks go to `change`, before any intent or earlier owner.
    pub fn claim(&mut self, name: Option<&str>, change: &str, hunks: &[String]) -> Result<Value> {
        let (name, mut sh) = self.share_of(name, change)?;
        if !sh.changes.iter().any(|c| c == change) {
            bail!("change {change} is not in share {name:?}");
        }
        let known: BTreeSet<&str> = sh.last.iter().filter(|p| !p.kept).map(|p| p.id.as_str()).collect();
        if let Some(h) = hunks.iter().find(|h| !known.contains(h.as_str())) {
            bail!("no hunk {h} in share {name:?} (ids come from the last assign)");
        }
        for h in hunks {
            sh.claims.insert(h.clone(), change.to_string());
        }
        let v = self.plan_view(&mut sh)?;
        self.share_save(&name, &sh)?;
        Ok(
            json!({"share": name, "base": sh.base, "changes": v["changes"], "unassigned": v["unassigned"], "landed": v["landed"], "skipped": v["skipped"]}),
        )
    }

    /// `apply_assigned`: writes the change's ready hunks (owned, not depending on another
    /// change) and nothing else. Each path is base + own hunks; when the head moved since the
    /// last write (a rebase), that step is merged 3-way onto the head.
    pub fn apply_assigned(
        &mut self,
        name: Option<&str>,
        change: &str,
        allow: &dyn Fn(&str) -> Result<()>,
    ) -> Result<Value> {
        let (name, mut sh) = self.share_of(name, change)?;
        let ch = self.change(change)?;
        if ch.state != ChangeState::Open {
            bail!("change {change} is {}", ch.state.as_str());
        }
        let plan = self.plan(&mut sh)?;
        let mine: Vec<&Planned> = plan.hunks.iter().filter(|h| h.owner.as_deref() == Some(change)).collect();
        let mut segs: BTreeMap<String, Vec<(usize, usize, Vec<String>)>> = BTreeMap::new();
        for h in mine.iter().filter(|h| h.ready()) {
            segs.entry(h.path.clone()).or_default().push((h.start, h.end, h.added.clone()));
        }
        for p in sh.last.iter().filter(|p| p.kept && p.owner.as_deref() == Some(change)) {
            segs.entry(p.path.clone()).or_default().push((p.start, p.end, p.added.clone()));
        }
        let before = sh.written.get(change).cloned().unwrap_or_default();
        let paths: BTreeSet<String> = segs.keys().chain(before.keys()).cloned().collect();
        let head_m = self.manifest(&ch.head)?;
        let (mut edits, mut conflicts, mut now) = (Vec::new(), Vec::new(), BTreeMap::new());
        for path in paths {
            let base = self.base_text(&sh.base, &path)?;
            let new = match segs.get_mut(&path) {
                Some(s) => {
                    s.sort_by_key(|x| (x.0, x.1));
                    let lines = split(base.as_deref().unwrap_or(""));
                    let (mut out, mut pos) = (String::new(), 0);
                    for (start, end, added) in s.iter() {
                        if *start < pos {
                            continue;
                        }
                        out.extend(lines[pos..*start].iter().map(String::as_str));
                        out.extend(added.iter().map(String::as_str));
                        pos = *end;
                    }
                    out.extend(lines[pos..].iter().map(String::as_str));
                    let gone = matches!(sh.dir.get(&path), Some(None));
                    if out.is_empty() && gone { None } else { Some(out) }
                }
                None => base.clone(),
            };
            let old = before.get(&path).cloned().unwrap_or(base);
            let cur = head_m.get(&path).cloned();
            let mode = cur.as_ref().map_or(MODE_FILE, |e| e.mode);
            let mut entry = |t: &Option<String>| -> Result<Option<Entry>> {
                t.as_ref().map(|t| Ok(Entry { blob: self.put_blob(t.as_bytes())?, mode })).transpose()
            };
            let (old_e, new_e) = (entry(&old)?, entry(&new)?);
            let result = if old_e == cur {
                new_e
            } else {
                match merge3(self, old_e.as_ref(), new_e.as_ref(), cur.as_ref())? {
                    Merged::Clean(e) => e,
                    Merged::Conflict { .. } => {
                        conflicts.push(path);
                        continue;
                    }
                }
            };
            now.insert(path.clone(), new);
            if result == cur {
                continue;
            }
            allow(&path)?;
            let mut e = Edit {
                path,
                content: None,
                content_b64: None,
                delete: None,
                replace: None,
                executable: None,
                symlink: None,
            };
            match result {
                Some(r) => e.content = Some(String::from_utf8(self.blob(&r.blob)?)?),
                None => e.delete = Some(true),
            }
            edits.push(e);
        }
        let ch = if edits.is_empty() { ch } else { self.apply_seen(change, &edits, None, Some(&ch.head))? };
        let entry = sh.written.entry(change.to_string()).or_default();
        for (p, t) in now {
            entry.insert(p, t);
        }
        self.share_save(&name, &sh)?;
        let held: Vec<Value> = mine.iter().filter(|h| !h.ready()).map(|h| h.view()).collect();
        Ok(json!({
            "share": name, "change": ch.id, "head": ch.head,
            "applied": mine.iter().filter(|h| h.ready()).map(|h| h.id.clone()).collect::<Vec<_>>(),
            "held": held, "paths": edits.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), "conflicts": conflicts,
        }))
    }

    fn plan_view(&mut self, sh: &mut Share) -> Result<Value> {
        let plan = self.plan(sh)?;
        let mut per = Vec::new();
        for id in &sh.changes {
            let ch = self.change(id)?;
            let hunks: Vec<Value> =
                plan.hunks.iter().filter(|h| h.owner.as_deref() == Some(id.as_str())).map(Planned::view).collect();
            per.push(json!({"change": ch.id, "agent": ch.agent, "state": ch.state.as_str(), "hunks": hunks}));
        }
        let free = |h: &&Planned| h.owner.is_none() && h.landed_in.is_none();
        let unassigned: Vec<Value> = plan.hunks.iter().filter(free).map(Planned::view).collect();
        let landed: Vec<Value> = plan.hunks.iter().filter(|h| h.landed_in.is_some()).map(Planned::view).collect();
        Ok(json!({"changes": per, "unassigned": unassigned, "landed": landed, "skipped": plan.skipped}))
    }

    /// Splits every changed file of the share into hunks and gives each one its owner, in this
    /// order: a claim, a declared symbol, a declared path, the earlier owner, nobody. A hunk that
    /// meets an earlier hunk of another change (it builds on that change's lines) depends on it;
    /// the older version then stays with its owner (`kept`). Updates `last` and `claims`.
    fn plan(&mut self, sh: &mut Share) -> Result<Plan> {
        let mut open: BTreeMap<String, Change> = BTreeMap::new();
        for id in &sh.changes {
            let ch = self.change(id)?;
            if ch.state == ChangeState::Open {
                open.insert(id.clone(), ch);
            }
        }
        let mut path_globs = BTreeMap::new();
        for ch in open.values() {
            path_globs.insert(ch.id.clone(), globs(&ch.declared.paths)?);
        }
        let mut out = Plan::default();
        let dir = sh.dir.clone();
        for (path, now) in &dir {
            let base = match self.base_text(&sh.base, path) {
                Ok(b) => b,
                Err(_) => {
                    out.skipped.push(path.clone());
                    continue;
                }
            };
            let (bt, nt) = (base.unwrap_or_default(), now.clone().unwrap_or_default());
            let (bl, nl) = (split(&bt), split(&nt));
            let priors: Vec<&Prior> = sh.last.iter().filter(|p| &p.path == path).collect();
            let mut ids: BTreeMap<String, usize> = BTreeMap::new();
            for h in xdiff::diff(bt.as_bytes(), nt.as_bytes()) {
                let added: Vec<String> = nl[h.after.clone()].to_vec();
                let mut id = hunk_id(path, &bl, &h, &added);
                let n = ids.entry(id.clone()).or_insert(0);
                *n += 1;
                if *n > 1 {
                    id = format!("{id}-{n}");
                }
                let range = (h.before.start, h.before.end);
                let after = (h.after.start, h.after.end);
                let same = priors.iter().find(|p| p.id == id && !p.kept);
                let over: Vec<&&Prior> =
                    priors.iter().filter(|p| p.id != id && meet((p.start, p.end), range)).collect();
                let over_owners: BTreeSet<String> = over.iter().filter_map(|p| p.owner.clone()).collect();
                let is_open = |c: &String| open.contains_key(c);
                // 1. A claim on this id, or a claim on the hunk it grew from.
                let mut owner = sh.claims.get(&id).filter(|c| is_open(c)).cloned().map(|c| (c, "claim"));
                if owner.is_none()
                    && let Some(c) = single(&over_owners).filter(is_open)
                    && over.iter().all(|p| p.owner.as_ref() == Some(&c))
                    && over.iter().any(|p| p.why == "claim")
                {
                    owner = Some((c, "claim"));
                }
                // 2. Intents: a declared symbol whose definition holds the hunk, then a declared path.
                let sticky: BTreeSet<String> =
                    same.and_then(|p| p.owner.clone()).into_iter().chain(over_owners.iter().cloned()).collect();
                let mut candidates = Vec::new();
                if owner.is_none() {
                    let mut by_symbol = BTreeSet::new();
                    for ch in open.values() {
                        for s in &ch.declared.symbols {
                            let (p, name) = match s.rsplit_once('#') {
                                Some((p, n)) => (Some(p), n),
                                None => (None, s.as_str()),
                            };
                            if p.is_some_and(|p| p.trim_matches('/') != path) {
                                continue;
                            }
                            let hit = definition(path, &bt, name).is_some_and(|r| inside(range, r))
                                || (!added.is_empty() && definition(path, &nt, name).is_some_and(|r| inside(after, r)));
                            if hit {
                                by_symbol.insert(ch.id.clone());
                            }
                        }
                    }
                    let by_path: BTreeSet<String> = open
                        .values()
                        .filter(|ch| {
                            path_globs[&ch.id].is_match(path.as_str())
                                || ch.declared.paths.iter().any(|p| p.trim_matches('/') == path)
                        })
                        .map(|ch| ch.id.clone())
                        .collect();
                    for set in [by_symbol, by_path] {
                        if set.is_empty() {
                            continue;
                        }
                        let tie: BTreeSet<String> = set.intersection(&sticky).cloned().collect();
                        match single(&set).or_else(|| single(&tie)) {
                            Some(c) => owner = Some((c, "intent")),
                            None => candidates = set.into_iter().collect(),
                        }
                        break;
                    }
                }
                // 3. Stickiness: the owner of this id before, or of the one hunk it grew from.
                if owner.is_none() && candidates.is_empty() {
                    let before = same
                        .and_then(|p| p.owner.clone())
                        .filter(is_open)
                        .or_else(|| single(&over_owners).filter(is_open));
                    owner = before.map(|c| (c, "sticky"));
                }
                let depends_on: Vec<String> =
                    over_owners.iter().filter(|c| owner.as_ref().is_none_or(|(o, _)| o != *c)).cloned().collect();
                let landed_in = same
                    .and_then(|p| p.owner.clone())
                    .filter(|c| !is_open(c) && self.change(c).is_ok_and(|ch| ch.state == ChangeState::Landed));
                let (owner, why) = match owner {
                    Some((c, w)) => (Some(c), w),
                    None if !candidates.is_empty() => (None, "ambiguous"),
                    None if landed_in.is_some() => (None, "landed"),
                    None => (None, "none"),
                };
                out.hunks.push(Planned {
                    path: path.clone(),
                    id,
                    start: range.0,
                    end: range.1,
                    new_start: after.0,
                    removed: bl[h.before.clone()].to_vec(),
                    added,
                    owner,
                    why,
                    depends_on,
                    candidates,
                    landed_in,
                });
            }
        }
        // An earlier hunk that a new one depends on stays with its owner until the new one is
        // settled (claimed by that owner, or gone).
        let mut kept: Vec<Prior> = Vec::new();
        for p in &sh.last {
            let Some(o) = &p.owner else { continue };
            let held = out.hunks.iter().any(|h| {
                h.path == p.path && h.id != p.id && meet((h.start, h.end), (p.start, p.end)) && h.depends_on.contains(o)
            });
            if held && !kept.iter().any(|k| k.id == p.id && k.owner == p.owner) {
                kept.push(Prior { kept: true, ..p.clone() });
            }
        }
        let current: BTreeSet<&str> = out.hunks.iter().map(|h| h.id.as_str()).collect();
        sh.claims.retain(|id, _| current.contains(id.as_str()));
        for h in out.hunks.iter().filter(|h| h.why == "claim") {
            sh.claims.insert(h.id.clone(), h.owner.clone().unwrap_or_default());
        }
        sh.last = out
            .hunks
            .iter()
            .map(|h| Prior {
                path: h.path.clone(),
                id: h.id.clone(),
                owner: h.owner.clone().or_else(|| h.landed_in.clone()),
                why: h.why.to_string(),
                start: h.start,
                end: h.end,
                added: h.added.clone(),
                kept: false,
            })
            .chain(kept)
            .collect();
        Ok(out)
    }
}

#[derive(Default)]
struct Plan {
    hunks: Vec<Planned>,
    skipped: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_ignore_line_numbers_and_ranges_meet() {
        let a = split("a\nb\nc\nd\ne\nf\n");
        let h = xdiff::Hunk { before: 3..4, after: 3..4 };
        let id = hunk_id("x", &a, &h, &["D\n".into()]);
        // The same edit two lines lower in a file with two more lines on top.
        let b = split("0\n1\na\nb\nc\nd\ne\nf\n");
        let h2 = xdiff::Hunk { before: 5..6, after: 5..6 };
        assert_eq!(id, hunk_id("x", &b, &h2, &["D\n".into()]));
        assert_ne!(id, hunk_id("y", &b, &h2, &["D\n".into()]));
        assert!(meet((3, 3), (3, 5)) && meet((2, 3), (3, 3)) && !meet((1, 2), (3, 4)));
        assert!(inside((4, 4), (2, 6)) && !inside((2, 2), (2, 6)) && !inside((6, 7), (2, 6)) && inside((5, 7), (2, 6)));
    }
}
