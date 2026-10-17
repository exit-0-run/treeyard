//! Coordination: the event bus, declared intents, live collisions between open changes, and
//! the semantic guard that flags edits a rebase made vanish.

use anyhow::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

use crate::config::globs;
use crate::git::object_id;
use crate::merge::{Blobs, Merged, merge3};
use crate::model::{Change, Entry, MODE_GITLINK, Manifest};
use crate::repo::{Inner, now};

/// Events kept in memory for `events` polls; older ones are dropped (the reply says so).
const KEEP: usize = 20_000;

/// In-memory event log with sequence numbers, a condvar for long polls and a broadcast channel
/// for server-sent events. `epoch` changes on every server start: a client that sees a new
/// epoch starts again from seq 0.
pub struct Events {
    log: Mutex<(u64, VecDeque<Value>)>,
    cv: Condvar,
    tx: broadcast::Sender<Value>,
    pub epoch: String,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub change: Option<String>,
    pub agent: Option<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
}

impl Filter {
    pub fn matches(&self, ev: &Value) -> bool {
        let has = |field: &str, want: &str| ev[field].as_array().is_some_and(|a| a.iter().any(|x| x == want));
        self.change.as_deref().is_none_or(|c| has("changes", c))
            && self.agent.as_deref().is_none_or(|a| has("agents", a))
            && (self.kinds.is_empty() || self.kinds.iter().any(|k| ev["kind"] == k.as_str()))
    }
}

impl Default for Events {
    fn default() -> Self {
        Self {
            log: Mutex::new((0, VecDeque::new())),
            cv: Condvar::new(),
            tx: broadcast::channel(4096).0,
            epoch: format!("{:016x}", rand::random::<u64>()),
        }
    }
}

impl Events {
    pub fn publish(&self, events: Vec<Value>) {
        if events.is_empty() {
            return;
        }
        let mut g = self.log.lock().unwrap_or_else(|e| e.into_inner());
        for mut ev in events {
            g.0 += 1;
            ev["seq"] = json!(g.0);
            g.1.push_back(ev.clone());
            if g.1.len() > KEEP {
                g.1.pop_front();
            }
            let _ = self.tx.send(ev);
        }
        drop(g);
        self.cv.notify_all();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.tx.subscribe()
    }

    /// Events after `since` that match, waiting up to `wait` for the first one.
    pub fn poll(&self, since: u64, f: &Filter, limit: usize, wait: Duration) -> Value {
        let deadline = std::time::Instant::now() + wait;
        let mut g = self.log.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let oldest = g.1.front().and_then(|e| e["seq"].as_u64()).unwrap_or(g.0 + 1);
            let events: Vec<Value> =
                g.1.iter()
                    .filter(|e| e["seq"].as_u64().unwrap_or(0) > since && f.matches(e))
                    .take(limit)
                    .cloned()
                    .collect();
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if !events.is_empty() || left.is_zero() {
                let next = events.last().and_then(|e| e["seq"].as_u64()).unwrap_or(g.0.max(since));
                return json!({
                    "epoch": self.epoch, "events": events, "next": next, "latest": g.0,
                    "dropped": since + 1 < oldest && since < g.0,
                });
            }
            g = self.cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// Blob access for trial merges: reads through, but merged results are never stored.
struct DryBlobs<'a>(&'a mut Inner);

impl Blobs for DryBlobs<'_> {
    fn read_blob(&mut self, id: &str) -> Result<Vec<u8>> {
        self.0.blob(id)
    }
    fn write_blob(&mut self, data: &[u8]) -> Result<String> {
        Ok(object_id("blob", data))
    }
}

/// Changed regions of `b` against `a` as half-open line ranges of `a` (1-based); a pure
/// insertion is an empty range at the line it goes before.
pub fn changed_ranges(a: &str, b: &str) -> Vec<(usize, usize)> {
    let patch = diffy::DiffOptions::new().set_context_len(0).create_patch(a, b);
    patch
        .hunks()
        .iter()
        .map(|h| {
            let r = h.old_range();
            let start = if r.is_empty() { r.start() + 1 } else { r.start() };
            (start, start + r.len())
        })
        .collect()
}

/// One hunk as unified-diff text.
fn hunk_text(h: &diffy::Hunk<'_, str>) -> String {
    let (o, n) = (h.old_range(), h.new_range());
    let mut s = format!("@@ -{},{} +{},{} @@\n", o.start(), o.len(), n.start(), n.len());
    for l in h.lines() {
        let (sign, text) = match l {
            diffy::Line::Context(t) => (' ', t),
            diffy::Line::Delete(t) => ('-', t),
            diffy::Line::Insert(t) => ('+', t),
        };
        s.push(sign);
        s.push_str(text);
        if !text.ends_with('\n') {
            s.push('\n');
        }
    }
    s
}

/// Two sets of regions overlap or touch (the cases where diff3 conflicts).
fn regions_meet(a: &[(usize, usize)], b: &[(usize, usize)]) -> bool {
    a.iter().any(|x| b.iter().any(|y| x.0 <= y.1 && y.0 <= x.1))
}

/// The lines `[start, end)` of the definition of `name` in `text`: from the line that defines it
/// to the next line at the same or smaller indent (a closing bracket line still belongs to it).
pub fn symbol_range(text: &str, name: &str) -> Option<(usize, usize)> {
    let def = Regex::new(&format!(
        r"^\s*(?:(?:pub(?:\([^)]*\))?|export|async|unsafe|static|default|public|private|protected|extern)\s+)*(?:fn|struct|enum|trait|impl(?:<[^>]*>)?|type|const|let|var|mod|def|class|function|interface|func|macro_rules!)\s+[^\n]*?\b{}\b",
        regex::escape(name)
    ))
    .ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let start = lines.iter().position(|l| def.is_match(l))?;
    let ind = indent(lines[start]);
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        let t = l.trim_start();
        if t.is_empty() || indent(l) > ind {
            continue;
        }
        end = if t.starts_with(['}', ')', ']']) { i + 1 } else { i };
        break;
    }
    Some((start + 1, end + 1))
}

/// `name` or `path#name`.
fn split_symbol(s: &str) -> (Option<&str>, &str) {
    match s.rsplit_once('#') {
        Some((p, n)) => (Some(p), n),
        None => (None, s),
    }
}

impl Inner {
    pub fn emit(&mut self, kind: &str, changes: &[&str], agents: &[&str], body: Value) {
        let mut ev = json!({"kind": kind, "time": now(), "changes": changes, "agents": agents});
        if let Value::Object(m) = body {
            for (k, v) in m {
                ev[k] = v;
            }
        }
        self.outbox.push(ev);
    }

    /// Main changesets that are first-parent ancestors get a height (distance from the root),
    /// to pick the older of two bases.
    pub fn height(&mut self, id: &str) -> Result<u64> {
        let mut chain = Vec::new();
        let mut cur = id.to_string();
        let mut h = loop {
            if let Some(h) = self.heights.get(&cur) {
                break Some(*h);
            }
            chain.push(cur.clone());
            match self.first_parent(&cur)? {
                Some(p) => cur = p,
                None => break None,
            }
        };
        for c in chain.into_iter().rev() {
            let next = h.map_or(0, |x| x + 1);
            self.heights.insert(c, next);
            h = Some(next);
        }
        Ok(h.unwrap_or(0))
    }

    /// True if `a` is `b` or a first-parent ancestor of `b`.
    pub fn is_ancestor(&mut self, a: &str, b: &str) -> Result<bool> {
        let ha = self.height(a)?;
        let mut cur = b.to_string();
        while self.height(&cur)? > ha {
            match self.first_parent(&cur)? {
                Some(p) => cur = p,
                None => return Ok(false),
            }
        }
        Ok(cur == a)
    }

    /// First parent of a changeset, cached (changesets are immutable).
    pub fn first_parent(&mut self, id: &str) -> Result<Option<String>> {
        if let Some(p) = self.parents.get(id) {
            return Ok(p.clone());
        }
        let p = self.changeset(id)?.parents.first().cloned();
        self.parents.insert(id.to_string(), p.clone());
        Ok(p)
    }

    /// True if one change is stacked on the other (overlap between them is expected).
    fn stacked_pair(&self, a: &Change, b: &Change) -> Result<bool> {
        let on = |x: &Change, y: &Change| -> Result<bool> {
            Ok(self.changeset(&x.base)?.meta.change.as_deref() == Some(y.id.as_str()))
        };
        Ok(on(a, b)? || on(b, a)?)
    }

    fn edited(&self, ch: &Change) -> Result<BTreeSet<String>> {
        if ch.head == ch.base {
            Ok(BTreeSet::new())
        } else {
            Ok(self.changeset(&ch.head)?.changes.into_keys().collect())
        }
    }

    /// Declared path globs expanded against `m`, plus the literal (glob-free) paths as given.
    fn declared_paths(&self, ch: &Change, m: &Manifest) -> Result<BTreeSet<String>> {
        if ch.declared.paths.is_empty() {
            return Ok(BTreeSet::new());
        }
        let g = globs(&ch.declared.paths)?;
        let mut out: BTreeSet<String> = m.keys().filter(|p| g.is_match(p.as_str())).cloned().collect();
        out.extend(
            ch.declared.paths.iter().filter(|p| !p.contains(['*', '?', '['])).map(|p| p.trim_matches('/').to_string()),
        );
        Ok(out)
    }

    fn text_at(&mut self, m: &Manifest, path: &str) -> Result<Option<String>> {
        match m.get(path) {
            Some(e) if e.mode != MODE_GITLINK => {
                let data = self.blob(&e.blob)?;
                Ok(String::from_utf8(data).ok().filter(|t| !t.as_bytes().iter().take(8000).any(|b| *b == 0)))
            }
            _ => Ok(None),
        }
    }

    /// Declared symbols of `ch` that the edits of `other` reach: same name declared by both, or
    /// a changed region of `other` inside the symbol's definition (in `other`'s base).
    fn symbol_hits(&mut self, ch: &Change, other: &Change) -> Result<Vec<String>> {
        let mut hits = Vec::new();
        if ch.declared.symbols.is_empty() {
            return Ok(hits);
        }
        let theirs: BTreeSet<(Option<String>, String)> = other
            .declared
            .symbols
            .iter()
            .map(|s| {
                let (p, n) = split_symbol(s);
                (p.map(str::to_string), n.to_string())
            })
            .collect();
        let edited = self.edited(other)?;
        let (base_m, head_m) = (self.manifest(&other.base)?, self.manifest(&other.head)?);
        for s in &ch.declared.symbols {
            let (path, name) = split_symbol(s);
            let named =
                theirs.iter().any(|(p, n)| n == name && (p.is_none() || path.is_none() || p.as_deref() == path));
            let mut reached = false;
            for f in edited.iter().filter(|f| path.is_none_or(|p| p == f.as_str())) {
                let (Some(old), new) = (self.text_at(&base_m, f)?, self.text_at(&head_m, f)?) else { continue };
                if let Some(r) = crate::symbols::definition_range(f, &old, name).or_else(|| symbol_range(&old, name))
                    && regions_meet(&[r], &changed_ranges(&old, new.as_deref().unwrap_or("")))
                {
                    reached = true;
                    break;
                }
            }
            if named || reached {
                hits.push(s.clone());
            }
        }
        Ok(hits)
    }

    /// Open changes whose declared or edited paths and symbols overlap what `ch` declared.
    /// Called by `open_change` and `declare`; publishes one `intent_overlap` event per hit.
    pub fn intent_overlaps(&mut self, ch: &Change) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        if ch.declared.is_empty() {
            return Ok(out);
        }
        let main = self.main()?;
        let m = self.manifest(&main)?;
        let mine = self.declared_paths(ch, &m)?;
        let mine_glob = globs(&ch.declared.paths)?;
        for other in self.store.changes(Some("open"))? {
            if other.id == ch.id || self.stacked_pair(ch, &other)? {
                continue;
            }
            let edited = self.edited(&other)?;
            let mut paths: BTreeMap<String, &str> = BTreeMap::new();
            for p in edited.iter().filter(|p| mine_glob.is_match(p.as_str()) || mine.contains(*p)) {
                paths.insert(p.clone(), "edited");
            }
            for p in self.declared_paths(&other, &m)?.intersection(&mine) {
                paths.entry(p.clone()).or_insert("declared");
            }
            let symbols = self.symbol_hits(ch, &other)?;
            if paths.is_empty() && symbols.is_empty() {
                continue;
            }
            let v = json!({
                "change": other.id, "agent": other.agent, "intent": other.intent,
                "paths": paths.keys().collect::<Vec<_>>(), "via": paths, "symbols": symbols,
            });
            self.emit(
                "intent_overlap",
                &[&ch.id, &other.id],
                &[&ch.agent, &other.agent],
                json!({"change": ch.id, "other": other.id, "paths": v["paths"], "symbols": symbols}),
            );
            out.push(v);
        }
        Ok(out)
    }

    /// Live collisions of `ch` with every other open change: shared edited paths get a trial
    /// 3-way merge (merge base = the older of the two bases) and hunk ranges; edits inside
    /// another change's declared paths or symbols count too. New or changed collisions are
    /// published as `collision` events.
    pub fn collisions(&mut self, ch: &Change) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mine = self.edited(ch)?;
        if mine.is_empty() {
            return Ok(out);
        }
        let main = self.main()?;
        let main_m = self.manifest(&main)?;
        let my_head = self.manifest(&ch.head)?;
        for other in self.store.changes(Some("open"))? {
            if other.id == ch.id || self.stacked_pair(ch, &other)? {
                continue;
            }
            let theirs = self.edited(&other)?;
            let shared: Vec<String> = mine.intersection(&theirs).cloned().collect();
            let declared: Vec<String> = self
                .declared_paths(&other, &main_m)?
                .intersection(&mine)
                .filter(|p| !theirs.contains(*p))
                .cloned()
                .collect();
            let symbols = self.symbol_hits(&other, ch)?;
            if shared.is_empty() && declared.is_empty() && symbols.is_empty() {
                continue;
            }
            let older = if self.height(&ch.base)? <= self.height(&other.base)? { &ch.base } else { &other.base };
            let base_m = self.manifest(older)?;
            let their_head = self.manifest(&other.head)?;
            let mut files = Vec::new();
            for path in shared {
                let (b, o, t) =
                    (base_m.get(&path).cloned(), my_head.get(&path).cloned(), their_head.get(&path).cloned());
                let merged = merge3(&mut DryBlobs(self), b.as_ref(), o.as_ref(), t.as_ref())?;
                let kind = if matches!(merged, Merged::Conflict { .. }) { "conflict" } else { "overlap" };
                let (bt, ot, tt) =
                    (self.text_at(&base_m, &path)?, self.text_at(&my_head, &path)?, self.text_at(&their_head, &path)?);
                let (mh, th) = match (bt, ot, tt) {
                    (Some(b), o, t) => {
                        (changed_ranges(&b, o.as_deref().unwrap_or("")), changed_ranges(&b, t.as_deref().unwrap_or("")))
                    }
                    _ => (vec![], vec![]),
                };
                files.push(json!({"path": path, "kind": kind, "hunks": {"mine": mh, "theirs": th}}));
            }
            for path in declared {
                files.push(json!({"path": path, "kind": "declared"}));
            }
            for s in &symbols {
                files.push(json!({"symbol": s, "kind": "symbol"}));
            }
            let (a, b) = if ch.id < other.id { (&ch.id, &other.id) } else { (&other.id, &ch.id) };
            for f in &files {
                let what = f["path"].as_str().or(f["symbol"].as_str()).unwrap_or_default();
                let key = format!("{a}|{b}|{what}");
                let kind = f["kind"].as_str().unwrap_or_default().to_string();
                if self.seen_collisions.get(&key) != Some(&kind) {
                    self.seen_collisions.insert(key, kind);
                    self.emit(
                        "collision",
                        &[&ch.id, &other.id],
                        &[&ch.agent, &other.agent],
                        json!({"change": ch.id, "other": other.id, "file": f}),
                    );
                }
            }
            let worst = if files.iter().any(|f| f["kind"] == "conflict") { "conflict" } else { "overlap" };
            out.push(json!({"change": other.id, "agent": other.agent, "intent": other.intent, "kind": worst, "files": files}));
        }
        Ok(out)
    }

    /// Drops collision memory of a change that left the open set.
    pub fn forget(&mut self, id: &str) {
        self.seen_collisions.retain(|k, _| !k.split('|').take(2).any(|x| x == id));
    }

    /// Semantic guard: our edit at a path (base -> ours) that the rebase result no longer
    /// carries against theirs. Returns the vanished hunks as unified-diff text.
    pub fn absorbed(
        &mut self,
        b: Option<&Entry>,
        o: Option<&Entry>,
        t: Option<&Entry>,
        result: Option<&Entry>,
    ) -> Result<Option<(bool, Vec<String>)>> {
        if o == b || t == b {
            return Ok(None);
        }
        let text = |s: &mut Self, e: Option<&Entry>| -> Result<Option<String>> {
            match e {
                None => Ok(Some(String::new())),
                Some(e) if e.mode == MODE_GITLINK => Ok(None),
                Some(e) => Ok(String::from_utf8(s.blob(&e.blob)?).ok()),
            }
        };
        let whole = result == t;
        let (Some(bt), Some(ot)) = (text(self, b)?, text(self, o)?) else {
            return Ok(whole.then(|| (true, vec!["(binary)".to_string()])));
        };
        let ours = diffy::DiffOptions::new().set_context_len(0).create_patch(&bt, &ot);
        let hunks: Vec<String> = ours.hunks().iter().map(hunk_text).collect();
        if whole {
            return Ok(Some((true, hunks)));
        }
        let (Some(tt), Some(rt)) = (text(self, t)?, text(self, result)?) else { return Ok(None) };
        let after = diffy::DiffOptions::new().set_context_len(0).create_patch(&tt, &rt);
        let signed = |l: &diffy::Line<'_, str>| match l {
            diffy::Line::Insert(s) => Some(format!("+{s}")),
            diffy::Line::Delete(s) => Some(format!("-{s}")),
            diffy::Line::Context(_) => None,
        };
        let kept: BTreeSet<String> = after.hunks().iter().flat_map(|h| h.lines().iter().filter_map(signed)).collect();
        let vanished: Vec<String> = ours
            .hunks()
            .iter()
            .filter(|h| {
                let lines: Vec<String> =
                    h.lines().iter().filter_map(signed).filter(|l| l.trim() != "+" && l.trim() != "-").collect();
                !lines.is_empty() && lines.iter().all(|l| !kept.contains(l))
            })
            .map(hunk_text)
            .collect();
        Ok((!vanished.is_empty()).then_some((false, vanished)))
    }

    /// Records a review task for a change and publishes a `review` event.
    pub fn add_review(&mut self, ch: &Change, kind: &str, detail: Value) -> Result<i64> {
        let body = json!({"kind": kind, "change": ch.id, "agent": ch.agent, "intent": ch.intent, "detail": detail});
        let id = self.store.add_review(&ch.id, &body)?;
        self.emit(
            "review",
            &[&ch.id],
            &[&ch.agent],
            json!({"review": id, "review_kind": kind, "detail": body["detail"]}),
        );
        Ok(id)
    }

    pub fn reviews_view(&self, state: Option<&str>, change: Option<&str>) -> Result<Value> {
        let rows = self.store.reviews(state)?;
        let list: Vec<Value> = rows
            .into_iter()
            .filter(|(_, c, _, _)| change.is_none_or(|x| x == c))
            .map(|(id, _, state, mut body)| {
                body["id"] = json!(id);
                body["state"] = json!(state);
                body
            })
            .collect();
        Ok(json!({"reviews": list}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_symbols() {
        let a = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n";
        let b = a.replace("    2", "    3");
        assert_eq!(changed_ranges(a, &b), vec![(6, 7)]);
        assert_eq!(symbol_range(a, "b"), Some((5, 8)));
        assert_eq!(symbol_range(a, "a"), Some((1, 4)));
        assert!(regions_meet(&[(6, 7)], &[symbol_range(a, "b").unwrap()]));
        assert!(!regions_meet(&[(6, 7)], &[symbol_range(a, "a").unwrap()]));
        let py = "class C:\n    def run(self):\n        return 1\n\n    def stop(self):\n        pass\n";
        // The blank line before the next method belongs to the range; the next method does not.
        assert_eq!(symbol_range(py, "run"), Some((2, 5)));
        // A pure insertion sits before the line it precedes.
        assert_eq!(changed_ranges("x\ny\n", "x\nnew\ny\n"), vec![(2, 2)]);
    }

    #[test]
    fn event_poll_filters_and_waits() {
        let ev = Events::default();
        ev.publish(vec![json!({"kind": "landed", "changes": ["ch_a"], "agents": ["a"]})]);
        ev.publish(vec![json!({"kind": "collision", "changes": ["ch_a", "ch_b"], "agents": ["a", "b"]})]);
        let f = Filter { change: Some("ch_b".into()), ..Filter::default() };
        let r = ev.poll(0, &f, 10, Duration::ZERO);
        assert_eq!(r["events"].as_array().unwrap().len(), 1);
        assert_eq!(r["next"], 2);
        let t = std::time::Instant::now();
        let r = ev.poll(2, &Filter::default(), 10, Duration::from_millis(50));
        assert!(r["events"].as_array().unwrap().is_empty() && t.elapsed() >= Duration::from_millis(50));
    }
}
