//! The repository: changes, server-side rebase and landing, read queries and git export.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rand::Rng;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::git::{self, Odb, Signature};
use crate::merge::{Blobs, Merged, merge3};
use crate::model::*;
use crate::store::Store;

const TEXT_LIMIT: usize = 256 * 1024;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// One edit in `apply`. Exactly one of `content`, `content_b64`, `delete`, `replace`, or only
/// `executable` to flip the mode.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub path: String,
    pub content: Option<String>,
    pub content_b64: Option<String>,
    pub delete: Option<bool>,
    pub replace: Option<Replace>,
    pub executable: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    pub old: String,
    pub new: String,
    #[serde(default)]
    pub all: bool,
}

pub struct Repo {
    inner: Mutex<Inner>,
}

pub struct Inner {
    pub store: Store,
    pub odb: Odb,
    pub branch: String,
    manifests: HashMap<String, Manifest>,
}

fn paths(data: &Path) -> (PathBuf, PathBuf) {
    (data.join("rootstock.db"), data.join("git"))
}

impl Repo {
    /// Creates a data directory from the tip of `branch` in the git repo at `src`. The export
    /// repo is a bare clone of `src`, so landed commits continue its history.
    pub fn init(data: &Path, src: &Path, branch: &str) -> Result<String> {
        let (db, git_dir) = paths(data);
        if db.exists() || git_dir.exists() {
            bail!("{} already holds a rootstock repo", data.display());
        }
        fs::create_dir_all(data)?;
        let (src_s, git_s) = (src.to_string_lossy(), git_dir.to_string_lossy());
        git::run(None, &["clone", "--bare", "--quiet", &src_s, &git_s])?;
        let commit = git::run(Some(&git_dir), &["rev-parse", "--verify", &format!("refs/heads/{branch}^{{commit}}")])?
            .trim()
            .to_string();
        git::run(Some(&git_dir), &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])?;
        let time: i64 = git::run(Some(&git_dir), &["show", "-s", "--format=%ct", &commit])?.trim().parse()?;
        let changes = git::ls_tree(&git_dir, &commit)?.into_iter().map(|(p, e)| (p, Some(e))).collect();
        let root = Changeset {
            parents: vec![],
            changes,
            meta: Meta {
                agent: "import".into(),
                intent: format!("import {branch} at {commit}"),
                time,
                ..Meta::default()
            },
        };
        let store = Store::open(&db)?;
        store.tx(|s| {
            let id = s.put_changeset(&root)?;
            s.map_commit(&id, &commit)?;
            s.set_ref("main", &id)?;
            s.set_ref("branch", branch)?;
            Ok(id)
        })
    }

    pub fn open(data: &Path) -> Result<Self> {
        let (db, git_dir) = paths(data);
        if !db.exists() {
            bail!("{} is not a rootstock repo (run `rootstock init` first)", data.display());
        }
        let store = Store::open(&db)?;
        let branch = store.get_ref("branch")?.context("no branch ref")?;
        let mut inner = Inner { store, odb: Odb::open(&git_dir)?, branch, manifests: HashMap::new() };
        // Heals a crash between the metadata commit and the git ref write.
        let main = inner.main()?;
        let commit = inner.commit_for(&main)?;
        inner.odb.update_ref(&format!("refs/heads/{}", inner.branch), &commit)?;
        Ok(Self { inner: Mutex::new(inner) })
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut Inner) -> Result<R>) -> Result<R> {
        f(&mut self.lock())
    }
}

impl Blobs for Inner {
    fn read_blob(&mut self, id: &str) -> Result<Vec<u8>> {
        self.blob(id)
    }
    fn write_blob(&mut self, data: &[u8]) -> Result<String> {
        self.put_blob(data)
    }
}

fn check_path(path: &str) -> Result<()> {
    let bad = path.is_empty()
        || path.contains('\0')
        || path.split('/').any(|c| c.is_empty() || c == "." || c == ".." || c.eq_ignore_ascii_case(".git"));
    if bad {
        bail!("invalid path {path:?}: use a relative path like src/main.rs");
    }
    Ok(())
}

fn as_text(data: &[u8]) -> Option<String> {
    if data.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    String::from_utf8(data.to_vec()).ok()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn entry_json(e: &Option<Entry>) -> Value {
    match e {
        Some(e) => json!({"blob": e.blob, "mode": format!("{:o}", e.mode)}),
        None => Value::Null,
    }
}

impl Inner {
    pub fn main(&self) -> Result<String> {
        self.store.get_ref("main")?.context("no main ref")
    }

    pub fn changeset(&self, id: &str) -> Result<Changeset> {
        self.store.changeset(id)?.with_context(|| format!("unknown changeset {id}"))
    }

    pub fn change(&self, id: &str) -> Result<Change> {
        self.store.change(id)?.with_context(|| format!("unknown change {id}"))
    }

    fn open_change_mut(&self, id: &str) -> Result<Change> {
        let ch = self.change(id)?;
        if ch.state != ChangeState::Open {
            bail!("change {id} is {}", ch.state.as_str());
        }
        Ok(ch)
    }

    /// `main` (default), a change id (its head), a git commit id, or a changeset id or prefix.
    pub fn resolve_at(&self, at: Option<&str>) -> Result<String> {
        let at = at.unwrap_or("main").trim();
        if at.is_empty() || at == "main" {
            return self.main();
        }
        if at.starts_with("ch_") {
            return Ok(self.change(at)?.head);
        }
        if at.len() == 40
            && let Some(cs) = self.store.changeset_of_commit(at)?
        {
            return Ok(cs);
        }
        let ids = self.store.changesets_by_prefix(at)?;
        match ids.as_slice() {
            [one] => Ok(one.clone()),
            [] => bail!("unknown revision {at:?}"),
            _ => bail!("ambiguous revision {at:?}: give more characters"),
        }
    }

    pub fn manifest(&mut self, id: &str) -> Result<Manifest> {
        let mut chain = Vec::new();
        let mut cur = id.to_string();
        let mut m = loop {
            if let Some(m) = self.manifests.get(&cur) {
                break m.clone();
            }
            let cs = self.changeset(&cur)?;
            let parent = cs.parents.first().cloned();
            chain.push((cur, cs));
            match parent {
                Some(p) => cur = p,
                None => break Manifest::new(),
            }
        };
        for (cid, cs) in chain.into_iter().rev() {
            for (path, e) in cs.changes {
                match e {
                    Some(e) => m.insert(path, e),
                    None => m.remove(&path),
                };
            }
            self.manifests.insert(cid, m.clone());
        }
        Ok(m)
    }

    pub fn blob(&mut self, id: &str) -> Result<Vec<u8>> {
        if let Some(d) = self.store.blob(id)? {
            return Ok(d);
        }
        match self.odb.read(id)? {
            Some((kind, data)) if kind == "blob" => Ok(data),
            Some((kind, _)) => bail!("{id} is a {kind}, not a blob"),
            None => bail!("missing blob {id}"),
        }
    }

    pub fn put_blob(&mut self, data: &[u8]) -> Result<String> {
        let id = git::object_id("blob", data);
        if !self.odb.has(&id) {
            self.store.put_blob(&id, data)?;
        }
        Ok(id)
    }

    fn own_changes(&self, ch: &Change) -> Result<Changes> {
        if ch.head == ch.base { Ok(Changes::new()) } else { Ok(self.changeset(&ch.head)?.changes) }
    }

    fn meta_for(ch: &Change, message: Option<String>) -> Meta {
        Meta {
            change: Some(ch.id.clone()),
            agent: ch.agent.clone(),
            model: ch.model.clone(),
            session: ch.session.clone(),
            intent: ch.intent.clone(),
            message,
            time: now(),
        }
    }

    // ---- changes -------------------------------------------------------------------------

    pub fn open_change(
        &mut self,
        agent: &str,
        intent: &str,
        model: Option<String>,
        session: Option<String>,
        base: Option<&str>,
    ) -> Result<Change> {
        if agent.trim().is_empty() || intent.trim().is_empty() {
            bail!("open_change needs a non-empty agent and intent");
        }
        let base = self.resolve_at(base)?;
        let id = format!("ch_{:012x}", rand::thread_rng().r#gen::<u64>() & 0xffff_ffff_ffff);
        let ch = Change {
            id,
            state: ChangeState::Open,
            agent: one_line(agent),
            model,
            session,
            intent: intent.trim().to_string(),
            base: base.clone(),
            head: base,
            predecessors: vec![],
            conflicts: vec![],
            landed_as: None,
            created: now(),
        };
        self.store.put_change(&ch)?;
        Ok(ch)
    }

    /// Sets paths of the change to the given entries (amend), clearing their conflicts.
    fn set_entries(
        &mut self,
        ch: &mut Change,
        entries: Vec<(String, Option<Entry>)>,
        message: Option<String>,
    ) -> Result<()> {
        let base_m = self.manifest(&ch.base)?;
        let mut changes = self.own_changes(ch)?;
        for (path, e) in entries {
            ch.conflicts.retain(|c| c.path != path);
            if e.as_ref() == base_m.get(&path) {
                changes.remove(&path);
            } else {
                changes.insert(path, e);
            }
        }
        let head = if changes.is_empty() {
            ch.base.clone()
        } else {
            let meta = Self::meta_for(ch, message);
            self.store.put_changeset(&Changeset { parents: vec![ch.base.clone()], changes, meta })?
        };
        if head != ch.head {
            ch.predecessors.push(std::mem::replace(&mut ch.head, head));
        }
        self.store.put_change(ch)
    }

    fn edit_entry(&mut self, e: &Edit, cur: Option<&Entry>) -> Result<Option<Entry>> {
        let given = [e.content.is_some(), e.content_b64.is_some(), e.delete == Some(true), e.replace.is_some()]
            .iter()
            .filter(|x| **x)
            .count();
        if given > 1 || (given == 0 && e.executable.is_none()) {
            bail!("edit of {}: give exactly one of content, content_b64, delete, replace", e.path);
        }
        if cur.is_some_and(|c| c.mode == MODE_GITLINK) {
            bail!("{} is a submodule; rootstock does not edit submodules", e.path);
        }
        if e.delete == Some(true) {
            if cur.is_none() {
                bail!("cannot delete {}: no such file", e.path);
            }
            return Ok(None);
        }
        let mode = match e.executable {
            Some(true) => MODE_EXEC,
            Some(false) => MODE_FILE,
            None => cur.map_or(MODE_FILE, |c| c.mode),
        };
        let data = if let Some(c) = &e.content {
            c.clone().into_bytes()
        } else if let Some(b) = &e.content_b64 {
            B64.decode(b).map_err(|err| anyhow!("content_b64 of {}: {err}", e.path))?
        } else if let Some(r) = &e.replace {
            let cur = cur.with_context(|| format!("replace in {}: no such file", e.path))?;
            let text = as_text(&self.blob(&cur.blob)?).with_context(|| format!("{} is not UTF-8 text", e.path))?;
            if r.old.is_empty() {
                bail!("replace in {}: old text is empty", e.path);
            }
            match text.matches(&r.old).count() {
                0 => bail!("replace in {}: old text not found", e.path),
                1 => text.replacen(&r.old, &r.new, 1).into_bytes(),
                n if r.all => {
                    let _ = n;
                    text.replace(&r.old, &r.new).into_bytes()
                }
                n => bail!("replace in {}: old text found {n} times; add context or set all=true", e.path),
            }
        } else {
            let cur = cur.with_context(|| format!("cannot change the mode of {}: no such file", e.path))?;
            return Ok(Some(Entry { blob: cur.blob.clone(), mode }));
        };
        Ok(Some(Entry { blob: self.put_blob(&data)?, mode }))
    }

    pub fn apply(&mut self, id: &str, edits: &[Edit], message: Option<String>) -> Result<Change> {
        let mut ch = self.open_change_mut(id)?;
        let mut work = self.manifest(&ch.head)?;
        let mut out = Vec::new();
        for e in edits {
            check_path(&e.path)?;
            let cur = work.get(&e.path).cloned();
            let new = self.edit_entry(e, cur.as_ref())?;
            match &new {
                Some(n) => {
                    if cur.is_none() {
                        check_no_collision(&work, &e.path)?;
                    }
                    work.insert(e.path.clone(), n.clone());
                }
                None => {
                    work.remove(&e.path);
                }
            }
            out.push((e.path.clone(), new));
        }
        self.set_entries(&mut ch, out, message)?;
        Ok(ch)
    }

    /// Settles one conflicted path by taking one side; `apply` on the path also settles it.
    pub fn resolve(&mut self, id: &str, path: &str, take: &str) -> Result<Change> {
        let mut ch = self.open_change_mut(id)?;
        let c = ch.conflicts.iter().find(|c| c.path == path).cloned();
        let c = c.with_context(|| format!("change {id} has no conflict at {path}"))?;
        let entry = match take {
            "ours" => c.ours,
            "theirs" => c.theirs,
            "base" => c.base,
            "delete" => None,
            other => bail!("take must be ours, theirs, base or delete, not {other:?}"),
        };
        self.set_entries(&mut ch, vec![(path.to_string(), entry)], None)?;
        Ok(ch)
    }

    pub fn abandon(&mut self, id: &str) -> Result<Change> {
        let mut ch = self.open_change_mut(id)?;
        ch.state = ChangeState::Abandoned;
        self.store.put_change(&ch)?;
        Ok(ch)
    }

    /// Main changesets after `from` up to `to` that touched `path`, newest first.
    fn touches(&self, from: &str, to: &str, path: &str) -> Result<Vec<Touch>> {
        let mut out = Vec::new();
        let mut cur = to.to_string();
        while cur != from && out.len() < 20 {
            let cs = self.changeset(&cur)?;
            if cs.changes.contains_key(path) {
                out.push(Touch {
                    changeset: cur.clone(),
                    change: cs.meta.change.clone(),
                    agent: cs.meta.agent.clone(),
                    intent: cs.meta.intent.clone(),
                });
            }
            match cs.parents.first() {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        Ok(out)
    }

    fn rebase_onto(&mut self, ch: &mut Change, onto: &str) -> Result<()> {
        if ch.base == onto {
            return Ok(());
        }
        if !ch.conflicts.is_empty() {
            bail!("change {} has unresolved conflicts; resolve them before rebasing", ch.id);
        }
        let base_m = self.manifest(&ch.base)?;
        let theirs_m = self.manifest(onto)?;
        let mut changes = Changes::new();
        let mut conflicts = Vec::new();
        for (path, ours) in self.own_changes(ch)? {
            let (b, t) = (base_m.get(&path), theirs_m.get(&path));
            let (result, kind) = match merge3(self, b, ours.as_ref(), t)? {
                Merged::Clean(r) => (r, None),
                Merged::Conflict { kind, materialized } => (materialized, Some(kind)),
            };
            if let Some(kind) = kind {
                conflicts.push(Conflict {
                    path: path.clone(),
                    kind,
                    base: b.cloned(),
                    ours: ours.clone(),
                    theirs: t.cloned(),
                    theirs_by: self.touches(&ch.base, onto, &path)?,
                });
            }
            if result.as_ref() != t {
                changes.insert(path, result);
            }
        }
        let head = if changes.is_empty() {
            onto.to_string()
        } else {
            let meta = Self::meta_for(ch, None);
            self.store.put_changeset(&Changeset { parents: vec![onto.to_string()], changes, meta })?
        };
        ch.predecessors.push(std::mem::replace(&mut ch.head, head));
        ch.base = onto.to_string();
        ch.conflicts = conflicts;
        Ok(())
    }

    pub fn rebase(&mut self, id: &str) -> Result<Change> {
        let mut ch = self.open_change_mut(id)?;
        let main = self.main()?;
        self.rebase_onto(&mut ch, &main)?;
        self.store.put_change(&ch)?;
        Ok(ch)
    }

    /// Rebases onto main when needed and moves main to the change's head. Conflicts come back
    /// as typed tasks and leave main untouched.
    pub fn land(&mut self, id: &str) -> Result<Value> {
        let mut ch = self.open_change_mut(id)?;
        if !ch.conflicts.is_empty() {
            return Ok(json!({"status": "conflict", "change": self.change_view(&ch)?}));
        }
        let main = self.main()?;
        let mut merged_from = Value::Null;
        if ch.base != main {
            merged_from = json!({
                "base": self.commit_for(&ch.base)?,
                "ours": self.commit_for(&ch.head)?,
                "theirs": self.commit_for(&main)?,
            });
            self.rebase_onto(&mut ch, &main)?;
            self.store.put_change(&ch)?;
            if !ch.conflicts.is_empty() {
                return Ok(json!({"status": "conflict", "merged_from": merged_from, "change": self.change_view(&ch)?}));
            }
        }
        if ch.head == ch.base {
            bail!("change {id} has nothing to land (empty, or main already has its edits)");
        }
        let commit = self.commit_for(&ch.head)?;
        ch.state = ChangeState::Landed;
        ch.landed_as = Some(ch.head.clone());
        self.store.tx(|s| {
            s.set_ref("main", &ch.head)?;
            s.put_change(&ch)
        })?;
        self.odb.update_ref(&format!("refs/heads/{}", self.branch), &commit)?;
        Ok(
            json!({"status": "landed", "change": ch.id, "changeset": ch.head, "commit": commit, "merged_from": merged_from}),
        )
    }

    /// Opens a change that undoes a main changeset and lands it.
    pub fn revert(&mut self, changeset: &str, agent: &str) -> Result<Value> {
        let target = self.resolve_at(Some(changeset))?;
        let cs = self.changeset(&target)?;
        let parent = cs.parents.first().context("cannot revert the import changeset")?.clone();
        let parent_m = self.manifest(&parent)?;
        let intent = format!("revert {}: {}", &target[..12], cs.meta.intent);
        let mut ch = self.open_change(agent, &intent, None, None, Some(&target))?;
        let entries = cs.changes.keys().map(|p| (p.clone(), parent_m.get(p).cloned())).collect();
        self.set_entries(&mut ch, entries, None)?;
        self.land(&ch.id)
    }

    // ---- git export ----------------------------------------------------------------------

    pub fn commit_for(&mut self, id: &str) -> Result<String> {
        if let Some(c) = self.store.commit_of(id)? {
            return Ok(c);
        }
        let cs = self.changeset(id)?;
        let parents = cs.parents.iter().map(|p| self.commit_for(p)).collect::<Result<Vec<_>>>()?;
        for e in cs.changes.values().flatten() {
            if e.mode != MODE_GITLINK && !self.odb.has(&e.blob) {
                let data = self.store.blob(&e.blob)?.with_context(|| format!("missing blob {}", e.blob))?;
                self.odb.write("blob", &data)?;
            }
        }
        let manifest = self.manifest(id)?;
        let tree = git::write_tree(&mut self.odb, &manifest)?;
        let m = &cs.meta;
        let slug: String =
            m.agent.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' }).collect();
        let name = m.agent.replace(['<', '>', '\n'], "");
        let author = Signature { name, email: format!("{slug}@agents.rootstock"), time: m.time };
        let committer = Signature { name: "Rootstock".into(), email: "rootstock@localhost".into(), time: m.time };
        let bytes = git::commit_bytes(&tree, &parents, &author, &committer, &commit_message(m, id));
        let commit = self.odb.write("commit", &bytes)?;
        self.store.map_commit(id, &commit)?;
        Ok(commit)
    }

    pub fn export_change(&mut self, id: &str) -> Result<Value> {
        let ch = self.change(id)?;
        let commit = self.commit_for(&ch.head)?;
        let name = format!("refs/rootstock/changes/{id}");
        self.odb.update_ref(&name, &commit)?;
        Ok(json!({"change": id, "commit": commit, "ref": name}))
    }

    // ---- views and queries ---------------------------------------------------------------

    fn text_of(&mut self, e: &Option<Entry>) -> Result<Value> {
        let Some(e) = e else { return Ok(Value::Null) };
        if e.mode == MODE_GITLINK {
            return Ok(Value::Null);
        }
        let data = self.blob(&e.blob)?;
        Ok(match as_text(&data) {
            Some(t) if t.len() <= TEXT_LIMIT => Value::String(t),
            _ => Value::Null,
        })
    }

    pub fn change_view(&mut self, ch: &Change) -> Result<Value> {
        let paths: Vec<String> = self.own_changes(ch)?.into_keys().collect();
        let mut conflicts = Vec::new();
        for c in &ch.conflicts {
            conflicts.push(json!({
                "path": c.path,
                "kind": c.kind,
                "base": entry_json(&c.base),
                "ours": entry_json(&c.ours),
                "theirs": entry_json(&c.theirs),
                "text": {"base": self.text_of(&c.base)?, "ours": self.text_of(&c.ours)?, "theirs": self.text_of(&c.theirs)?},
                "ours_intent": ch.intent,
                "theirs_by": c.theirs_by,
                "resolve": "write the final content of this path with apply, or call resolve with take = ours | theirs | base | delete; then land again",
            }));
        }
        Ok(json!({
            "id": ch.id, "state": ch.state, "agent": ch.agent, "model": ch.model, "session": ch.session,
            "intent": ch.intent, "base": ch.base, "head": ch.head, "paths": paths, "conflicts": conflicts,
            "landed_as": ch.landed_as, "created": ch.created,
        }))
    }

    /// Other open changes that touch any of this change's paths.
    pub fn overlaps(&mut self, ch: &Change) -> Result<Vec<Value>> {
        let mine: BTreeSet<String> = self.own_changes(ch)?.into_keys().collect();
        let mut out = Vec::new();
        if mine.is_empty() {
            return Ok(out);
        }
        for other in self.store.changes(Some("open"))? {
            if other.id == ch.id {
                continue;
            }
            let shared: Vec<String> = self.own_changes(&other)?.into_keys().filter(|p| mine.contains(p)).collect();
            if !shared.is_empty() {
                out.push(json!({"change": other.id, "agent": other.agent, "intent": other.intent, "paths": shared}));
            }
        }
        Ok(out)
    }

    pub fn status(&mut self) -> Result<Value> {
        let main = self.main()?;
        let commit = self.commit_for(&main)?;
        let open = self.store.changes(Some("open"))?;
        let files = self.manifest(&main)?.len();
        Ok(json!({
            "main": main, "commit": commit, "branch": self.branch, "files": files,
            "open_changes": open.iter().map(|c| json!({"id": c.id, "agent": c.agent, "intent": c.intent, "conflicts": c.conflicts.len()})).collect::<Vec<_>>(),
        }))
    }

    pub fn read(&mut self, path: &str, at: Option<&str>) -> Result<Value> {
        let cs = self.resolve_at(at)?;
        let e =
            self.manifest(&cs)?.get(path).cloned().with_context(|| format!("no such file {path} at {}", &cs[..12]))?;
        if e.mode == MODE_GITLINK {
            return Ok(json!({"path": path, "at": cs, "mode": "160000", "submodule": e.blob}));
        }
        let data = self.blob(&e.blob)?;
        let mut v =
            json!({"path": path, "at": cs, "blob": e.blob, "mode": format!("{:o}", e.mode), "size": data.len()});
        match as_text(&data) {
            Some(t) => v["content"] = Value::String(t),
            None => v["content_b64"] = Value::String(B64.encode(&data)),
        }
        Ok(v)
    }

    pub fn list(&mut self, dir: &str, at: Option<&str>) -> Result<Value> {
        let cs = self.resolve_at(at)?;
        let m = self.manifest(&cs)?;
        let dir = dir.trim_matches('/');
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let mut entries: BTreeMap<String, Value> = BTreeMap::new();
        for (path, e) in m.range(prefix.clone()..) {
            let Some(rest) = path.strip_prefix(&prefix) else { break };
            match rest.split_once('/') {
                Some((sub, _)) => {
                    entries.entry(sub.to_string()).or_insert_with(|| json!({"name": sub, "kind": "dir"}));
                }
                None => {
                    entries.insert(
                        rest.to_string(),
                        json!({"name": rest, "kind": "file", "mode": format!("{:o}", e.mode), "blob": e.blob}),
                    );
                }
            }
        }
        if entries.is_empty() && !prefix.is_empty() {
            bail!("no such directory {dir}");
        }
        Ok(json!({"at": cs, "path": dir, "entries": entries.into_values().collect::<Vec<_>>()}))
    }

    pub fn grep(&mut self, pattern: &str, at: Option<&str>, dir: Option<&str>, max: usize) -> Result<Value> {
        let re = Regex::new(pattern).map_err(|e| anyhow!("bad pattern: {e}"))?;
        let cs = self.resolve_at(at)?;
        let m = self.manifest(&cs)?;
        let prefix = dir.map(|d| d.trim_matches('/')).filter(|d| !d.is_empty()).map(|d| format!("{d}/"));
        let mut matches = Vec::new();
        let mut truncated = false;
        'files: for (path, e) in m.iter() {
            if prefix.as_ref().is_some_and(|p| !path.starts_with(p)) || e.mode == MODE_GITLINK {
                continue;
            }
            let Some(text) = as_text(&self.blob(&e.blob)?) else { continue };
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    if matches.len() == max {
                        truncated = true;
                        break 'files;
                    }
                    matches.push(json!({"path": path, "line": i + 1, "text": line}));
                }
            }
        }
        Ok(json!({"at": cs, "matches": matches, "truncated": truncated}))
    }

    pub fn diff(&mut self, from: &str, to: &str) -> Result<Value> {
        let (a, b) = (self.manifest(from)?, self.manifest(to)?);
        let mut keys: BTreeSet<&String> = a.keys().collect();
        keys.extend(b.keys());
        let mut files = Vec::new();
        for path in keys {
            let (x, y) = (a.get(path).cloned(), b.get(path).cloned());
            if x == y {
                continue;
            }
            let status = match (&x, &y) {
                (None, _) => "added",
                (_, None) => "deleted",
                _ => "modified",
            };
            let (tx, ty) = (self.text_of(&x)?, self.text_of(&y)?);
            let patch = match (x.is_none() || tx.is_string(), y.is_none() || ty.is_string()) {
                (true, true) => {
                    let p = diffy::create_patch(tx.as_str().unwrap_or(""), ty.as_str().unwrap_or(""));
                    Value::String(p.to_string())
                }
                _ => Value::Null,
            };
            files.push(
                json!({"path": path, "status": status, "from": entry_json(&x), "to": entry_json(&y), "patch": patch}),
            );
        }
        Ok(json!({"from": from, "to": to, "files": files}))
    }

    pub fn log(&mut self, path: Option<&str>, limit: usize) -> Result<Value> {
        let path = path.map(|p| p.trim_matches('/')).filter(|p| !p.is_empty());
        let mut out = Vec::new();
        let mut cur = Some(self.main()?);
        while let Some(id) = cur {
            if out.len() >= limit {
                break;
            }
            let cs = self.changeset(&id)?;
            let touched: Vec<&String> =
                cs.changes.keys().filter(|k| path.is_none_or(|p| *k == p || k.starts_with(&format!("{p}/")))).collect();
            if !touched.is_empty() || (path.is_none() && cs.parents.is_empty()) {
                let m = &cs.meta;
                out.push(json!({
                    "changeset": id, "commit": self.store.commit_of(&id)?, "change": m.change,
                    "agent": m.agent, "model": m.model, "intent": m.intent, "message": m.message, "time": m.time,
                    "paths": touched.iter().take(50).collect::<Vec<_>>(), "path_count": touched.len(),
                }));
            }
            cur = cs.parents.first().cloned();
        }
        Ok(json!({"entries": out}))
    }
}

fn check_no_collision(work: &Manifest, path: &str) -> Result<()> {
    for (i, _) in path.match_indices('/') {
        if work.contains_key(&path[..i]) {
            bail!("cannot create {path}: {} is a file", &path[..i]);
        }
    }
    let dir = format!("{path}/");
    if work.range(dir.clone()..).next().is_some_and(|(k, _)| k.starts_with(&dir)) {
        bail!("cannot create {path}: it is a directory");
    }
    Ok(())
}

fn commit_message(m: &Meta, changeset: &str) -> String {
    let body = m.message.as_deref().unwrap_or(&m.intent).trim();
    let mut s = if body.is_empty() { "(no message)".to_string() } else { body.to_string() };
    s.push_str("\n\n");
    if let Some(c) = &m.change {
        s.push_str(&format!("Rootstock-Change: {c}\n"));
    }
    s.push_str(&format!("Rootstock-Changeset: {changeset}\n"));
    s.push_str(&format!("Rootstock-Agent: {}\n", one_line(&m.agent)));
    if let Some(x) = &m.model {
        s.push_str(&format!("Rootstock-Model: {}\n", one_line(x)));
    }
    if let Some(x) = &m.session {
        s.push_str(&format!("Rootstock-Session: {}\n", one_line(x)));
    }
    if m.message.is_some() {
        s.push_str(&format!("Rootstock-Intent: {}\n", one_line(&m.intent)));
    }
    s
}
