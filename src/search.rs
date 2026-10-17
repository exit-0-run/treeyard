//! Trigram index for regex grep (the Google Code Search / zoekt idea, kept small).
//!
//! Derived data per blob: the sorted set of byte trigrams of a text file, persisted in SQLite
//! (content-addressed, so a new revision only adds its new blobs). In memory the server keeps
//! one posting index for main, updated incrementally when main moves (changed files are
//! marked dead and appended; a rebuild happens when half the files are dead). A query turns
//! the regex into an AND/OR of trigrams, takes the candidate files from the postings, and runs
//! the regex only on them. Revisions other than main use main's index plus the changed files.

use anyhow::{Result, anyhow};
use regex::Regex;
use regex_syntax::hir::{Class, Hir, HirKind};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

use crate::model::{MODE_GITLINK, Manifest};
use crate::repo::{Inner, Repo, as_text};

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS trigrams (blob TEXT PRIMARY KEY, data BLOB NOT NULL) WITHOUT ROWID;
";

/// Marker stored for a blob that is not text (never searched).
const BINARY: &[u8] = b"\xff";
/// Largest cross product of alternatives the planner tracks before it splits a literal.
const MAX_SET: usize = 64;
/// More changed files than this between main and the asked revision: scan instead.
const MAX_OVERLAY: usize = 4096;

/// Sorted, unique byte trigrams of `data`.
pub fn trigrams(data: &[u8]) -> Vec<u32> {
    let mut v: Vec<u32> =
        data.windows(3).map(|w| (u32::from(w[0]) << 16) | (u32::from(w[1]) << 8) | u32::from(w[2])).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn encode(v: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    let mut prev = 0u32;
    for &x in v {
        let mut d = x - prev;
        prev = x;
        loop {
            let b = (d & 0x7f) as u8;
            d >>= 7;
            if d == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }
    out
}

fn decode(data: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let (mut prev, mut cur, mut shift) = (0u32, 0u32, 0);
    for &b in data {
        cur |= u32::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            prev += cur;
            out.push(prev);
            cur = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    out
}

// ---- query planning ---------------------------------------------------------------------

/// What a file must contain for the regex to match a line in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    All,
    Tri(u32),
    And(Vec<Plan>),
    Or(Vec<Plan>),
}

impl Plan {
    fn and(parts: Vec<Plan>) -> Plan {
        let mut v: Vec<Plan> = Vec::new();
        for p in parts {
            match p {
                Plan::All => {}
                Plan::And(xs) => v.extend(xs),
                x => v.push(x),
            }
        }
        v.dedup();
        match v.len() {
            0 => Plan::All,
            1 => v.pop().unwrap_or(Plan::All),
            _ => Plan::And(v),
        }
    }

    fn or(parts: Vec<Plan>) -> Plan {
        if parts.is_empty() || parts.contains(&Plan::All) {
            return Plan::All;
        }
        let mut v: Vec<Plan> = Vec::new();
        for p in parts {
            match p {
                Plan::Or(xs) => v.extend(xs),
                x => v.push(x),
            }
        }
        if v.len() == 1 { v.pop().unwrap_or(Plan::All) } else { Plan::Or(v) }
    }

    /// Number of trigram lookups (0 = the plan cannot narrow the search).
    pub fn size(&self) -> usize {
        match self {
            Plan::All => 0,
            Plan::Tri(_) => 1,
            Plan::And(v) | Plan::Or(v) => v.iter().map(Plan::size).sum(),
        }
    }
}

fn exact_plan(set: &[Vec<u8>]) -> Plan {
    Plan::or(set.iter().map(|s| Plan::and(trigrams(s).into_iter().map(Plan::Tri).collect())).collect())
}

struct Info {
    /// Every string the node can match, when the set is small.
    exact: Option<Vec<Vec<u8>>>,
    /// Otherwise: what a match must contain.
    plan: Plan,
}

impl Info {
    fn exact(set: Vec<Vec<u8>>) -> Self {
        Info { exact: Some(set), plan: Plan::All }
    }
    fn any() -> Self {
        Info { exact: None, plan: Plan::All }
    }
    fn into_plan(self) -> Plan {
        match self.exact {
            Some(s) => exact_plan(&s),
            None => self.plan,
        }
    }
}

fn class_strings(c: &Class) -> Option<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    match c {
        Class::Unicode(u) => {
            for r in u.ranges() {
                for ch in r.start()..=r.end() {
                    out.push(ch.to_string().into_bytes());
                    if out.len() > 16 {
                        return None;
                    }
                }
            }
        }
        Class::Bytes(b) => {
            for r in b.ranges() {
                for x in r.start()..=r.end() {
                    out.push(vec![x]);
                    if out.len() > 16 {
                        return None;
                    }
                }
            }
        }
    }
    Some(out)
}

fn cross(a: &[Vec<u8>], b: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(a.len() * b.len());
    for x in a {
        for y in b {
            let mut s = x.clone();
            s.extend_from_slice(y);
            out.push(s);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn analyze(h: &Hir) -> Info {
    match h.kind() {
        HirKind::Empty | HirKind::Look(_) => Info::exact(vec![vec![]]),
        HirKind::Literal(l) => Info::exact(vec![l.0.to_vec()]),
        HirKind::Class(c) => class_strings(c).map_or_else(Info::any, Info::exact),
        HirKind::Capture(c) => analyze(&c.sub),
        HirKind::Repetition(r) => {
            if r.min == 0 {
                return Info::any();
            }
            let inner = analyze(&r.sub);
            if r.min == 1 && r.max == Some(1) {
                return inner;
            }
            Info { exact: None, plan: inner.into_plan() }
        }
        HirKind::Concat(hs) => {
            let mut parts = Vec::new();
            let mut cur: Vec<Vec<u8>> = vec![vec![]];
            let mut all_exact = true;
            for child in hs {
                let ci = analyze(child);
                match ci.exact {
                    Some(e) if cur.len() * e.len() <= MAX_SET => cur = cross(&cur, &e),
                    Some(e) => {
                        all_exact = false;
                        parts.push(exact_plan(&cur));
                        cur = e;
                    }
                    None => {
                        all_exact = false;
                        parts.push(exact_plan(&cur));
                        parts.push(ci.plan);
                        cur = vec![vec![]];
                    }
                }
            }
            if all_exact {
                return Info::exact(cur);
            }
            parts.push(exact_plan(&cur));
            Info { exact: None, plan: Plan::and(parts) }
        }
        HirKind::Alternation(hs) => {
            let infos: Vec<Info> = hs.iter().map(analyze).collect();
            if infos.iter().all(|i| i.exact.is_some()) {
                let set: Vec<Vec<u8>> = infos.iter().flat_map(|i| i.exact.clone().unwrap_or_default()).collect();
                if set.len() <= MAX_SET {
                    return Info::exact(set);
                }
            }
            Info { exact: None, plan: Plan::or(infos.into_iter().map(Info::into_plan).collect()) }
        }
    }
}

/// The trigram plan of a regex (the same syntax as the `regex` crate).
pub fn plan(pattern: &str) -> Result<Plan> {
    let hir = regex_syntax::Parser::new().parse(pattern).map_err(|e| anyhow!("bad pattern: {e}"))?;
    Ok(analyze(&hir).into_plan())
}

fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

fn union(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = a.iter().chain(b).copied().collect();
    out.sort_unstable();
    out.dedup();
    out
}

// ---- the in-memory index ----------------------------------------------------------------

/// Posting lists for the text files of one revision (normally main).
#[derive(Default)]
pub struct Index {
    pub at: String,
    files: Vec<(String, String)>,
    live: Vec<bool>,
    by_path: HashMap<String, u32>,
    postings: HashMap<u32, Vec<u32>>,
    dead: usize,
    /// Blobs with a row in `trigrams` (loaded once).
    known: HashSet<String>,
    loaded: bool,
}

impl Index {
    fn add(&mut self, path: &str, blob: &str, tris: &[u32]) {
        let id = self.files.len() as u32;
        self.files.push((path.to_string(), blob.to_string()));
        self.live.push(true);
        self.by_path.insert(path.to_string(), id);
        for t in tris {
            self.postings.entry(*t).or_default().push(id);
        }
    }

    fn kill(&mut self, path: &str) {
        if let Some(id) = self.by_path.remove(path) {
            self.live[id as usize] = false;
            self.dead += 1;
        }
    }

    fn eval(&self, p: &Plan) -> Option<Vec<u32>> {
        match p {
            Plan::All => None,
            Plan::Tri(t) => Some(self.postings.get(t).cloned().unwrap_or_default()),
            Plan::And(v) => {
                let mut sets: Vec<Vec<u32>> = v.iter().filter_map(|x| self.eval(x)).collect();
                sets.sort_by_key(Vec::len);
                let mut it = sets.into_iter();
                let first = it.next()?;
                Some(it.fold(first, |acc, s| intersect(&acc, &s)))
            }
            Plan::Or(v) => {
                let mut acc = Vec::new();
                for x in v {
                    acc = union(&acc, &self.eval(x)?);
                }
                Some(acc)
            }
        }
    }

    /// Memory held by the posting lists, in bytes (rough).
    pub fn bytes(&self) -> usize {
        self.postings.values().map(|v| v.capacity() * 4 + 32).sum::<usize>() + self.files.len() * 96
    }
}

fn searchable(e: &crate::model::Entry) -> bool {
    e.mode != MODE_GITLINK
}

impl Inner {
    fn load_known(&mut self) -> Result<()> {
        if self.search.loaded {
            return Ok(());
        }
        let mut stmt = self.store.conn().prepare("SELECT blob FROM trigrams")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        self.search.known.extend(ids);
        self.search.loaded = true;
        Ok(())
    }

    /// Up to `max_bytes` of blobs of `at` that have no trigram row yet, as (blob, data).
    pub fn trigram_todo(&mut self, at: &str, max_bytes: usize) -> Result<Vec<(String, Vec<u8>)>> {
        self.load_known()?;
        let m = self.manifest(at)?;
        let mut out = Vec::new();
        let mut size = 0;
        let mut seen = HashSet::new();
        for (_, e) in m.iter() {
            if !searchable(e) || self.search.known.contains(&e.blob) || !seen.insert(e.blob.clone()) {
                continue;
            }
            let data = self.blob(&e.blob)?;
            size += data.len();
            out.push((e.blob.clone(), data));
            if size >= max_bytes {
                break;
            }
        }
        Ok(out)
    }

    pub fn trigram_store(&mut self, rows: &[(String, Vec<u8>)]) -> Result<()> {
        self.store.tx(|s| {
            let mut st = s.conn().prepare_cached("INSERT OR REPLACE INTO trigrams (blob, data) VALUES (?1, ?2)")?;
            for (b, d) in rows {
                st.execute(params![b, d])?;
            }
            Ok(())
        })?;
        self.search.known.extend(rows.iter().map(|(b, _)| b.clone()));
        Ok(())
    }

    fn trigrams_of(&mut self, blob: &str) -> Result<Option<Vec<u32>>> {
        let row: Option<Vec<u8>> = rusqlite::OptionalExtension::optional(self.store.conn().query_row(
            "SELECT data FROM trigrams WHERE blob = ?1",
            [blob],
            |r| r.get(0),
        ))?;
        let data = match row {
            Some(d) => d,
            None => {
                let d = trigram_row(&self.blob(blob)?);
                self.trigram_store(&[(blob.to_string(), d.clone())])?;
                d
            }
        };
        Ok(if data == BINARY { None } else { Some(decode(&data)) })
    }

    /// Moves the in-memory index to `at` (computing missing trigram rows inline).
    pub fn search_index(&mut self, at: &str) -> Result<()> {
        if self.search.at == at && !self.search.files.is_empty() {
            return Ok(());
        }
        let new = self.manifest(at)?;
        let rebuild = self.search.at.is_empty() || self.search.dead * 2 > self.search.files.len().max(64);
        let changed: Vec<(String, Option<crate::model::Entry>)> = if rebuild {
            let known = std::mem::take(&mut self.search.known);
            self.search = Index { known, loaded: self.search.loaded, ..Index::default() };
            new.iter().map(|(p, e)| (p.clone(), Some(e.clone()))).collect()
        } else {
            let old = self.manifest(&self.search.at.clone())?;
            diff_manifests(&old, &new)
        };
        for (path, e) in changed {
            self.search.kill(&path);
            if let Some(e) = e.filter(searchable)
                && let Some(t) = self.trigrams_of(&e.blob)?
            {
                self.search.add(&path, &e.blob, &t);
            }
        }
        self.search.at = at.to_string();
        Ok(())
    }

    /// Regex grep through the trigram index; same matches, order and limit as the scan.
    pub fn grep_indexed(
        &mut self,
        pattern: &str,
        at: Option<&str>,
        dir: Option<&str>,
        max: usize,
        allow: &dyn Fn(&str) -> bool,
    ) -> Result<Value> {
        let re = Regex::new(pattern).map_err(|e| anyhow!("bad pattern: {e}"))?;
        let plan = plan(pattern)?;
        let cs = self.resolve_at(at)?;
        let main = self.main()?;
        self.search_index(&main)?;
        let mut cands: Vec<(String, String)> = match self.search.eval(&plan) {
            Some(ids) => ids
                .into_iter()
                .filter(|i| self.search.live[*i as usize])
                .map(|i| self.search.files[i as usize].clone())
                .collect(),
            None => self.search.by_path.values().map(|i| self.search.files[*i as usize].clone()).collect(),
        };
        let mut overlay = 0;
        if cs != main {
            let (a, b) = (self.manifest(&main)?, self.manifest(&cs)?);
            let changed = diff_manifests(&a, &b);
            if changed.len() > MAX_OVERLAY {
                return self.grep_scan(pattern, Some(&cs), dir, max, allow);
            }
            overlay = changed.len();
            let gone: HashSet<&String> = changed.iter().map(|(p, _)| p).collect();
            cands.retain(|(p, _)| !gone.contains(p));
            cands.extend(
                changed
                    .iter()
                    .filter_map(|(p, e)| e.as_ref().filter(|e| searchable(e)).map(|e| (p.clone(), e.blob.clone()))),
            );
        }
        let prefix = dir.map(|d| d.trim_matches('/')).filter(|d| !d.is_empty()).map(|d| format!("{d}/"));
        cands.retain(|(p, _)| prefix.as_ref().is_none_or(|x| p.starts_with(x)) && allow(p));
        cands.sort();
        let considered = cands.len();
        let (matches, truncated) = self.verify(&re, &cands, max)?;
        Ok(json!({
            "at": cs, "matches": matches, "truncated": truncated,
            "index": {"used": true, "trigrams": plan.size(), "candidates": considered, "files": self.search.by_path.len(), "overlay": overlay, "postings_mb": self.search.bytes() / (1 << 20)},
        }))
    }

    fn verify(&mut self, re: &Regex, files: &[(String, String)], max: usize) -> Result<(Vec<Value>, bool)> {
        let mut matches = Vec::new();
        for (path, blob) in files {
            let data = self.blob(blob)?;
            let Some(text) = as_text(&data) else { continue };
            if !re.is_match(&text) {
                continue;
            }
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    if matches.len() == max {
                        return Ok((matches, true));
                    }
                    matches.push(json!({"path": path, "line": i + 1, "text": line}));
                }
            }
        }
        Ok((matches, false))
    }

    /// Regex grep by reading every text file of the revision (no index).
    pub fn grep_scan(
        &mut self,
        pattern: &str,
        at: Option<&str>,
        dir: Option<&str>,
        max: usize,
        allow: &dyn Fn(&str) -> bool,
    ) -> Result<Value> {
        let re = Regex::new(pattern).map_err(|e| anyhow!("bad pattern: {e}"))?;
        let cs = self.resolve_at(at)?;
        let m = self.manifest(&cs)?;
        let prefix = dir.map(|d| d.trim_matches('/')).filter(|d| !d.is_empty()).map(|d| format!("{d}/"));
        let files: Vec<(String, String)> = m
            .iter()
            .filter(|(p, e)| searchable(e) && prefix.as_ref().is_none_or(|x| p.starts_with(x)) && allow(p))
            .map(|(p, e)| (p.clone(), e.blob.clone()))
            .collect();
        let (matches, truncated) = self.verify(&re, &files, max)?;
        Ok(
            json!({"at": cs, "matches": matches, "truncated": truncated, "index": {"used": false, "files": files.len()}}),
        )
    }
}

/// The stored row for a blob: encoded trigrams, or the binary marker.
pub fn trigram_row(data: &[u8]) -> Vec<u8> {
    match as_text(data) {
        Some(_) => encode(&trigrams(data)),
        None => BINARY.to_vec(),
    }
}

/// Paths whose entry differs between two manifests: (path, new entry or None).
pub fn diff_manifests(a: &Manifest, b: &Manifest) -> Vec<(String, Option<crate::model::Entry>)> {
    let mut out = Vec::new();
    for item in a.diff(b) {
        match item {
            im::ordmap::DiffItem::Add(p, e) => out.push((p.clone(), Some(e.clone()))),
            im::ordmap::DiffItem::Update { new: (p, e), .. } => out.push((p.clone(), Some(e.clone()))),
            im::ordmap::DiffItem::Remove(p, _) => out.push((p.clone(), None)),
        }
    }
    out
}

/// Computes missing trigram rows for `at` in chunks: blobs are read under the repo lock, the
/// trigrams are computed outside it (on all cores), so agents keep landing meanwhile.
pub fn prepare(repo: &Repo, at: &str) -> Result<usize> {
    let mut done = 0;
    loop {
        let todo = repo.with(|r| r.trigram_todo(at, 16 << 20))?;
        if todo.is_empty() {
            return Ok(done);
        }
        done += todo.len();
        let rows = par_map(&todo, |(b, d)| (b.clone(), trigram_row(d)));
        repo.with(|r| r.trigram_store(&rows))?;
    }
}

/// Maps `f` over `items` on all cores, keeping the order.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let chunk = items.len().div_ceil(n).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = items.chunks(chunk).map(|c| s.spawn(|| c.iter().map(&f).collect::<Vec<R>>())).collect();
        handles.into_iter().flat_map(|h| h.join().expect("worker thread")).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Plan {
        Plan::and(trigrams(s.as_bytes()).into_iter().map(Plan::Tri).collect())
    }

    #[test]
    fn varint_round_trip() {
        let v = trigrams(b"hello world, hello treeyard");
        assert_eq!(decode(&encode(&v)), v);
    }

    #[test]
    fn literals_need_their_trigrams() {
        assert_eq!(plan("hello").unwrap(), t("hello"));
        assert_eq!(plan("ab").unwrap(), Plan::All);
        assert_eq!(plan(".*").unwrap(), Plan::All);
        assert_eq!(plan(r"\w+").unwrap(), Plan::All);
    }

    #[test]
    fn concat_and_alternation() {
        assert_eq!(plan("foo|bar").unwrap(), Plan::or(vec![t("foo"), t("bar")]));
        let p = plan(r"fn\s+parse_args").unwrap();
        assert_eq!(p, Plan::and(vec![t("fn"), t("parse_args")]));
        assert!(plan("(?i)hello").unwrap().size() > 0);
        assert_eq!(plan("x(abc)?y").unwrap(), Plan::All);
        assert_eq!(plan("(abc)+def").unwrap(), Plan::and(vec![t("abc"), t("def")]));
    }
}
