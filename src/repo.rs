//! The repository: changes, server-side rebase and landing, read queries and git export.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rand::Rng;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Config, DriverRule, Drivers, Mode};
use crate::coord::Events;
use crate::git::{self, Odb, Signature};
use crate::merge::{Blobs, Merged, merge3};
use crate::model::*;
use crate::safety::{self, check_path};
use crate::store::Store;
use crate::{drivers, landing};

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
    /// The content is a symlink target.
    pub symlink: Option<bool>,
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
    git_dir: PathBuf,
    /// Coordination events, published after each operation that produced them.
    pub events: Events,
    /// API tokens (own SQLite connection: a token check never waits for the repo lock).
    pub auth: crate::auth::Auth,
}

pub struct Inner {
    pub store: Store,
    pub odb: Odb,
    pub branch: String,
    pub cfg: Config,
    drivers: Drivers,
    mergiraf: Option<String>,
    data: PathBuf,
    manifests: ManifestCache,
    /// Whether `manifest` may read a git tree after `MANIFEST_WALK` steps (tests turn it off
    /// to compare with a full replay).
    pub manifest_from_git: bool,
    pub(crate) heights: HashMap<String, u64>,
    pub(crate) parents: HashMap<String, Option<String>>,
    pub(crate) seen_collisions: HashMap<String, String>,
    /// Events of the running operation; `Repo::with` publishes them when it succeeds.
    pub(crate) outbox: Vec<Value>,
    /// The trigram posting index of main (derived data, see `search`).
    pub(crate) search: crate::search::Index,
    /// (blob, language) pairs with symbol rows (see `symbols`).
    pub(crate) symbol_done: HashSet<(String, String)>,
}

/// What a rebase did besides the line merge: merge drivers used per path, and edits of the
/// change that the result no longer carries (the semantic guard).
#[derive(Debug, Default)]
pub struct RebaseReport {
    pub drivers: Vec<Value>,
    pub absorbed: Vec<Value>,
}

/// A driver decision for one conflicted path.
enum Driven {
    Clean(Entry, &'static str),
    /// Regenerate after the per-path pass, starting from this entry.
    Later(DriverRule, Option<Entry>),
}

/// Most manifests kept in memory. A manifest shares structure with its parent, so the cost of
/// one is the changed paths times the map depth, but an unbounded cache grows with history.
pub const MANIFEST_CACHE: usize = 4096;

/// Changesets a cold `manifest` call replays before it reads a git tree instead. On linux
/// (96 080 files) `git ls-tree -r` takes about 0.09 s, and a replay of 10 000 imported
/// changesets took 0.75 s and 745 MB (`docs/benchmarks.md`, scale run).
pub const MANIFEST_WALK: usize = 64;

/// Manifests by changeset, least recently used out first. A miss rebuilds from the nearest
/// cached ancestor (or the root changeset) by replaying the changesets in between.
#[derive(Default)]
struct ManifestCache {
    map: HashMap<String, (Manifest, u64)>,
    tick: u64,
}

impl ManifestCache {
    fn get(&mut self, id: &str) -> Option<Manifest> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(id).map(|(m, used)| {
            *used = t;
            m.clone()
        })
    }

    fn insert(&mut self, id: String, m: Manifest) {
        self.tick += 1;
        self.map.insert(id, (m, self.tick));
        if self.map.len() > MANIFEST_CACHE {
            // Drop the oldest quarter at once, so eviction costs O(1) per insert on average.
            let mut ticks: Vec<u64> = self.map.values().map(|(_, t)| *t).collect();
            ticks.sort_unstable();
            let cut = ticks[MANIFEST_CACHE / 4];
            self.map.retain(|_, (_, t)| *t > cut);
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

fn paths(data: &Path) -> (PathBuf, PathBuf) {
    (data.join("treeyard.db"), data.join("git"))
}

impl Repo {
    /// Creates a data directory from the tip of `branch` in the git repo at `src`, or from `rev`
    /// (then `branch` is pointed at it). The export repo is a bare clone of `src`, so landed
    /// commits continue its history.
    pub fn init(data: &Path, src: &Path, branch: &str, rev: Option<&str>) -> Result<String> {
        Self::init_with_history(data, src, branch, rev, 0)
    }

    /// Like `init`, and also imports the last `history` first-parent commits as changesets,
    /// each mapped to its own git commit (so blame, log and bisect reach into real history).
    /// Returns the main changeset.
    pub fn init_with_history(
        data: &Path,
        src: &Path,
        branch: &str,
        rev: Option<&str>,
        history: usize,
    ) -> Result<String> {
        let (db, git_dir) = paths(data);
        if db.exists() || git_dir.exists() {
            bail!("{} already holds a treeyard repo", data.display());
        }
        fs::create_dir_all(data)?;
        clone_bare(src, &git_dir)?;
        let rev = rev.map_or_else(|| format!("refs/heads/{branch}"), str::to_string);
        let tip =
            git::run(Some(&git_dir), &["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?.trim().to_string();
        git::run(Some(&git_dir), &["update-ref", &format!("refs/heads/{branch}"), &tip])?;
        git::run(Some(&git_dir), &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])?;
        // The root changeset holds the whole tree of the oldest imported commit.
        let mut commit = tip.clone();
        if history > 0 {
            let n = history.to_string();
            let list = git::run(Some(&git_dir), &["rev-list", "--first-parent", "-n", &n, &tip])?;
            let oldest = list.lines().last().unwrap_or(&tip).to_string();
            commit = git::run(Some(&git_dir), &["rev-parse", "--verify", "--quiet", &format!("{oldest}^1")])
                .map(|s| s.trim().to_string())
                .unwrap_or(oldest);
        }
        let log = if commit == tip { vec![] } else { crate::history::import_log(&git_dir, &commit, &tip)? };
        for (c, cs) in &log {
            for p in cs.changes.keys() {
                check_path(p).with_context(|| format!("importing {c}"))?;
            }
        }
        if commit != tip {
            let tip_files = git::ls_tree(&git_dir, &tip)?;
            safety::check_tree(tip_files.iter().map(|(p, _)| p.as_str()))
                .with_context(|| format!("importing {tip}"))?;
        }
        let time: i64 = git::run(Some(&git_dir), &["show", "-s", "--format=%ct", &commit])?.trim().parse()?;
        let files = git::ls_tree(&git_dir, &commit)?;
        // Hostile repo content: a path that escapes the work tree, names .git, or sits under a
        // file (a symlink there would redirect writes) is refused before anything is stored.
        safety::check_tree(files.iter().map(|(p, _)| p.as_str())).with_context(|| format!("importing {commit}"))?;
        let changes: Changes = files.into_iter().map(|(p, e)| (p, Some(e))).collect();
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
        store
            .tx(|s| {
                let mut id = s.put_changeset(&root)?;
                s.map_commit(&id, &commit)?;
                for (c, mut cs) in log {
                    cs.parents = vec![id];
                    id = s.put_changeset(&cs)?;
                    s.map_commit(&id, &c)?;
                }
                s.set_ref("main", &id)?;
                s.set_ref("branch", branch)?;
                Ok(id)
            })
            .inspect(|_| {
                let _ = fs::write(data.join(crate::config::FILE), crate::config::TEMPLATE);
            })
    }

    pub fn open(data: &Path) -> Result<Self> {
        let (db, git_dir) = paths(data);
        if !db.exists() {
            bail!("{} is not a treeyard repo (run `treeyard init` first)", data.display());
        }
        let store = Store::open(&db)?;
        let branch = store.get_ref("branch")?.context("no branch ref")?;
        let cfg = Config::load(data)?;
        let (drivers, mergiraf) = (cfg.drivers()?, cfg.mergiraf()?);
        let mut inner = Inner {
            store,
            odb: Odb::open(&git_dir)?,
            branch,
            cfg,
            drivers,
            mergiraf,
            data: data.to_path_buf(),
            manifests: ManifestCache::default(),
            manifest_from_git: true,
            heights: HashMap::new(),
            parents: HashMap::new(),
            seen_collisions: HashMap::new(),
            outbox: Vec::new(),
            search: Default::default(),
            symbol_done: HashSet::new(),
        };
        // Heals a crash between the metadata commit and the git ref write.
        let main = inner.main()?;
        let commit = inner.commit_for(&main)?;
        inner.odb.update_ref(&format!("refs/heads/{}", inner.branch), &commit)?;
        if let Some(notes) = inner.store.get_ref("notes")? {
            inner.odb.update_ref(crate::attach::NOTES_REF, &notes)?;
        }
        let auth = crate::auth::Auth::open(&db)?;
        Ok(Self { inner: Mutex::new(inner), git_dir, events: Events::default(), auth })
    }

    /// The export repo (bare git dir). Read it without the lock; write only through `Inner`.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` under the repo lock, then publishes the events it produced (none if it failed).
    pub fn with<R>(&self, f: impl FnOnce(&mut Inner) -> Result<R>) -> Result<R> {
        let (out, events) = {
            let mut g = self.lock();
            let out = f(&mut g);
            (out, std::mem::take(&mut g.outbox))
        };
        if out.is_ok() {
            self.events.publish(events);
        }
        out
    }
}

impl Blobs for Inner {
    fn read_blob(&mut self, id: &str) -> Result<Vec<u8>> {
        self.blob(id)
    }
    fn write_blob(&mut self, data: &[u8]) -> Result<String> {
        self.put_blob(data)
    }
    fn merge_text(&mut self, base: &str, ours: &str, theirs: &str) -> Result<std::result::Result<String, String>> {
        crate::merge::git_merge_file(&self.scratch(), base, ours, theirs)
    }
}

pub(crate) fn as_text(data: &[u8]) -> Option<String> {
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
                break m;
            }
            // A long walk stops at the first changeset with a git commit: its tree is the
            // persisted form of its manifest (an export or an imported commit, `map_commit`).
            if self.manifest_from_git
                && chain.len() >= MANIFEST_WALK
                && let Some(commit) = self.store.commit_of(&cur)?
            {
                let m: Manifest = git::ls_tree(self.odb.dir(), &commit)?.into_iter().collect();
                self.manifests.insert(cur.clone(), m.clone());
                break m;
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

    /// The commit message on the change's current head, kept across amend, resolve and rebase.
    fn head_message(&self, ch: &Change) -> Result<Option<String>> {
        if ch.head == ch.base { Ok(None) } else { Ok(self.changeset(&ch.head)?.meta.message) }
    }

    pub(crate) fn own_changes(&self, ch: &Change) -> Result<Changes> {
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
        self.open_change_with(agent, intent, model, session, base, Intent::default())
    }

    /// Opens a change with declared paths (globs) and symbols; see `intent_overlaps`.
    pub fn open_change_with(
        &mut self,
        agent: &str,
        intent: &str,
        model: Option<String>,
        session: Option<String>,
        base: Option<&str>,
        declared: Intent,
    ) -> Result<Change> {
        crate::config::globs(&declared.paths)?;
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
            declared,
            view: None,
            group: None,
        };
        self.store.put_change(&ch)?;
        self.emit(
            "opened",
            &[&ch.id],
            &[&ch.agent],
            json!({"change": ch.id, "intent": ch.intent, "declared": ch.declared}),
        );
        Ok(ch)
    }

    /// Replaces the declared paths and symbols of an open change.
    pub fn declare(&mut self, id: &str, declared: Intent) -> Result<Change> {
        crate::config::globs(&declared.paths)?;
        let mut ch = self.open_change_mut(id)?;
        ch.declared = declared;
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
            let meta = Self::meta_for(ch, message.or(self.head_message(ch)?));
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
            bail!("{} is a submodule; treeyard does not edit submodules", e.path);
        }
        if e.delete == Some(true) {
            if cur.is_none() {
                bail!("cannot delete {}: no such file", e.path);
            }
            return Ok(None);
        }
        let mode = match (e.symlink, e.executable) {
            (Some(true), _) => MODE_LINK,
            (_, Some(true)) => MODE_EXEC,
            (_, Some(false)) => MODE_FILE,
            _ => cur.map_or(MODE_FILE, |c| c.mode),
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
        if data.len() > safety::MAX_FILE {
            bail!(
                "{} is {} bytes; the API takes files up to {} bytes (push big files with git)",
                e.path,
                data.len(),
                safety::MAX_FILE
            );
        }
        Ok(Some(Entry { blob: self.put_blob(&data)?, mode }))
    }

    pub fn apply(&mut self, id: &str, edits: &[Edit], message: Option<String>) -> Result<Change> {
        self.apply_seen(id, edits, message, None)
    }

    /// `apply` where whole-file writes are relative to `seen` (a revision the client read), or,
    /// when not given, to the head before the server last rebased the change on its own. Paths
    /// that moved since then get a 3-way merge instead of an overwrite, so continuous rebase never
    /// loses main's edits. `seen` equal to the current head clears that memory.
    pub fn apply_seen(
        &mut self,
        id: &str,
        edits: &[Edit],
        message: Option<String>,
        seen: Option<&str>,
    ) -> Result<Change> {
        if edits.len() > safety::MAX_EDITS {
            bail!("apply takes at most {} edits per call, not {}", safety::MAX_EDITS, edits.len());
        }
        let mut ch = self.open_change_mut(id)?;
        let mut work = self.manifest(&ch.head)?;
        let explicit = seen.is_some();
        let seen = match seen {
            Some(s) => Some(self.resolve_at(Some(s))?),
            None => ch.view.clone(),
        };
        let up_to_date = seen.as_deref() == Some(ch.head.as_str());
        // Main changesets the stale writer did not see start after the base its view sat on.
        let since = match &seen {
            Some(v) => {
                let cs = self.changeset(v)?;
                if cs.meta.change.as_deref() == Some(id) {
                    cs.parents.first().cloned().unwrap_or_default()
                } else {
                    v.clone()
                }
            }
            None => String::new(),
        };
        let view = match &seen {
            Some(v) if *v != ch.head => Some(self.manifest(v)?),
            _ => None,
        };
        // With kept conflicts, a write to a conflicted path is its resolution: no merge with the
        // stale view (that would nest conflict markers).
        let kept: BTreeSet<String> = if self.cfg.coordination.keep_conflicted {
            ch.conflicts.iter().map(|c| c.path.clone()).collect()
        } else {
            BTreeSet::new()
        };
        let mut late_conflicts = Vec::new();
        let mut out = Vec::new();
        for e in edits {
            check_path(&e.path)?;
            let cur = work.get(&e.path).cloned();
            let mut new = self.edit_entry(e, cur.as_ref())?;
            if let Some(v) = &view
                && e.replace.is_none()
                && !kept.contains(&e.path)
                && v.get(&e.path) != cur.as_ref()
            {
                let old = v.get(&e.path).cloned();
                match merge3(self, old.as_ref(), new.as_ref(), cur.as_ref())? {
                    Merged::Clean(m) => new = m,
                    Merged::Conflict { kind, materialized } => {
                        late_conflicts.push(Conflict {
                            path: e.path.clone(),
                            kind,
                            base: old,
                            ours: new.clone(),
                            theirs: cur.clone(),
                            theirs_by: self.touches(&since, &ch.base, &e.path)?,
                        });
                        new = materialized;
                    }
                }
            }
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
        if explicit && up_to_date {
            ch.view = None;
        }
        if !late_conflicts.is_empty() {
            let paths: Vec<String> = late_conflicts.iter().map(|c| c.path.clone()).collect();
            ch.conflicts.extend(late_conflicts);
            self.emit(
                "conflict",
                &[&ch.id],
                &[&ch.agent],
                json!({"change": ch.id, "paths": paths, "cause": "stale_write"}),
            );
        }
        self.store.put_change(&ch)?;
        Ok(ch)
    }

    /// Sets paths of an open change to entries whose blobs already exist (used by replay).
    pub fn apply_entries(
        &mut self,
        id: &str,
        entries: Vec<(String, Option<Entry>)>,
        message: Option<String>,
    ) -> Result<Change> {
        let mut ch = self.open_change_mut(id)?;
        self.set_entries(&mut ch, entries, message)?;
        Ok(ch)
    }

    /// Records an existing git commit as a changeset on top of its first parent's changeset
    /// (which must be known), mapped to that same commit. Not on main.
    pub fn import_commit(&mut self, commit: &str) -> Result<String> {
        if let Some(cs) = self.store.changeset_of_commit(commit)? {
            return Ok(cs);
        }
        let gd = self.odb.dir().to_path_buf();
        let parent = git::run(Some(&gd), &["rev-parse", &format!("{commit}^1")])?.trim().to_string();
        let parent_cs = self
            .store
            .changeset_of_commit(&parent)?
            .with_context(|| format!("parent {parent} of {commit} is not imported"))?;
        let info = git::run(Some(&gd), &["show", "-s", "--format=%ct%n%an%n%s", commit])?;
        let mut lines = info.lines();
        let time: i64 = lines.next().unwrap_or("0").parse()?;
        let author = lines.next().unwrap_or("").to_string();
        let subject = lines.next().unwrap_or("").to_string();
        let changes: Changes = git::diff_tree(&gd, &parent, commit)?.into_iter().collect();
        for p in changes.keys() {
            check_path(p).with_context(|| format!("importing {commit}"))?;
        }
        let cs = Changeset {
            parents: vec![parent_cs],
            changes,
            meta: Meta { agent: format!("import:{author}"), intent: subject, time, ..Meta::default() },
        };
        let id = self.store.put_changeset(&cs)?;
        self.store.map_commit(&id, commit)?;
        Ok(id)
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
        self.store.dequeue(id)?;
        self.forget(id);
        self.emit("abandoned", &[id], &[&ch.agent], json!({"change": id}));
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

    /// A merge driver for a path whose line merge conflicted, from `[merge]` in treeyard.toml.
    fn drive(&mut self, path: &str, b: Option<&Entry>, o: &Entry, t: &Entry) -> Result<Option<Driven>> {
        let text = |s: &mut Self, e: Option<&Entry>| -> Result<Option<String>> {
            match e {
                None => Ok(Some(String::new())),
                Some(e) => Ok(as_text(&s.blob(&e.blob)?)),
            }
        };
        let (Some(bt), Some(ot), Some(tt)) = (text(self, b)?, text(self, Some(o))?, text(self, Some(t))?) else {
            return Ok(None);
        };
        let mode = o.mode;
        if let Some(rule) = self.drivers.rule(path).cloned() {
            let merged = match rule.driver.as_str() {
                "union" => Some((drivers::union(&bt, &ot, &tt), "union")),
                "cargo-lock" => drivers::cargo_lock(&bt, &ot, &tt).map(|m| (m, "cargo-lock")),
                "ours" => Some((ot.clone(), "ours")),
                "theirs" => Some((tt.clone(), "theirs")),
                _ => None,
            };
            match (merged, &rule.command) {
                (Some((m, _)), Some(_)) => {
                    let start = Entry { blob: self.put_blob(m.as_bytes())?, mode };
                    return Ok(Some(Driven::Later(rule, Some(start))));
                }
                (Some((m, name)), None) => {
                    return Ok(Some(Driven::Clean(Entry { blob: self.put_blob(m.as_bytes())?, mode }, name)));
                }
                (None, Some(_)) if rule.driver == "regenerate" => return Ok(Some(Driven::Later(rule, None))),
                _ => {}
            }
        }
        if let Some(bin) = self.mergiraf.clone()
            && let Some(m) = drivers::mergiraf(&bin, path, &bt, &ot, &tt)?
        {
            return Ok(Some(Driven::Clean(Entry { blob: self.put_blob(m.as_bytes())?, mode }, "mergiraf")));
        }
        Ok(None)
    }

    /// Scratch space for test runs and regeneration (inside the data directory).
    pub fn scratch(&self) -> PathBuf {
        self.data.join("tmp")
    }

    /// Runs a regenerate command in a temp copy of `tree` and reads `path` back.
    fn regenerate(&mut self, tree: &Manifest, path: &str, rule: &DriverRule) -> Result<Vec<u8>> {
        let cmd = rule.command.as_deref().context("regenerate needs a command")?;
        let dir = landing::scratch_dir(&self.scratch(), "regen")?;
        let res = (|| {
            landing::materialize(self, tree, &dir)?;
            let timeout = std::time::Duration::from_secs(rule.timeout_s.unwrap_or(300));
            let out = landing::run_cmd(cmd, &dir, timeout, &[("TREEYARD_PATH", path)])?;
            if !out.ok {
                bail!("{cmd:?} failed: {}", out.output.trim());
            }
            fs::read(dir.join(path)).with_context(|| format!("{cmd:?} did not leave {path}"))
        })();
        let _ = fs::remove_dir_all(&dir);
        res
    }

    pub(crate) fn rebase_onto(&mut self, ch: &mut Change, onto: &str) -> Result<RebaseReport> {
        let mut report = RebaseReport::default();
        if ch.base == onto {
            return Ok(report);
        }
        if !ch.conflicts.is_empty() && !self.cfg.coordination.keep_conflicted {
            bail!("change {} has unresolved conflicts; resolve them before rebasing", ch.id);
        }
        // A kept conflict merges again from its own base and our side, never from the text
        // with markers, so main's later edits to the path join the same conflict.
        let kept: HashMap<String, Conflict> = ch.conflicts.drain(..).map(|c| (c.path.clone(), c)).collect();
        let base_m = self.manifest(&ch.base)?;
        let theirs_m = self.manifest(onto)?;
        let mut changes = Changes::new();
        let mut conflicts = Vec::new();
        let mut later = Vec::new();
        // Drivers resolve conflicts silently, so they run only on a true merge base. When the
        // base is not an ancestor of `onto` (a base holding work main lacks, as in replay),
        // main seems to delete that work and a driver would apply the deletion.
        let drivers_ok = self.is_ancestor(&ch.base, onto)?;
        let mut own = self.own_changes(ch)?;
        for (p, c) in &kept {
            own.entry(p.clone()).or_insert_with(|| c.ours.clone());
        }
        for (path, mut ours) in own {
            let (mut b, t) = (base_m.get(&path), theirs_m.get(&path));
            let old = kept.get(&path);
            if let Some(c) = old {
                (b, ours) = (c.base.as_ref(), c.ours.clone());
            }
            let mut merged = merge3(self, b, ours.as_ref(), t)?;
            if let (
                true,
                Merged::Conflict { kind: ConflictKind::Content | ConflictKind::AddAdd, .. },
                Some(o),
                Some(te),
            ) = (drivers_ok, &merged, ours.as_ref(), t)
            {
                match self.drive(&path, b, o, te)? {
                    Some(Driven::Clean(e, name)) => {
                        report.drivers.push(json!({"path": path, "driver": name}));
                        merged = Merged::Clean(Some(e));
                    }
                    Some(Driven::Later(rule, start)) => later.push((path.clone(), rule, start)),
                    None => {}
                }
            }
            let (result, kind) = match merged {
                Merged::Clean(r) => (r, None),
                Merged::Conflict { kind, materialized } => (materialized, Some(kind)),
            };
            if let Some(kind) = kind {
                let mut theirs_by = self.touches(&ch.base, onto, &path)?;
                theirs_by.extend(old.map(|c| c.theirs_by.clone()).unwrap_or_default());
                conflicts.push(Conflict {
                    path: path.clone(),
                    kind,
                    base: b.cloned(),
                    ours: ours.clone(),
                    theirs: t.cloned(),
                    theirs_by,
                });
            } else if let Some((whole, hunks)) = self.absorbed(b, ours.as_ref(), t, result.as_ref())? {
                report.absorbed.push(json!({
                    "path": path, "whole_file": whole, "hunks": hunks,
                    "theirs_by": self.touches(&ch.base, onto, &path)?,
                }));
            }
            if result.as_ref() != t {
                changes.insert(path, result);
            }
        }
        if !later.is_empty() {
            // Regenerate from the merged tree, with each regenerated path starting from the
            // driver's merge or from main's version.
            let mut tree = theirs_m.clone();
            for (p, e) in &changes {
                match e {
                    Some(e) => tree.insert(p.clone(), e.clone()),
                    None => tree.remove(p),
                };
            }
            for (p, _, start) in &later {
                if let Some(e) = start.clone().or_else(|| theirs_m.get(p).cloned()) {
                    tree.insert(p.clone(), e);
                }
            }
            for (path, rule, _) in later {
                match self.regenerate(&tree, &path, &rule) {
                    Ok(data) => {
                        let mode = tree.get(&path).map_or(MODE_FILE, |e| e.mode);
                        let e = Entry { blob: self.put_blob(&data)?, mode };
                        conflicts.retain(|c| c.path != path);
                        if theirs_m.get(&path) == Some(&e) {
                            changes.remove(&path);
                        } else {
                            changes.insert(path.clone(), Some(e));
                        }
                        report.drivers.push(json!({"path": path, "driver": "regenerate"}));
                    }
                    Err(err) => {
                        report.drivers.push(json!({"path": path, "driver": "regenerate", "error": format!("{err:#}")}))
                    }
                }
            }
        }
        let head = if changes.is_empty() {
            onto.to_string()
        } else {
            let meta = Self::meta_for(ch, self.head_message(ch)?);
            self.store.put_changeset(&Changeset { parents: vec![onto.to_string()], changes, meta })?
        };
        // Predecessors are earlier heads with edits; an empty change only moves its base.
        let old = std::mem::replace(&mut ch.head, head);
        if old != ch.base {
            ch.predecessors.push(old);
        }
        ch.base = onto.to_string();
        ch.conflicts = conflicts;
        Ok(report)
    }

    /// Turns the guard findings of a rebase into review tasks; returns their ids.
    pub(crate) fn guard(&mut self, ch: &Change, report: &RebaseReport) -> Result<Vec<i64>> {
        let mut ids = Vec::new();
        for a in &report.absorbed {
            ids.push(self.add_review(ch, "absorbed_edit", a.clone())?);
        }
        Ok(ids)
    }

    pub fn rebase(&mut self, id: &str) -> Result<Change> {
        self.rebase_to(id, None)
    }

    /// Rebases onto main, or with `onto` = another open change, stacks on that change's head: its
    /// edits show up in this change now, and `land` waits until it has landed. Two changes cannot
    /// stack on each other.
    pub fn rebase_to(&mut self, id: &str, onto: Option<&str>) -> Result<Change> {
        let mut ch = self.open_change_mut(id)?;
        let main = match onto {
            None => self.main()?,
            Some(other) => {
                let o = self.change(other)?;
                if o.id == ch.id {
                    bail!("a change cannot stack on itself");
                }
                if o.state != ChangeState::Open {
                    bail!("change {other} is {}; rebase onto main instead", o.state.as_str());
                }
                if self.changeset(&o.base)?.meta.change.as_deref() == Some(ch.id.as_str()) {
                    bail!("change {other} is stacked on {id}; two changes cannot stack on each other");
                }
                o.head
            }
        };
        let report = self.rebase_onto(&mut ch, &main)?;
        self.store.put_change(&ch)?;
        self.guard(&ch, &report)?;
        Ok(ch)
    }

    /// Lands by the repo's landing mode: `direct` and `optimistic` move main now, `queue` enqueues.
    pub fn land(&mut self, id: &str) -> Result<Value> {
        // Landing twice answers like the first time, so a client may retry a land whose
        // reply it lost (network error, server restart).
        let ch = self.change(id)?;
        if ch.state == ChangeState::Landed {
            let cs = ch.landed_as.clone().context("landed change without landed_as")?;
            let commit = self.commit_for(&cs)?;
            let status = if ch.head == ch.base { "already_on_main" } else { "landed" };
            return Ok(json!({"status": status, "change": id, "changeset": cs, "commit": commit, "repeated": true}));
        }
        match self.cfg.landing.mode {
            Mode::Queue => self.enqueue(id),
            _ => self.land_direct(id),
        }
    }

    /// Rebases onto main when needed and moves main to the change's head. Conflicts come back
    /// as typed tasks and leave main untouched.
    pub fn land_direct(&mut self, id: &str) -> Result<Value> {
        let mut ch = self.open_change_mut(id)?;
        if !ch.conflicts.is_empty() {
            return Ok(json!({"status": "conflict", "change": self.change_view(&ch)?}));
        }
        if ch.head == ch.base && ch.predecessors.is_empty() {
            bail!("change {id} is empty: apply edits before landing");
        }
        // A change stacked on another change's head waits until that change lands; landing it
        // first would apply its diff to a main that lacks the work it was written against.
        if let Some(dep) = self.changeset(&ch.base)?.meta.change.filter(|d| *d != ch.id) {
            match self.change(&dep)?.state {
                ChangeState::Open => return Ok(json!({"status": "waiting", "change": ch.id, "waiting_for": dep})),
                ChangeState::Abandoned => {
                    bail!("change {id} is stacked on abandoned change {dep}; rebase it onto main")
                }
                ChangeState::Landed => {}
            }
        }
        let main = self.main()?;
        let mut merged_from = Value::Null;
        let mut report = RebaseReport::default();
        if ch.base != main {
            merged_from = json!({
                "base": self.commit_for(&ch.base)?,
                "ours": self.commit_for(&ch.head)?,
                "theirs": self.commit_for(&main)?,
            });
            report = self.rebase_onto(&mut ch, &main)?;
            self.store.put_change(&ch)?;
            if !ch.conflicts.is_empty() {
                let paths: Vec<&String> = ch.conflicts.iter().map(|c| &c.path).collect();
                self.emit("conflict", &[id], &[&ch.agent], json!({"change": id, "paths": paths, "cause": "land"}));
                return Ok(
                    json!({"status": "conflict", "merged_from": merged_from, "drivers": report.drivers, "change": self.change_view(&ch)?}),
                );
            }
        }
        let mut reviews = self.guard(&ch, &report)?;
        if ch.head == ch.base {
            // The rebase found every edit already on main: nothing new to commit.
            ch.state = ChangeState::Landed;
            ch.landed_as = Some(main.clone());
            self.store.put_change(&ch)?;
            reviews.push(self.add_review(&ch, "already_on_main", json!({"absorbed": report.absorbed}))?);
            self.forget(id);
            self.emit(
                "landed",
                &[id],
                &[&ch.agent],
                json!({"change": id, "status": "already_on_main", "changeset": main}),
            );
            let commit = self.commit_for(&main)?;
            return Ok(json!({
                "status": "already_on_main", "change": ch.id, "changeset": main, "commit": commit,
                "merged_from": merged_from, "drivers": report.drivers, "reviews": reviews,
            }));
        }
        crate::failpoint::hit("land:before-objects");
        let commit = self.commit_for(&ch.head)?;
        crate::failpoint::hit("land:after-objects");
        ch.state = ChangeState::Landed;
        ch.landed_as = Some(ch.head.clone());
        // Crash safety: git objects first, then one SQLite transaction (the point of truth),
        // then the git ref. `Repo::open` rewrites the ref from SQLite, so a crash between the
        // last two steps heals on restart; a crash before the transaction leaves the change open.
        self.store.tx(|s| {
            s.set_ref("main", &ch.head)?;
            crate::failpoint::hit("land:in-tx");
            s.put_change(&ch)
        })?;
        crate::failpoint::hit("land:after-db");
        self.odb.update_ref(&format!("refs/heads/{}", self.branch), &commit)?;
        crate::failpoint::hit("land:after-ref");
        let notes = self.notes_after_land(id, &ch.head, &commit);
        self.forget(id);
        self.emit(
            "landed",
            &[id],
            &[&ch.agent],
            json!({"change": id, "status": "landed", "changeset": ch.head, "commit": commit}),
        );
        let rebased = self.continuous_rebase()?;
        Ok(json!({
            "status": "landed", "change": ch.id, "changeset": ch.head, "commit": commit, "merged_from": merged_from,
            "drivers": report.drivers, "reviews": reviews, "rebased": rebased, "notes": notes,
        }))
    }

    /// Continuous rebase (`[coordination] continuous_rebase`): after main moved, rebase every
    /// open change that is not stacked, queued or conflicted (unless `keep_conflicted`) onto it,
    /// so conflicts show up while they are small. The owner's later whole-file writes merge
    /// through `view`.
    pub fn continuous_rebase(&mut self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        if !self.cfg.coordination.continuous_rebase {
            return Ok(out);
        }
        let main = self.main()?;
        let queued: BTreeSet<String> = self.store.queue()?.into_iter().collect();
        for mut o in self.store.changes(Some("open"))? {
            let conflicted = !o.conflicts.is_empty() && !self.cfg.coordination.keep_conflicted;
            if o.base == main || conflicted || queued.contains(&o.id) {
                continue;
            }
            // A base that is not an ancestor of main holds work main lacks (replay's stale
            // parents): a rebase now would record that work as deleted. It rebases at land.
            if !self.is_ancestor(&o.base, &main)? {
                continue;
            }
            if let Some(dep) = self.changeset(&o.base)?.meta.change.filter(|d| *d != o.id)
                && self.change(&dep)?.state == ChangeState::Open
            {
                continue;
            }
            let (old_base, old_head) = (o.base.clone(), o.head.clone());
            let report = self.rebase_onto(&mut o, &main)?;
            o.view.get_or_insert(old_head.clone());
            self.store.put_change(&o)?;
            let reviews = self.guard(&o, &report)?;
            let paths: Vec<String> = o.conflicts.iter().map(|c| c.path.clone()).collect();
            self.emit(
                "rebased",
                &[&o.id],
                &[&o.agent],
                json!({"change": o.id, "onto": main, "conflicts": paths, "drivers": report.drivers}),
            );
            if !paths.is_empty() {
                self.emit(
                    "conflict",
                    &[&o.id],
                    &[&o.agent],
                    json!({"change": o.id, "paths": paths, "cause": "continuous_rebase"}),
                );
            }
            out.push(json!({
                "change": o.id, "base": old_base, "ours": old_head, "theirs": main, "head": o.head,
                "conflicts": paths, "drivers": report.drivers, "reviews": reviews,
            }));
        }
        Ok(out)
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
        // A revert never waits in the queue: it is the way back to a green main.
        self.land_direct(&ch.id)
    }

    // ---- checkpoints inside an open change -----------------------------------------------

    /// Marks the head of an open change as the next checkpoint. A head that is already the
    /// newest checkpoint is not marked twice (a hook may call this every turn).
    pub fn checkpoint(&mut self, id: &str, label: Option<String>) -> Result<Value> {
        let ch = self.open_change_mut(id)?;
        let label = label.map(|l| one_line(&l)).filter(|l| !l.is_empty());
        if let Some(last) = self.store.checkpoints(id)?.pop()
            && last.changeset == ch.head
        {
            return Ok(json!({"change": id, "checkpoint": last, "repeated": true}));
        }
        let cp = Checkpoint {
            id: format!("cp_{:012x}", rand::thread_rng().r#gen::<u64>() & 0xffff_ffff_ffff),
            change: id.to_string(),
            n: 0,
            changeset: ch.head.clone(),
            label,
            time: now(),
        };
        let cp = self.store.add_checkpoint(&cp)?;
        Ok(json!({"change": id, "checkpoint": cp, "repeated": false}))
    }

    /// The checkpoints of a change, oldest first, each with the paths that differ from the one
    /// before (the first one: from the base it was made on).
    pub fn checkpoints(&mut self, id: &str) -> Result<Value> {
        let ch = self.change(id)?;
        let mut out = Vec::new();
        let mut prev: Option<String> = None;
        for cp in self.store.checkpoints(id)? {
            let from = match &prev {
                Some(p) => p.clone(),
                None => self.edit_base(id, &cp.changeset)?,
            };
            let paths = self.changed_paths(id, &from, &cp.changeset)?;
            out.push(json!({
                "id": cp.id, "n": cp.n, "changeset": cp.changeset, "label": cp.label, "time": cp.time,
                "paths": paths, "current": cp.changeset == ch.head,
            }));
            prev = Some(cp.changeset);
        }
        Ok(json!({"change": id, "head": ch.head, "checkpoints": out}))
    }

    /// The base a changeset of change `id` was made on (its parent), or the changeset itself
    /// when it is a base (the change was empty then).
    fn edit_base(&self, id: &str, cs: &str) -> Result<String> {
        let c = self.changeset(cs)?;
        match (c.meta.change.as_deref() == Some(id), c.parents.first()) {
            (true, Some(p)) => Ok(p.clone()),
            _ => Ok(cs.to_string()),
        }
    }

    /// The change's own edits in changeset `cs` (empty when `cs` is a base).
    fn edits_of(&self, id: &str, cs: &str) -> Result<Changes> {
        let c = self.changeset(cs)?;
        Ok(if c.meta.change.as_deref() == Some(id) && !c.parents.is_empty() { c.changes } else { Changes::new() })
    }

    /// Paths edited by the change in `a` or `b` whose entries differ between the two trees.
    fn changed_paths(&mut self, id: &str, a: &str, b: &str) -> Result<Vec<String>> {
        let mut keys: BTreeSet<String> = self.edits_of(id, a)?.into_keys().collect();
        keys.extend(self.edits_of(id, b)?.into_keys());
        let (ma, mb) = (self.manifest(a)?, self.manifest(b)?);
        Ok(keys.into_iter().filter(|p| ma.get(p) != mb.get(p)).collect())
    }

    /// `to` of `restore_change`: a checkpoint id `cp_...`, a checkpoint number (0 = the base,
    /// an empty change), or a changeset of this change (a former head) or its base.
    fn restore_target(&self, ch: &Change, to: &str) -> Result<(String, Option<i64>)> {
        let to = to.trim();
        let cps = self.store.checkpoints(&ch.id)?;
        if to.starts_with("cp_") {
            let cp =
                cps.iter().find(|c| c.id == to).with_context(|| format!("change {} has no checkpoint {to}", ch.id))?;
            return Ok((cp.changeset.clone(), Some(cp.n)));
        }
        if !to.is_empty() && to.len() < 10 && to.bytes().all(|b| b.is_ascii_digit()) {
            let n: i64 = to.parse()?;
            if n == 0 {
                return Ok((ch.base.clone(), Some(0)));
            }
            let cp = cps
                .iter()
                .find(|c| c.n == n)
                .with_context(|| format!("change {} has no checkpoint {n} (it has {})", ch.id, cps.len()))?;
            return Ok((cp.changeset.clone(), Some(n)));
        }
        let cs = self.resolve_at(Some(to))?;
        let ours = self.changeset(&cs)?.meta.change.as_deref() == Some(ch.id.as_str());
        if !(ours || cs == ch.base || cps.iter().any(|c| c.changeset == cs)) {
            bail!("{} is not a changeset of change {}", &cs[..12], ch.id);
        }
        let n = cps.iter().find(|c| c.changeset == cs).map(|c| c.n);
        Ok((cs, n))
    }

    /// Makes a new head whose tree equals an earlier state of the change (`restore_target`).
    /// If the change was rebased since, that state's own edits are merged onto the current base
    /// instead, so main's later work stays. The head before the restore becomes a checkpoint,
    /// so a restore is undone by restoring to it.
    pub fn restore_change(&mut self, id: &str, to: &str) -> Result<Value> {
        let mut ch = self.open_change_mut(id)?;
        let (target, n) = self.restore_target(&ch, to)?;
        let before = self.checkpoint(id, Some(format!("before restore to {to}")))?;
        let previous = ch.head.clone();
        let from = self.edit_base(id, &target)?;
        let mut paths: BTreeSet<String> = self.edits_of(id, &target)?.into_keys().collect();
        paths.extend(self.own_changes(&ch)?.into_keys());
        let (pm, tm, bm) = (self.manifest(&from)?, self.manifest(&target)?, self.manifest(&ch.base)?);
        let (mut entries, mut conflicts) = (Vec::new(), Vec::new());
        for path in paths {
            let (b, o, t) = (pm.get(&path), tm.get(&path), bm.get(&path));
            match merge3(self, b, o, t)? {
                Merged::Clean(e) => entries.push((path, e)),
                Merged::Conflict { kind, materialized } => {
                    conflicts.push(Conflict {
                        path: path.clone(),
                        kind,
                        base: b.cloned(),
                        ours: o.cloned(),
                        theirs: t.cloned(),
                        theirs_by: self.touches(&from, &ch.base, &path)?,
                    });
                    entries.push((path, materialized));
                }
            }
        }
        self.set_entries(&mut ch, entries, None)?;
        if !conflicts.is_empty() {
            let paths: Vec<String> = conflicts.iter().map(|c| c.path.clone()).collect();
            ch.conflicts.extend(conflicts);
            self.store.put_change(&ch)?;
            self.emit("conflict", &[id], &[&ch.agent], json!({"change": id, "paths": paths, "cause": "restore"}));
        }
        Ok(json!({
            "change": self.change_view(&ch)?, "restored_to": target, "checkpoint": n, "previous": previous,
            "undo": before["checkpoint"]["n"], "rebased_since": from != ch.base && target != ch.base,
        }))
    }

    // ---- best-of-N groups --------------------------------------------------------------

    /// Puts an open change in a best-of-N group.
    pub fn set_group(&mut self, id: &str, group: &str) -> Result<Change> {
        let group = check_group(group)?;
        let mut ch = self.open_change_mut(id)?;
        ch.group = Some(group.to_string());
        self.store.put_change(&ch)?;
        Ok(ch)
    }

    /// The members of a group with state and diff size (files, lines added and removed).
    pub fn group_view(&mut self, group: &str) -> Result<Value> {
        let members = self.store.group(group)?;
        if members.is_empty() {
            bail!("no group {group:?}");
        }
        let mut out = Vec::new();
        for ch in &members {
            let d = self.diff(&ch.base, &ch.head)?;
            let files = d["files"].as_array().map_or(0, Vec::len);
            let (mut added, mut removed) = (0, 0);
            for f in d["files"].as_array().into_iter().flatten() {
                for l in f["patch"].as_str().unwrap_or("").lines() {
                    if l.starts_with('+') && !l.starts_with("+++") {
                        added += 1;
                    } else if l.starts_with('-') && !l.starts_with("---") {
                        removed += 1;
                    }
                }
            }
            out.push(json!({
                "change": ch.id, "agent": ch.agent, "model": ch.model, "intent": ch.intent, "state": ch.state,
                "head": ch.head, "conflicts": ch.conflicts.len(), "landed_as": ch.landed_as,
                "diff": {"files": files, "added": added, "removed": removed},
            }));
        }
        Ok(json!({"group": group, "members": out}))
    }

    /// Lands `id` and, once it is on main, abandons the other open changes of its group. In
    /// queue mode it answers `queued` and abandons nothing; call it again after the land.
    pub fn land_one_of(&mut self, group: &str, id: &str) -> Result<Value> {
        let ch = self.change(id)?;
        if ch.group.as_deref() != Some(group) {
            bail!("change {id} is not in group {group:?}");
        }
        let land = self.land(id)?;
        let mut abandoned = Vec::new();
        if matches!(land["status"].as_str(), Some("landed" | "already_on_main")) {
            for other in self.store.group(group)? {
                if other.id != id && other.state == ChangeState::Open {
                    self.abandon(&other.id)?;
                    abandoned.push(other.id);
                }
            }
        }
        Ok(json!({"status": land["status"], "group": group, "change": id, "land": land, "abandoned": abandoned}))
    }

    // ---- git push from people -------------------------------------------------------------

    /// Lands commits a person pushed to the branch. `new` must contain main's exported commit
    /// (fast-forward); each commit on the path from it to `new` becomes a change that goes
    /// through `land`, so agents see the push at once. The pushed commits stay byte-identical.
    /// An error is a rejection: its text goes back to the person.
    pub fn land_push(&mut self, old: &str, new: &str) -> Result<Value> {
        let main = self.main()?;
        let cur = self.commit_for(&main)?;
        let gd = self.odb.dir().to_path_buf();
        let branch = self.branch.clone();
        if new.bytes().all(|b| b == b'0') {
            bail!("deleting {branch} is not allowed: main is shared with the agents");
        }
        if new == cur {
            return Ok(json!({"status": "up_to_date", "commit": cur, "changes": []}));
        }
        if git::run(Some(&gd), &["rev-parse", "--verify", "--quiet", &format!("{new}^{{commit}}")]).is_err() {
            bail!("{new} is not a commit");
        }
        if !git::is_ancestor(&gd, &cur, new)? {
            let moved = if old == cur { String::new() } else { format!(" (main moved from {old})") };
            bail!(
                "non-fast-forward: your commit does not contain main {}{moved}; \
                 run `git pull --rebase` and push again (TreeYard never rewrites main)",
                &cur[..12]
            );
        }
        // Same connectivity check as git receive-pack: every object reachable from `new` exists.
        git::run(Some(&gd), &["rev-list", "--objects", "--quiet", new, "--not", &cur])
            .context("the push is missing objects")?;
        let chain = git::ancestry_chain(&gd, &cur, new)?;
        let (mut prev_cs, mut prev) = (main, cur);
        let mut landed = Vec::new();
        for commit in &chain {
            let info = git::run(Some(&gd), &["show", "-s", "--format=%ct%n%an%n%B", commit])?;
            let mut lines = info.splitn(3, '\n');
            let time: i64 = lines.next().unwrap_or("0").parse()?;
            let author = one_line(lines.next().unwrap_or(""));
            let message = lines.next().unwrap_or("").trim_end().to_string();
            let subject = message.lines().next().unwrap_or("").trim().to_string();
            let intent = if subject.is_empty() { format!("git push {}", &commit[..12]) } else { subject };
            let id = format!("ch_{:012x}", rand::thread_rng().r#gen::<u64>() & 0xffff_ffff_ffff);
            let changes: Changes = git::diff_tree(&gd, &prev, commit)?.into_iter().collect();
            self.check_pushed(&prev_cs, &changes).with_context(|| format!("refusing commit {}", &commit[..12]))?;
            let meta = Meta {
                change: Some(id.clone()),
                agent: format!("git:{author}"),
                intent: intent.clone(),
                message: Some(message),
                time,
                ..Meta::default()
            };
            let cs = Changeset { parents: vec![prev_cs.clone()], changes, meta };
            let head = self.store.tx(|s| {
                let head = s.put_changeset(&cs)?;
                s.map_commit(&head, commit)?;
                Ok(head)
            })?;
            let ch = Change {
                id: id.clone(),
                state: ChangeState::Open,
                agent: format!("git:{author}"),
                model: None,
                session: Some("git push".into()),
                intent,
                base: prev_cs.clone(),
                head: head.clone(),
                predecessors: vec![],
                conflicts: vec![],
                landed_as: None,
                created: now(),
                declared: Intent::default(),
                view: None,
                group: None,
            };
            self.store.put_change(&ch)?;
            let out = self.land_direct(&id)?;
            if out["status"] != "landed" || out["commit"] != commit.as_str() {
                bail!("internal: pushed commit {commit} did not land as itself: {out}");
            }
            landed.push(json!({"change": id, "changeset": head, "commit": commit}));
            (prev_cs, prev) = (head, commit.clone());
        }
        Ok(json!({"status": "landed", "commit": new, "changes": landed}))
    }

    /// Pushed paths must be safe to write on disk (no `..`, no .git, no file over a directory).
    fn check_pushed(&mut self, prev: &str, changes: &Changes) -> Result<()> {
        let mut m = self.manifest(prev)?;
        for (p, e) in changes {
            check_path(p)?;
            match e {
                Some(e) => m.insert(p.clone(), e.clone()),
                None => m.remove(p),
            };
        }
        for (p, e) in changes {
            if let Some(e) = e {
                m.remove(p);
                check_no_collision(&m, p)?;
                m.insert(p.clone(), e.clone());
            }
        }
        Ok(())
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
        // Incremental export: only the directories on the changed paths are written again;
        // the root changeset (no parent) writes the whole tree once.
        let tree = match parents.first() {
            Some(pc) => {
                let parent_tree = git::commit_tree(&mut self.odb, pc)?;
                git::write_tree_changed(&mut self.odb, &parent_tree, &cs.changes)?
            }
            None => {
                let manifest = self.manifest(id)?;
                git::write_tree(&mut self.odb, &manifest)?
            }
        };
        let m = &cs.meta;
        let slug: String =
            m.agent.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' }).collect();
        let name = m.agent.replace(['<', '>', '\n'], "");
        let author = Signature { name, email: format!("{slug}@agents.treeyard"), time: m.time };
        let committer = Signature { name: "TreeYard".into(), email: "treeyard@localhost".into(), time: m.time };
        let bytes = git::commit_bytes(&tree, &parents, &author, &committer, &commit_message(m, id));
        let commit = self.odb.write("commit", &bytes)?;
        self.store.map_commit(id, &commit)?;
        Ok(commit)
    }

    /// Checks the exported tree of a changeset against a full write of its manifest (the
    /// incremental export must give the same tree). Returns the tree id.
    pub fn verify_export(&mut self, id: &str) -> Result<String> {
        let commit = self.commit_for(id)?;
        let exported = git::commit_tree(&mut self.odb, &commit)?;
        let manifest = self.manifest(id)?;
        let full = git::write_tree(&mut self.odb, &manifest)?;
        if exported != full {
            bail!("export of {id}: commit {commit} has tree {exported}, the manifest gives {full}");
        }
        Ok(full)
    }

    pub fn export_change(&mut self, id: &str) -> Result<Value> {
        let ch = self.change(id)?;
        let commit = self.commit_for(&ch.head)?;
        let name = format!("refs/treeyard/changes/{id}");
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
            "landed_as": ch.landed_as, "created": ch.created, "revisions": ch.predecessors.len() + 1,
            "declared": ch.declared, "view": ch.view, "conflicted": !ch.conflicts.is_empty(), "group": ch.group,
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
            "cache": {"manifests": self.manifests.len(), "objects": self.odb.cached()},
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
        if data.len() > safety::MAX_READ {
            bail!(
                "{path} is {} bytes; read returns files up to {} bytes (use blobs or git clone)",
                data.len(),
                safety::MAX_READ
            );
        }
        let mut v =
            json!({"path": path, "at": cs, "blob": e.blob, "mode": format!("{:o}", e.mode), "size": data.len()});
        match as_text(&data) {
            Some(t) => v["content"] = Value::String(t),
            None => v["content_b64"] = Value::String(B64.encode(&data)),
        }
        Ok(v)
    }

    /// The whole tree of a revision as `[path, mode, blob]` rows (for a node daemon).
    pub fn tree(&mut self, at: Option<&str>) -> Result<Value> {
        let cs = self.resolve_at(at)?;
        let m = self.manifest(&cs)?;
        let entries: Vec<Value> = m.iter().map(|(p, e)| json!([p, e.mode, e.blob])).collect();
        Ok(json!({"at": cs, "commit": self.store.commit_of(&cs)?, "files": entries.len(), "entries": entries}))
    }

    /// Blob contents (base64) for a node daemon, up to about `max_bytes` per call. Ids that did
    /// not fit come back in `rest`; at least one blob is always returned.
    pub fn blobs(&mut self, ids: &[String], max_bytes: usize) -> Result<Value> {
        let mut out = serde_json::Map::new();
        let mut size = 0;
        let mut rest = Vec::new();
        for id in ids {
            if size >= max_bytes && !out.is_empty() {
                rest.push(id.clone());
                continue;
            }
            let data = self.blob(id)?;
            size += data.len();
            out.insert(id.clone(), Value::String(B64.encode(&data)));
        }
        Ok(json!({"blobs": out, "rest": rest}))
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
}

/// Makes the export repo: a bare clone of `src`. git copies a shallow repo through its pack
/// transport (minutes and gigabytes of memory for a big repo), so a local shallow source gets
/// its object directory cloned copy-on-write instead, with its `shallow` file and its refs.
fn clone_bare(src: &Path, git_dir: &Path) -> Result<()> {
    let (src_s, git_s) = (src.to_string_lossy(), git_dir.to_string_lossy());
    let shallow = git::run(None, &["-C", &src_s, "rev-parse", "--is-shallow-repository"]).map(|s| s.trim() == "true");
    if shallow.unwrap_or(false) {
        let common = git::run(None, &["-C", &src_s, "rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        let common = PathBuf::from(common.trim());
        git::run(None, &["init", "--bare", "--quiet", &git_s])?;
        crate::cow::remove_tree(&git_dir.join("objects"))?;
        crate::cow::clone_tree(&common.join("objects"), &git_dir.join("objects"), crate::cow::Strategy::Auto)?;
        fs::copy(common.join("shallow"), git_dir.join("shallow"))?;
        let refs = git::run(
            None,
            &["-C", &src_s, "for-each-ref", "--format=%(objectname) %(refname)", "refs/heads", "refs/tags"],
        )?;
        let mut lines: Vec<&str> = refs.lines().filter(|l| !l.is_empty()).collect();
        lines.sort_by_key(|l| l.split_once(' ').map(|(_, r)| r).unwrap_or(""));
        let mut packed = String::from("# pack-refs with: sorted \n");
        for l in lines {
            packed.push_str(l);
            packed.push('\n');
        }
        fs::write(git_dir.join("packed-refs"), packed)?;
        return Ok(());
    }
    git::run(None, &["clone", "--bare", "--quiet", &src_s, &git_s])?;
    Ok(())
}

/// A group name: 1 to 100 characters on one line, trimmed.
pub fn check_group(group: &str) -> Result<&str> {
    let group = group.trim();
    if group.is_empty() || group.len() > 100 || group.chars().any(char::is_control) {
        bail!("group must be a name of 1 to 100 characters on one line");
    }
    Ok(group)
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
        s.push_str(&format!("TreeYard-Change: {c}\n"));
    }
    s.push_str(&format!("TreeYard-Changeset: {changeset}\n"));
    s.push_str(&format!("TreeYard-Agent: {}\n", one_line(&m.agent)));
    if let Some(x) = &m.model {
        s.push_str(&format!("TreeYard-Model: {}\n", one_line(x)));
    }
    if let Some(x) = &m.session {
        s.push_str(&format!("TreeYard-Session: {}\n", one_line(x)));
    }
    if m.message.is_some() {
        s.push_str(&format!("TreeYard-Intent: {}\n", one_line(&m.intent)));
    }
    s
}
