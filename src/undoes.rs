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
//! - it was not extended: a line whose words (identifiers, numbers, words of strings), in order,
//!   all appear on one added line of the same diff hunk, or on two adjacent added lines, was
//!   rewritten around its words (`value.lower()` -> `value.lower().strip()`) and is quiet;
//!   dropping a word warns (`value.lower()` -> `value`). Operators are not words, so
//!   `x > 0` -> `x >= 0` is quiet: the rule trades that miss for no warning on extensions;
//! - it has a letter or digit (a lone `}` never warns, but joins a run of lines around it).
//!
//! Per change, the scan of each path is kept while the change's base and the path's head blob
//! stay the same, so an `apply` to another file costs no diff or blame. Entries already sent in an
//! earlier `apply` or `resolve` reply are counted (`undoes_earlier`), not repeated.

use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::model::{Change, ChangeState, Entry, MODE_GITLINK};
use crate::repo::Inner;

/// At most this many entries per reply; the rest are counted in `undoes_more`.
pub const MAX: usize = 20;
pub const ADVICE: &str = "These lines were written by another agent for its own task (landed, or the open change you \
are stacked on). Keep them unless your task says to remove them; if both tasks cannot hold, say so in your message \
instead of removing its work.";
/// Removed text per entry: at most this many lines and bytes (`text_cut` past that).
const TEXT_LINES: usize = 8;
const TEXT_BYTES: usize = 800;

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
    origin: String,
    rewritten: bool,
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
    pub entries: Vec<Value>,
    pub more: usize,
    pub earlier: usize,
}

impl Report {
    /// Writes `undoes` (always), and `undoes_advice`, `undoes_more`, `undoes_earlier` when they
    /// apply, into a reply, keeping only entries whose path `can_read` allows.
    pub fn put(self, v: &mut Value, can_read: &dyn Fn(&str) -> bool) {
        let entries: Vec<Value> =
            self.entries.into_iter().filter(|e| e["path"].as_str().is_some_and(can_read)).collect();
        if !entries.is_empty() {
            v["undoes_advice"] = json!(ADVICE);
        }
        if self.more > 0 {
            v["undoes_more"] = json!(self.more);
        }
        if self.earlier > 0 {
            v["undoes_earlier"] = json!(self.earlier);
        }
        v["undoes"] = Value::Array(entries);
    }
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

    fn scan(&mut self, base: &str, path: &str, before: Option<&Entry>, after: Option<&Entry>) -> Result<Scan> {
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
        for h in hunks.into_iter().filter(|h| !h.before.is_empty()) {
            let added: Vec<Vec<&str>> = new_lines[h.after.clone()].iter().map(|l| words(l)).collect();
            for i in h.before {
                let Some(&(o, _)) = origins.get(i) else { continue };
                let origin = &blamed.origins[o as usize];
                if origin.starts_with("git:") {
                    continue;
                }
                let text = old_lines.get(i).copied().unwrap_or_default();
                let w = words(text);
                let rewritten = added.iter().enumerate().any(|(k, a)| {
                    kept_in(&w, a)
                        || added.get(k + 1).is_some_and(|b| kept_in(&w, &[a.as_slice(), b.as_slice()].concat()))
                });
                s.removed.push(Removed {
                    line: i + 1,
                    text: text.to_string(),
                    norm: norm(text),
                    origin: origin.clone(),
                    rewritten,
                });
            }
        }
        Ok(s)
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
            let s = self.scan(&ch.base, path, base_m.get(path), after.as_ref())?;
            state.paths.insert(path.clone(), s);
        }
        // Candidates: removed lines of another agent's work that were not extended.
        let mut owners: HashMap<String, Option<Owner>> = HashMap::new();
        let mut cands: Vec<(String, usize)> = Vec::new();
        for (path, s) in &state.paths {
            for (k, r) in s.removed.iter().enumerate() {
                if r.rewritten || trivial(&r.norm) {
                    continue;
                }
                if !owners.contains_key(&r.origin) {
                    let o = self.owner(&r.origin, ch)?;
                    owners.insert(r.origin.clone(), o);
                }
                if owners[&r.origin].is_some() {
                    cands.push((path.clone(), k));
                }
            }
        }
        let mut lost: HashSet<(String, usize)> = HashSet::new();
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
            for (path, k) in &cands {
                let n = state.paths[path].removed[*k].norm.as_str();
                if !deficit.contains_key(n) {
                    let d: i64 = state.paths.values().filter_map(|s| s.delta.as_ref()?.get(n)).sum();
                    deficit.insert(n, d);
                }
                let d = deficit.get_mut(n).expect("set above");
                if *d > 0 {
                    *d -= 1;
                    lost.insert((path.clone(), *k));
                }
            }
        }
        // Runs of lost lines with one origin; trivial removed lines of that origin join a run.
        let mut entries = Vec::new();
        let mut keys = Vec::new();
        for (path, s) in &state.paths {
            let mut runs: Vec<(usize, usize, Vec<usize>)> = Vec::new(); // (first k, last k, lost ks)
            for (k, r) in s.removed.iter().enumerate() {
                let is_lost = lost.contains(&(path.clone(), k));
                let joins = |run: &(usize, usize, Vec<usize>)| {
                    let last = &s.removed[run.1];
                    last.origin == r.origin && last.line + 1 == r.line
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
                let owner = owners[&r0.origin].clone().expect("lost lines have an owner");
                let mut text = String::new();
                let mut cut = false;
                for (n, r) in s.removed[first..=last].iter().enumerate() {
                    if n >= TEXT_LINES || text.len() + r.text.len() > TEXT_BYTES {
                        cut = true;
                        break;
                    }
                    text.push_str(&r.text);
                }
                let key = format!(
                    "{path}\0{}\0{}",
                    r0.origin,
                    ks.iter().map(|k| s.removed[*k].norm.as_str()).collect::<Vec<_>>().join("\n")
                );
                let mut e = json!({
                    "path": path, "lines": [r0.line, s.removed[last].line], "agent": owner.agent,
                    "intent": owner.intent, "changeset": r0.origin, "change": owner.change, "text": text,
                });
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
        if send.len() > MAX {
            report.more = send.len() - MAX;
            send.truncate(MAX);
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
    fn normalised_lines_ignore_whitespace_and_line_ends() {
        assert_eq!(norm("    x = 1\r\n"), norm("x = 1"));
        assert!(trivial(&norm("    }\n")) && !trivial(&norm("pass")));
        let old = ["a\n", "}\n", "b\n"];
        let new = ["  a\r\n", "c\n"];
        let h = crate::xdiff::Hunk { before: 0..3, after: 0..2 };
        assert_eq!(delta(&old, &new, &[h]), HashMap::from([("b".to_string(), 1), ("c".to_string(), -1)]));
    }
}
