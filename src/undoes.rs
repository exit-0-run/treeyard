//! `undoes`: lines a change removes that another agent wrote for its own task.
//!
//! A line counts when all of this holds:
//! - the change's base-to-head diff removes it (deletes or rewrites it);
//! - blame at the change's base names a changeset of another agent's change that has landed, or
//!   that is still open (the parent a change is stacked on); the import, the caller's own work
//!   (same agent name, or the same non-empty `session`) and abandoned changes do not count;
//! - its text is gone from the change's new tree: whitespace-normalised (all whitespace removed),
//!   the line occurs fewer times in the head versions of the changed paths than in their base
//!   versions. So a block that moved (to a helper, to another file, a rename), was re-indented, or
//!   changed only its line ends, is quiet;
//! - what the other change contributed to the line is gone. The contribution comes from that
//!   change's own diff of the path (`OriginDiff`): the words (identifiers, numbers, words of
//!   strings) of the line that are not among the words of the lines its hunk replaced, or every
//!   word of the line when the hunk replaced nothing (or the change added no word of its own: an
//!   operator, a reordering). The line is quiet when those words, in order, all appear on one added
//!   line of the current diff hunk, or on two adjacent added lines. So `x.casefold()` that another
//!   agent made `x.strip().casefold()` (contribution `strip`) may become `x.strip().lower()`, but
//!   not `x.casefold()`; a line another agent added whole keeps the rule of every word
//!   (`value.lower()` -> `value.lower().strip()` is quiet, `value.lower()` -> `value` warns).
//!   Operators are not words, so `x > 0` -> `x >= 0` is quiet;
//! - blame names only the last change of a line. Earlier contributions to it are found by
//!   following the lines that change's hunk replaced back (blame at its parent, up to `CHAIN`
//!   steps): an earlier change of another agent whose words the later line still carries
//!   contributed them too, and dropping them warns with that change as the owner;
//! - it has a letter or digit (a lone `}` never warns, but joins a run of lines around it).
//!
//! Per change, the scan of each path is kept while the change's base and the path's head blob
//! stay the same, so an `apply` to another file costs no diff or blame. The origin diffs are kept
//! per (changeset, path) for the process (`Inner::origin_diffs`). Entries already sent in an
//! earlier `apply` or `resolve` reply are counted (`undoes_earlier`), not repeated.

use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::model::{Change, ChangeState, Entry, MODE_GITLINK};
use crate::repo::Inner;

/// At most this many entries per reply; the rest are counted in `undoes_more`.
pub const MAX: usize = 20;
pub const ADVICE: &str = "These lines hold work that another agent wrote for its own task (landed, or the open change \
you are stacked on). Keep what it contributed (the words in `contributed`, else the whole line) and still make your own edit \
of the same line: restoring means keeping its part, not giving up yours. If both tasks cannot hold, say so in your \
message instead of removing its work.";
/// The advice of a land held by `undoes` (status `undoes`).
pub const HOLD_ADVICE: &str = "Not landed: your change removes what another agent landed in these lines for its own \
task. Put its part back (the words in `contributed`, else the whole line) while keeping your own edit of the same \
lines, apply, and land again. If your task and these lines truly cannot both hold, keep the other agent's lines, \
land the rest, and say so in your final message so a person decides. Only if your task names these lines for removal, \
land again with undo = true and reason = why your task removes them.";

/// What `land` does when the change removes lines other agents landed (`undoes` entries without
/// `landed: false`, not of a change it was stacked on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undo<'a> {
    /// Do not check (revert, a human push, replay).
    Off,
    /// Hold the land: status `undoes` (unless `[coordination] hold_undoes = false`).
    Hold,
    /// The owner's explicit yes with its reason: land and record it (`undid`).
    Yes(&'a str),
}

/// Removed text per entry: at most this many lines and bytes (`text_cut` past that).
const TEXT_LINES: usize = 8;
const TEXT_BYTES: usize = 800;
/// How far back an earlier contribution to a line is followed (changes before blame's last one).
const CHAIN: usize = 3;
/// Replaced lines of one hunk followed back per step.
const CHAIN_LINES: usize = 8;
/// A contribution's `before` (the lines its hunk replaced): at most this many bytes.
const BEFORE_BYTES: usize = 300;
/// Origin diffs kept per process; the map is emptied when it is full.
const ORIGIN_DIFFS: usize = 512;

/// Which entries a reply carries.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Entries not sent in an earlier reply of this change (`apply`, `resolve`); remembers them.
    New,
    /// Every entry that stands now (`change`, `land`); remembers nothing.
    All,
}

/// Per open change: the scan of each changed path and the entries already sent.
#[derive(Default)]
pub struct State {
    paths: BTreeMap<String, Scan>,
    reported: HashSet<String>,
}

struct Removed {
    /// 1-based line in the base file.
    line: usize,
    text: String,
    norm: String,
    /// Blame's origin of the line (the last change that wrote it).
    origin: String,
    /// What other agents' changes contributed to the line, blame's origin first.
    contribs: Vec<Contrib>,
}

/// One change's part of a removed line.
#[derive(Clone, Debug)]
struct Contrib {
    origin: String,
    /// The words it added; `None`: every word of the line (it added the line with no counterpart,
    /// or added no word of its own).
    words: Option<Vec<String>>,
    /// That change's own before (the lines its hunk replaced, cut at `BEFORE_BYTES`) and after
    /// (its version of the line).
    before: String,
    after: String,
    /// The words are still on an added line of the current hunk (or on two adjacent ones).
    kept: bool,
}

/// A changeset's own diff of one path: its parent's version (through a rename the way blame
/// follows it) to its version.
pub struct OriginDiff {
    /// (parent changeset, the path there); `None`: the changeset added the file.
    parent: Option<(String, String)>,
    old: Vec<String>,
    new: Vec<String>,
    hunks: Vec<crate::xdiff::Hunk>,
}

struct Scan {
    key: (String, Option<String>),
    before: Option<String>,
    after: Option<String>,
    removed: Vec<Removed>,
    /// Per normalised line: how many more times the base version has it than the head version
    /// (lines outside the diff hunks are equal on both sides, so the hunks are enough). Built in
    /// the scan when it diffed anyway, else on the first need.
    delta: Option<HashMap<String, i64>>,
}

/// The result of one `undoes` computation.
#[derive(Default)]
pub struct Report {
    /// Every entry (a reply carries at most `MAX`, see `put`).
    pub entries: Vec<Value>,
    pub earlier: usize,
}

impl Report {
    /// Writes `undoes` (always), and `undoes_advice`, `undoes_more`, `undoes_earlier` when they
    /// apply, into a reply, keeping only entries whose path `can_read` allows, at most `MAX`.
    pub fn put(self, v: &mut Value, can_read: &dyn Fn(&str) -> bool) {
        let mut entries: Vec<Value> =
            self.entries.into_iter().filter(|e| e["path"].as_str().is_some_and(can_read)).collect();
        if !entries.is_empty() {
            v["undoes_advice"] = json!(ADVICE);
        }
        let more = entries.len().saturating_sub(MAX);
        entries.truncate(MAX);
        if more > 0 {
            v["undoes_more"] = json!(more);
        }
        if self.earlier > 0 {
            v["undoes_earlier"] = json!(self.earlier);
        }
        v["undoes"] = Value::Array(entries);
    }
}

/// Keeps only `undoes` entries whose path `can_read` allows in a reply that `Report::put` filled
/// for every path (`land`); drops the advice when none is left and counts the rest in
/// `undoes_hidden`.
pub fn filter(v: &mut Value, can_read: &dyn Fn(&str) -> bool) {
    let Some(list) = v.get_mut("undoes").and_then(Value::as_array_mut) else { return };
    let before = list.len();
    list.retain(|e| e["path"].as_str().is_some_and(can_read));
    let (left, hidden) = (list.len(), before - list.len());
    if hidden > 0 {
        v["undoes_hidden"] = json!(hidden);
    }
    if left == 0
        && let Some(m) = v.as_object_mut()
    {
        m.remove("undoes_advice");
    }
}

/// Removes the words of `replaced` from `line`, one occurrence each, earliest first: what a hunk
/// that turned `replaced` into lines including `line` added to it.
fn contributed<'a>(line: &[&'a str], replaced: &[&str]) -> Vec<&'a str> {
    let mut left: HashMap<&str, usize> = HashMap::new();
    for w in replaced {
        *left.entry(w).or_default() += 1;
    }
    let mut out = Vec::new();
    for w in line {
        match left.get_mut(w) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push(*w),
        }
    }
    out
}

/// `w`, in order, on one of `added` or on two adjacent ones.
fn kept_on(w: &[&str], added: &[Vec<&str>]) -> bool {
    added.iter().enumerate().any(|(k, a)| {
        kept_in(w, a) || added.get(k + 1).is_some_and(|b| kept_in(w, &[a.as_slice(), b.as_slice()].concat()))
    })
}

fn cut(lines: &[String], max: usize) -> String {
    let mut s = String::new();
    for l in lines {
        if s.len() + l.len() > max {
            break;
        }
        s.push_str(l);
    }
    s
}

fn owned_words(s: &str) -> Vec<String> {
    words(s).into_iter().map(str::to_string).collect()
}

/// An entry without its text, for events and lists.
pub fn brief(e: &Value) -> Value {
    json!({"path": e["path"], "lines": e["lines"], "agent": e["agent"], "change": e["change"]})
}

/// `id` and the changes whose lines `entries` name, once each (an event's `changes`).
fn with_owners(id: &str, entries: &[Value]) -> Vec<String> {
    let mut out = vec![id.to_string()];
    for c in entries.iter().filter_map(|e| e["change"].as_str()) {
        if !out.iter().any(|x| x == c) {
            out.push(c.to_string());
        }
    }
    out
}

/// The line without whitespace (ASCII whitespace, so line ends and indentation of any kind).
pub fn norm(line: &str) -> String {
    let b: Vec<u8> = line.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    String::from_utf8_lossy(&b).into_owned()
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|t| !t.is_empty()).collect()
}

/// Every word of `old`, in order, appears in `new`.
fn kept_in(old: &[&str], new: &[&str]) -> bool {
    let mut it = new.iter();
    !old.is_empty() && old.iter().all(|w| it.any(|n| n == w))
}

fn trivial(norm: &str) -> bool {
    !norm.chars().any(char::is_alphanumeric)
}

/// Removed minus added occurrences of each non-trivial normalised line in `hunks`.
fn delta(old_lines: &[&str], new_lines: &[&str], hunks: &[crate::xdiff::Hunk]) -> HashMap<String, i64> {
    let mut m = HashMap::new();
    for h in hunks {
        for (lines, d) in [(&old_lines[h.before.clone()], 1), (&new_lines[h.after.clone()], -1)] {
            for l in lines {
                let n = norm(l);
                if !trivial(&n) {
                    *m.entry(n).or_insert(0) += d;
                }
            }
        }
    }
    m.retain(|_, v| *v != 0);
    m
}

/// Who wrote an origin changeset, when it is another agent's landed or open change.
#[derive(Clone)]
struct Owner {
    agent: String,
    intent: String,
    change: String,
    landed: bool,
}

impl Inner {
    fn text_blob(&mut self, e: Option<&Entry>) -> Result<Option<String>> {
        let Some(e) = e.filter(|e| e.mode != MODE_GITLINK) else { return Ok(None) };
        let data = self.blob(&e.blob)?;
        if data.iter().take(8000).any(|b| *b == 0) {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&data).into_owned()))
    }

    fn scan(&mut self, ch: &Change, path: &str, before: Option<&Entry>, after: Option<&Entry>) -> Result<Scan> {
        let base = ch.base.as_str();
        let key = (base.to_string(), after.map(|e| e.blob.clone()));
        let mut s = Scan {
            key,
            before: before.map(|e| e.blob.clone()),
            after: after.map(|e| e.blob.clone()),
            removed: vec![],
            delta: None,
        };
        let Some(old) = self.text_blob(before)? else { return Ok(s) };
        // A file last touched by the import holds only imported lines: no diff or blame needed.
        match self.last_touch(path, base)? {
            Some(t) if !self.changeset(&t)?.parents.is_empty() => {}
            _ => return Ok(s),
        }
        let new = self.text_blob(after)?.unwrap_or_default();
        let hunks = crate::xdiff::diff(old.as_bytes(), new.as_bytes());
        let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
        let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
        s.delta = Some(delta(&old_lines, &new_lines, &hunks));
        if hunks.iter().all(|h| h.before.is_empty()) {
            return Ok(s);
        }
        let (_, blamed) = self.blame_of(path, base)?;
        let origins = blamed.lines();
        // Lines of the import, of the caller's own work or of abandoned changes never count.
        let mut foreign: HashMap<String, bool> = HashMap::new();
        for h in hunks.into_iter().filter(|h| !h.before.is_empty()) {
            let added: Vec<Vec<&str>> = new_lines[h.after.clone()].iter().map(|l| words(l)).collect();
            for i in h.before {
                let Some(&(o, at)) = origins.get(i) else { continue };
                let origin = &blamed.origins[o as usize];
                if origin.starts_with("git:") {
                    continue;
                }
                if !foreign.contains_key(origin) {
                    let f = self.owner(origin, ch)?.is_some();
                    foreign.insert(origin.clone(), f);
                }
                if !foreign[origin] {
                    continue;
                }
                let text = old_lines.get(i).copied().unwrap_or_default();
                let n = norm(text);
                let mut contribs =
                    if trivial(&n) { vec![] } else { self.contributions(ch, origin, path, at as usize, text, CHAIN)? };
                let all = words(text);
                for c in &mut contribs {
                    c.kept = match &c.words {
                        Some(w) => kept_on(&w.iter().map(String::as_str).collect::<Vec<_>>(), &added),
                        None => kept_on(&all, &added),
                    };
                }
                s.removed.push(Removed {
                    line: i + 1,
                    text: text.to_string(),
                    norm: n,
                    origin: origin.clone(),
                    contribs,
                });
            }
        }
        Ok(s)
    }

    /// The own diff of `path` in changeset `cs` (cached; `None` when `cs` did not write a text
    /// file there or has no parent).
    fn origin_diff(&mut self, cs: &str, path: &str) -> Result<Option<Arc<OriginDiff>>> {
        let key = (cs.to_string(), path.to_string());
        if let Some(d) = self.origin_diffs.get(&key) {
            return Ok(d.clone());
        }
        let d = self.origin_diff_uncached(cs, path)?.map(Arc::new);
        if self.origin_diffs.len() >= ORIGIN_DIFFS {
            self.origin_diffs.clear();
        }
        self.origin_diffs.insert(key, d.clone());
        Ok(d)
    }

    fn origin_diff_uncached(&mut self, cs: &str, path: &str) -> Result<Option<OriginDiff>> {
        let c = self.changeset(cs)?;
        let Some(Some(after)) = c.changes.get(path).cloned() else { return Ok(None) };
        let Some(parent) = c.parents.first().cloned() else { return Ok(None) };
        let Some(new) = self.text_blob(Some(&after))? else { return Ok(None) };
        let pm = self.manifest(&parent)?;
        let src = match pm.contains_key(path) {
            true => Some(path.to_string()),
            false => crate::blame::rename_source(self, &parent, cs, path)?,
        };
        let old = match src.as_ref().and_then(|p| pm.get(p)).cloned() {
            Some(e) => match self.text_blob(Some(&e))? {
                Some(t) => t,
                None => return Ok(None),
            },
            None => String::new(),
        };
        let hunks = crate::xdiff::diff(old.as_bytes(), new.as_bytes());
        let lines = |t: &str| t.split_inclusive('\n').map(str::to_string).collect::<Vec<_>>();
        Ok(Some(OriginDiff { parent: src.map(|p| (parent, p)), old: lines(&old), new: lines(&new), hunks }))
    }

    /// The path under which changeset `cs` wrote `text` as line `n` (1-based): `path`, or
    /// another path it wrote (blame followed a rename since), with its own diff.
    fn origin_path(&mut self, cs: &str, path: &str, n: usize, text: &str) -> Result<Option<Arc<OriginDiff>>> {
        let has = |d: &OriginDiff| n >= 1 && d.new.get(n - 1).is_some_and(|l| l == text);
        if let Some(d) = self.origin_diff(cs, path)?
            && has(&d)
        {
            return Ok(Some(d));
        }
        let written: Vec<String> =
            self.changeset(cs)?.changes.into_iter().filter(|(p, e)| e.is_some() && p != path).map(|(p, _)| p).collect();
        for p in written.iter().take(16) {
            if let Some(d) = self.origin_diff(cs, p)?
                && has(&d)
            {
                return Ok(Some(d));
            }
        }
        Ok(None)
    }

    /// What changeset `origin` (and, `depth` steps back, earlier changes of other agents whose
    /// words it kept) contributed to `text`, its line `n` (1-based) of `path`. Blame's origin
    /// comes first; it is the whole line when its diff cannot be read.
    fn contributions(
        &mut self,
        ch: &Change,
        origin: &str,
        path: &str,
        n: usize,
        text: &str,
        depth: usize,
    ) -> Result<Vec<Contrib>> {
        let whole = || Contrib {
            origin: origin.to_string(),
            words: None,
            before: String::new(),
            after: text.into(),
            kept: false,
        };
        let Some(d) = self.origin_path(origin, path, n, text)? else { return Ok(vec![whole()]) };
        let Some(h) = d.hunks.iter().find(|h| h.after.contains(&(n - 1))).cloned() else { return Ok(vec![whole()]) };
        if h.before.is_empty() {
            return Ok(vec![whole()]);
        }
        let line = words(text);
        let replaced: Vec<&str> = d.old[h.before.clone()].iter().flat_map(|l| words(l)).collect();
        let own = contributed(&line, &replaced);
        let mut out = vec![if own.is_empty() {
            whole()
        } else {
            Contrib {
                origin: origin.to_string(),
                words: Some(own.iter().map(|w| w.to_string()).collect()),
                before: cut(&d.old[h.before.clone()], BEFORE_BYTES),
                after: text.to_string(),
                kept: false,
            }
        }];
        let Some((parent, ppath)) = d.parent.clone().filter(|_| depth > 0) else { return Ok(out) };
        let (_, blamed) = self.blame_of(&ppath, &parent)?;
        let lines = blamed.lines();
        let mut seen: HashSet<(String, Vec<String>)> = HashSet::new();
        for j in h.before.clone().take(CHAIN_LINES) {
            let Some(&(o, at)) = lines.get(j) else { continue };
            let earlier = blamed.origins[o as usize].clone();
            if earlier == origin || earlier.starts_with("git:") || self.owner(&earlier, ch)?.is_none() {
                continue;
            }
            let Some(prev) = d.old.get(j).cloned() else { continue };
            for mut c in self.contributions(ch, &earlier, &ppath, at as usize, &prev, depth - 1)? {
                let w = c.words.clone().unwrap_or_else(|| owned_words(&c.after));
                let ws: Vec<&str> = w.iter().map(String::as_str).collect();
                // Only what flowed into this line: the later change kept those words.
                if c.origin != origin && kept_in(&ws, &line) && seen.insert((c.origin.clone(), w.clone())) {
                    c.words = Some(w);
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    /// `Scan::delta` of a path the scan did not diff (a new file, or one only the import touched).
    fn lazy_delta(&mut self, s: &Scan) -> Result<HashMap<String, i64>> {
        let text = |r: &mut Self, id: &Option<String>| -> Result<String> {
            let e = id.as_ref().map(|b| Entry { blob: b.clone(), mode: crate::model::MODE_FILE });
            Ok(r.text_blob(e.as_ref())?.unwrap_or_default())
        };
        let (old, new) = (text(self, &s.before)?, text(self, &s.after)?);
        let hunks = crate::xdiff::diff(old.as_bytes(), new.as_bytes());
        let (o, n): (Vec<&str>, Vec<&str>) = (old.split_inclusive('\n').collect(), new.split_inclusive('\n').collect());
        Ok(delta(&o, &n, &hunks))
    }

    fn owner(&mut self, origin: &str, ch: &Change) -> Result<Option<Owner>> {
        let cs = self.changeset(origin)?;
        if cs.parents.is_empty() || cs.meta.agent == ch.agent {
            return Ok(None);
        }
        if cs.meta.session.is_some() && cs.meta.session == ch.session {
            return Ok(None);
        }
        let Some(id) = cs.meta.change.clone().filter(|id| *id != ch.id) else { return Ok(None) };
        let landed = match self.store.change(&id)?.map(|c| c.state) {
            Some(ChangeState::Landed) => true,
            Some(ChangeState::Open) => false,
            _ => return Ok(None),
        };
        Ok(Some(Owner { agent: cs.meta.agent, intent: cs.meta.intent, change: id, landed }))
    }

    /// The entries that hold a land: lines another agent landed (not `landed: false`), not of a
    /// change `ch` was stacked on.
    /// Lines a revert put back count as the work they restore, not the revert's, so they never hold.
    pub fn held(&mut self, ch: &Change, entries: &[Value]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut reverts: HashMap<String, bool> = HashMap::new();
        for e in entries {
            let Some(owner) = e["change"].as_str() else { continue };
            if e.get("landed") == Some(&json!(false)) || ch.stacked_on.iter().any(|s| s == owner) {
                continue;
            }
            if !reverts.contains_key(owner) {
                let r = self.store.change(owner)?.is_some_and(|c| c.reverts.is_some());
                reverts.insert(owner.to_string(), r);
            }
            if !reverts[owner] {
                out.push(e.clone());
            }
        }
        Ok(out)
    }

    /// The reply (and event) of a land held by `held` entries; `cause` = `land` or `queue`.
    pub fn hold(&mut self, ch: &Change, held: Vec<Value>, cause: &str, merged_from: Value) -> Result<Value> {
        let mut agents: Vec<&str> = vec![&ch.agent];
        for a in held.iter().filter_map(|e| e["agent"].as_str()) {
            if !agents.contains(&a) {
                agents.push(a);
            }
        }
        let changes = with_owners(&ch.id, &held);
        let changes: Vec<&str> = changes.iter().map(String::as_str).collect();
        let brief: Vec<Value> = held.iter().map(brief).collect();
        self.emit("undoes", &changes, &agents, json!({"change": ch.id, "cause": cause, "undoes": brief}));
        let more = held.len().saturating_sub(MAX);
        let mut v = json!({"status": "undoes", "change": ch.id, "advice": HOLD_ADVICE});
        let mut held = held;
        held.truncate(MAX);
        v["undoes"] = Value::Array(held);
        if more > 0 {
            v["undoes_more"] = json!(more);
        }
        if !merged_from.is_null() {
            v["merged_from"] = merged_from;
        }
        Ok(v)
    }

    /// The `undo_override` event of a change that landed with `undid`.
    pub fn emit_undid(&mut self, ch: &Change) {
        let Some(u) = &ch.undid else { return };
        let owners = u.agents();
        let mut agents: Vec<&str> = vec![&ch.agent];
        agents.extend(owners.iter().map(String::as_str).filter(|a| *a != ch.agent));
        let changes = with_owners(&ch.id, &u.undoes);
        let changes: Vec<&str> = changes.iter().map(String::as_str).collect();
        let brief: Vec<Value> = u.undoes.iter().map(brief).collect();
        let body = json!({"change": ch.id, "by": u.by, "reason": u.reason, "agents": owners, "undoes": brief});
        self.emit("undo_override", &changes, &agents, body);
    }

    /// The `undoes` entries of `ch` (see the module docs). `Scope::New` sends only entries that no
    /// earlier `apply` or `resolve` reply of this change sent.
    pub fn undoes(&mut self, ch: &Change, scope: Scope) -> Result<Report> {
        let mut state = self.undo_state.remove(&ch.id).unwrap_or_default();
        let out = self.undoes_in(ch, scope, &mut state);
        if ch.state == ChangeState::Open {
            self.undo_state.insert(ch.id.clone(), state);
        }
        out
    }

    fn undoes_in(&mut self, ch: &Change, scope: Scope, state: &mut State) -> Result<Report> {
        if ch.head == ch.base {
            state.paths.clear();
            state.reported.clear();
            return Ok(Report::default());
        }
        let changed = self.changeset(&ch.head)?.changes;
        let base_m = self.manifest(&ch.base)?;
        state.paths.retain(|p, _| changed.contains_key(p));
        for (path, after) in &changed {
            let key = (ch.base.clone(), after.as_ref().map(|e| e.blob.clone()));
            if state.paths.get(path).is_some_and(|s| s.key == key) {
                continue;
            }
            let s = self.scan(ch, path, base_m.get(path), after.as_ref())?;
            state.paths.insert(path.clone(), s);
        }
        // Candidates: removed lines whose contribution of another agent's change is gone; the
        // first such contribution (blame's origin before earlier ones) names the owner.
        let mut owners: HashMap<String, Option<Owner>> = HashMap::new();
        let mut cands: Vec<(String, usize, usize)> = Vec::new();
        for (path, s) in &state.paths {
            for (k, r) in s.removed.iter().enumerate() {
                if trivial(&r.norm) {
                    continue;
                }
                for (ci, c) in r.contribs.iter().enumerate() {
                    if c.kept {
                        continue;
                    }
                    if !owners.contains_key(&c.origin) {
                        let o = self.owner(&c.origin, ch)?;
                        owners.insert(c.origin.clone(), o);
                    }
                    if owners[&c.origin].is_some() {
                        cands.push((path.clone(), k, ci));
                        break;
                    }
                }
            }
        }
        // (path, removed index) -> the contribution that is lost.
        let mut lost: HashMap<(String, usize), usize> = HashMap::new();
        if !cands.is_empty() {
            // Text still in the new tree (moved, re-indented, other line ends) is not lost.
            let paths: Vec<String> = state.paths.keys().cloned().collect();
            for p in &paths {
                if state.paths[p].delta.is_none() {
                    let d = self.lazy_delta(&state.paths[p])?;
                    state.paths.get_mut(p).expect("scanned").delta = Some(d);
                }
            }
            let mut deficit: HashMap<&str, i64> = HashMap::new();
            for (path, k, ci) in &cands {
                let n = state.paths[path].removed[*k].norm.as_str();
                if !deficit.contains_key(n) {
                    let d: i64 = state.paths.values().filter_map(|s| s.delta.as_ref()?.get(n)).sum();
                    deficit.insert(n, d);
                }
                let d = deficit.get_mut(n).expect("set above");
                if *d > 0 {
                    *d -= 1;
                    lost.insert((path.clone(), *k), *ci);
                }
            }
        }
        // Runs of lost lines with one owner; trivial removed lines of that origin join a run.
        let mut entries = Vec::new();
        let mut keys = Vec::new();
        for (path, s) in &state.paths {
            // The changeset a removed line counts for: its lost contribution's, else blame's.
            let whose = |k: usize| -> &str {
                match lost.get(&(path.clone(), k)) {
                    Some(ci) => &s.removed[k].contribs[*ci].origin,
                    None => &s.removed[k].origin,
                }
            };
            let mut runs: Vec<(usize, usize, Vec<usize>)> = Vec::new(); // (first k, last k, lost ks)
            for (k, r) in s.removed.iter().enumerate() {
                let is_lost = lost.contains_key(&(path.clone(), k));
                let joins = |run: &(usize, usize, Vec<usize>)| {
                    let last = &s.removed[run.1];
                    whose(run.1) == whose(k) && last.line + 1 == r.line
                };
                match runs.last_mut() {
                    Some(run) if joins(run) && (is_lost || trivial(&r.norm)) => {
                        run.1 = k;
                        if is_lost {
                            run.2.push(k);
                        }
                    }
                    _ if is_lost => runs.push((k, k, vec![k])),
                    _ => {}
                }
            }
            for (first, _, ks) in runs {
                // A run ends at its last lost line (trailing trivial lines do not count).
                let last = *ks.last().expect("a run has a lost line");
                let r0 = &s.removed[first];
                let origin = whose(first).to_string();
                let owner = owners[&origin].clone().expect("lost lines have an owner");
                let mut text = String::new();
                let mut cut = false;
                for (n, r) in s.removed[first..=last].iter().enumerate() {
                    if n >= TEXT_LINES || text.len() + r.text.len() > TEXT_BYTES {
                        cut = true;
                        break;
                    }
                    text.push_str(&r.text);
                }
                // What the owner added to lines it rewrote (a line it added whole has no item).
                let mut contributed = Vec::new();
                for k in &ks {
                    let c = &s.removed[*k].contribs[lost[&(path.clone(), *k)]];
                    if let Some(w) = &c.words
                        && contributed.len() < TEXT_LINES
                    {
                        let mut item = json!({"line": s.removed[*k].line, "words": w, "after": c.after});
                        if !c.before.is_empty() {
                            item["before"] = json!(c.before);
                        }
                        contributed.push(item);
                    }
                }
                let key = format!(
                    "{path}\0{origin}\0{}",
                    ks.iter().map(|k| s.removed[*k].norm.as_str()).collect::<Vec<_>>().join("\n")
                );
                let mut e = json!({
                    "path": path, "lines": [r0.line, s.removed[last].line], "agent": owner.agent,
                    "intent": owner.intent, "changeset": origin, "change": owner.change, "text": text,
                });
                if !contributed.is_empty() {
                    e["contributed"] = json!(contributed);
                }
                if !owner.landed {
                    e["landed"] = json!(false);
                }
                if cut {
                    e["text_cut"] = json!(true);
                }
                entries.push(e);
                keys.push(key);
            }
        }
        // Every entry: `put` cuts a reply at `MAX`, and a land holds on any of them.
        let mut report = Report::default();
        let mut send = Vec::new();
        for (e, key) in entries.into_iter().zip(&keys) {
            if scope == Scope::New && state.reported.contains(key) {
                report.earlier += 1;
            } else {
                send.push(e);
            }
        }
        if scope == Scope::New {
            state.reported = keys.into_iter().collect();
        }
        report.entries = send;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_in_order() {
        assert!(kept_in(&words("value.lower()"), &words("value.lower().strip()")));
        assert!(kept_in(&words("link[key] = value"), &words("link[key.lower()] = value")));
        assert!(!kept_in(&words("value = value.lower()"), &words("value = value")));
        assert!(!kept_in(&words("a(b)"), &words("b(a)")));
        assert!(!kept_in(&[], &words("x")));
    }

    #[test]
    fn a_contribution_is_what_the_hunk_added_and_it_is_kept_when_its_words_stay() {
        let base = words("normed_value = normed_value.casefold()");
        let theirs = words("normed_value = normed_value.strip().casefold()");
        let strip = contributed(&theirs, &base);
        assert_eq!(strip, ["strip"]);
        assert!(kept_on(&strip, &[words("normed_value = normed_value.strip().lower()")]));
        assert!(!kept_on(&strip, &[words("normed_value = normed_value.casefold()")]));
        assert!(!kept_on(&strip, &[words("normed_value = normed_value.lower()")]));
        // Split over two adjacent added lines.
        assert!(kept_on(&strip, &[words("x = (normed_value"), words("  .strip())")]));
        // A line added with no counterpart contributes every word.
        assert_eq!(contributed(&theirs, &[]), theirs);
        // Each replaced word cancels one occurrence.
        assert_eq!(contributed(&words("a = f(a, a)"), &words("a = 1")), ["f", "a", "a"]);
        // Only operators changed: no word of its own (the caller then takes the whole line).
        assert!(contributed(&words("x >= 0"), &words("x > 0")).is_empty());
    }

    #[test]
    fn normalised_lines_ignore_whitespace_and_line_ends() {
        assert_eq!(norm("    x = 1\r\n"), norm("x = 1"));
        assert!(trivial(&norm("    }\n")) && !trivial(&norm("pass")));
        let old = ["a\n", "}\n", "b\n"];
        let new = ["  a\r\n", "c\n"];
        let h = crate::xdiff::Hunk { before: 0..3, after: 0..2 };
        assert_eq!(delta(&old, &new, &[h]), HashMap::from([("b".to_string(), 1), ("c".to_string(), -1)]));
    }
}
