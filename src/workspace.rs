//! Node-local workspaces: a real directory per agent, cheap to make.
//!
//! The node keeps one materialised base per base changeset (`bases/<id>`). Each workspace is a
//! copy-on-write clone of it (`cow`), bound to one change on the server. `sync` sends the files
//! that differ from the last sync to the server as one new changeset of that change; the daemon
//! calls it after edits settle. Dependency directories (`target`, `node_modules`, `.venv`) are
//! layers keyed by a hash of their lockfiles: the first workspace builds them, later ones clone them.
//!
//! Layout of the node home:
//! ```text
//! node.lock                 node-wide lock (bases, layers, port slots)
//! bases/<changeset>/        materialised tree; bases/<changeset>.json = entries + stat cache
//! layers/<name>-<key>/      dependency dirs at their workspace-relative paths (+ .json)
//! ws/<name>/                the workspace; ws/<name>/.treeyard/env holds its environment
//! meta/<name>.json          workspace record; meta/<name>.state.json = what the server has
//! logs/<name>.<step>.log    create steps, setup and teardown output
//! gc.lock                   held while a gc runs
//! workspace.toml            node-local workspace config (overrides the repo's)
//! ```
//!
//! A repo can describe its workspaces in `.treeyard/workspace.toml` (setup and teardown commands,
//! layers, gitignored files to copy in, port block size, default `exec` command). The file is
//! read from the base tree, the node file overrides it key by key, and CLI flags override both.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::client;
use crate::cow::{self, Strategy};
use crate::git;
use crate::model::{MODE_EXEC, MODE_FILE, MODE_GITLINK, MODE_LINK};

/// The workspace's private directory: never synced, never part of a layer.
pub const PRIVATE: &str = ".treeyard";
/// One sync request carries at most about this many bytes of file content.
const BATCH_BYTES: usize = 8 << 20;
pub const DEFAULT_PORT_BASE: u16 = 20000;
pub const DEFAULT_PORT_BLOCK: u16 = 10;
/// The repo's workspace config, read from the base tree. The only repo path under `.treeyard/`.
pub const REPO_CONFIG: &str = ".treeyard/workspace.toml";
/// Lines of this root file name more gitignored files to copy in (like `copy`).
const WORKTREE_INCLUDE: &str = ".worktreeinclude";
/// The file in a mirror directory that lists what the mirror wrote (and marks it as one).
pub const MIRROR_MARK: &str = ".treeyard-mirror.json";
/// Most directories of a sparse workspace (`--paths`).
pub const MAX_SPARSE_PATHS: usize = 200;
/// Directories that `copy` never walks into in the source directory.
const COPY_SKIP: &[&str] = &[".git", PRIVATE, "target", "node_modules", ".venv"];

/// What the node knows about one file: git mode and blob, plus the stat that produced them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    pub mode: u32,
    pub blob: String,
    pub size: u64,
    pub mtime_ns: i64,
}

pub type State = BTreeMap<String, FileState>;

/// A dependency layer: directories named `dir` anywhere in the workspace (outside tracked
/// directories), keyed by the content of `keys` (lockfiles).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerSpec {
    pub dir: String,
    pub keys: Vec<String>,
}

impl std::str::FromStr for LayerSpec {
    type Err = anyhow::Error;
    /// `node_modules=pnpm-lock.yaml` or `target=Cargo.lock,rust-toolchain.toml`.
    fn from_str(s: &str) -> Result<Self> {
        let (dir, keys) = s.split_once('=').ok_or_else(|| anyhow!("layer {s:?}: write DIR=LOCKFILE[,LOCKFILE]"))?;
        let keys: Vec<String> = keys.split(',').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect();
        if dir.is_empty() || dir.contains('/') || keys.is_empty() {
            bail!("layer {s:?}: DIR is one directory name, then at least one lockfile");
        }
        Ok(LayerSpec { dir: dir.to_string(), keys })
    }
}

/// Lockfile -> dependency directory, for `--layers auto`.
const AUTO_LAYERS: &[(&str, &str)] = &[
    ("Cargo.lock", "target"),
    ("pnpm-lock.yaml", "node_modules"),
    ("package-lock.json", "node_modules"),
    ("yarn.lock", "node_modules"),
    ("bun.lockb", "node_modules"),
    ("uv.lock", ".venv"),
    ("poetry.lock", ".venv"),
    ("Pipfile.lock", ".venv"),
    ("requirements-dev.txt", ".venv"),
    ("requirements.txt", ".venv"),
];

/// Layers implied by the lockfiles at the root of a tree (first lockfile per directory wins).
pub fn auto_layers(paths: &BTreeSet<String>) -> Vec<LayerSpec> {
    let mut out: Vec<LayerSpec> = Vec::new();
    for (lock, dir) in AUTO_LAYERS {
        if paths.contains(*lock) && !out.iter().any(|l| l.dir == *dir) {
            out.push(LayerSpec { dir: dir.to_string(), keys: vec![lock.to_string()] });
        }
    }
    out
}

/// `.treeyard/workspace.toml` (repo) or `<home>/workspace.toml` (node). Every key is optional.
/// The repo file runs commands on the node that creates the workspace, so it is trusted like a
/// Makefile; commands that the server runs stay in `<data>/treeyard.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WsConfig {
    /// Shell command run in a new workspace (like `--setup`).
    pub setup: Option<String>,
    /// Shell command run by `remove` before the directory goes; a failure is reported only.
    pub teardown: Option<String>,
    /// Dependency layers, `DIR=LOCKFILE[,LOCKFILE]` each (like `--layer`).
    pub layers: Option<Vec<String>>,
    /// Globs of gitignored files (`.env*`, certificates) to copy from `copy_from` (or `--copy-from`).
    pub copy: Option<Vec<String>>,
    pub copy_from: Option<String>,
    /// Ports per workspace (default 10).
    pub ports: Option<u16>,
    /// Default command of `workspace exec NAME` with no command.
    pub run: Option<String>,
}

impl WsConfig {
    pub fn parse(text: &str, origin: &str) -> Result<Self> {
        toml::from_str(text).with_context(|| format!("parse {origin}"))
    }

    /// `self` with every key it lacks taken from `under`.
    pub fn over(self, under: WsConfig) -> WsConfig {
        WsConfig {
            setup: self.setup.or(under.setup),
            teardown: self.teardown.or(under.teardown),
            layers: self.layers.or(under.layers),
            copy: self.copy.or(under.copy),
            copy_from: self.copy_from.or(under.copy_from),
            ports: self.ports.or(under.ports),
            run: self.run.or(under.run),
        }
    }

    fn layer_specs(&self) -> Result<Option<Vec<LayerSpec>>> {
        self.layers.as_ref().map(|l| l.iter().map(|s| s.parse()).collect()).transpose()
    }
}

/// Where a workspace is in its life. `create` goes creating -> setting_up -> ready, or ends in
/// failed (the record and the logs stay); `remove` marks it removing before the directory goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsState {
    Creating,
    SettingUp,
    #[default]
    Ready,
    Removing,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub url: String,
    pub agent: String,
    pub change: String,
    pub base: String,
    pub dir: PathBuf,
    pub strategy: Strategy,
    pub slot: u16,
    pub ports: (u16, u16),
    pub layers: Vec<Value>,
    pub created: i64,
    /// Run by `remove` before the directory goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown: Option<String>,
    /// Default command of `exec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default)]
    pub state: WsState,
    /// What a failed create or setup said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `<home>/logs/<name>.create.log`: one line per create step with its time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_log: Option<PathBuf>,
    /// `<home>/logs/<name>.setup.log`: output of the setup command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_log: Option<PathBuf>,
    /// Files cloned, layers restored and saved, files copied, seconds.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub counts: Value,
    /// Unix time of the last sync that sent a changeset (0 = none yet).
    #[serde(default)]
    pub synced: i64,
    /// A stable directory that every sync updates with the change's files (`workspace mirror`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<PathBuf>,
    /// Sparse workspace: the directories it holds (plus root files). Empty = the whole tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl Workspace {
    pub fn env(&self) -> Vec<(String, String)> {
        vec![
            ("TREEYARD_URL".into(), self.url.clone()),
            ("TREEYARD_WORKSPACE".into(), self.name.clone()),
            ("TREEYARD_CHANGE".into(), self.change.clone()),
            ("TREEYARD_AGENT".into(), self.agent.clone()),
            ("TREEYARD_PORT_BASE".into(), self.ports.0.to_string()),
            ("TREEYARD_PORT_COUNT".into(), (self.ports.1 - self.ports.0 + 1).to_string()),
            ("PORT".into(), self.ports.0.to_string()),
            (
                "TREEYARD_DB".into(),
                format!("treeyard_{}", self.name.replace(|c: char| !c.is_ascii_alphanumeric(), "_")),
            ),
        ]
    }
}

/// Options for `create`.
#[derive(Clone, Debug)]
pub struct CreateOptions {
    pub name: String,
    pub agent: String,
    pub intent: String,
    pub base: Option<String>,
    pub strategy: Strategy,
    /// `None` = auto-detect from the lockfiles at the root.
    pub layers: Option<Vec<LayerSpec>>,
    /// Shell command run in the new workspace after the layers are restored (for example
    /// `pnpm install --offline`); layers that were missing are saved after it.
    pub setup: Option<String>,
    /// Put a `.git` in the workspace (HEAD = the base commit, history through the node's git
    /// cache), so `git status`, `git diff` and `git log` work. Edits still reach the server by sync.
    pub git: bool,
    /// Overrides `teardown` of the config files.
    pub teardown: Option<String>,
    /// Overrides `run` of the config files.
    pub run: Option<String>,
    /// Ports in the workspace's block; overrides `ports` of the config files.
    pub ports: Option<u16>,
    /// Where the `copy` globs (and `.worktreeinclude`) find gitignored files; overrides `copy_from`.
    pub copy_from: Option<PathBuf>,
    /// Sparse workspace: only these directories plus root files (git's cone mode). Empty = all.
    pub paths: Vec<String>,
    /// A git checkout whose uncommitted edits (changes against its HEAD, untracked files that
    /// are not ignored) seed the new change as its first changeset.
    pub from_dir: Option<PathBuf>,
}

impl Default for CreateOptions {
    fn default() -> Self {
        CreateOptions {
            name: String::new(),
            agent: "agent".into(),
            intent: "work in a workspace".into(),
            base: None,
            strategy: Strategy::Auto,
            layers: None,
            setup: None,
            git: false,
            teardown: None,
            run: None,
            ports: None,
            copy_from: None,
            paths: vec![],
            from_dir: None,
        }
    }
}

/// Options for `gc`. A limit of `None` is no limit.
#[derive(Clone, Debug)]
pub struct GcOptions {
    /// Keep at most this many workspaces.
    pub max: Option<usize>,
    /// Keep at most this many bytes in workspace directories (apparent size: clones count in full).
    pub max_bytes: Option<u64>,
    /// Report what would go, remove nothing.
    pub dry_run: bool,
    /// Never remove a workspace younger than this (default 10 minutes).
    pub min_age: std::time::Duration,
}

impl Default for GcOptions {
    fn default() -> Self {
        GcOptions {
            max: Some(25),
            max_bytes: Some(50 << 30),
            dry_run: false,
            min_age: std::time::Duration::from_secs(600),
        }
    }
}

/// One node: a home directory and the server it talks to.
pub struct Node {
    pub home: PathBuf,
    pub url: String,
    pub port_base: u16,
    pub port_block: u16,
}

fn check_rel(path: &str) -> Result<()> {
    let private = path != REPO_CONFIG && path.split('/').any(|c| c == PRIVATE);
    let bad = crate::safety::check_path(path).is_err() || private;
    if bad {
        bail!("refusing unsafe path {path:?} from the server");
    }
    Ok(())
}

fn write_json(path: &Path, v: &impl Serialize) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec(v)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))
}

fn mtime_ns(m: &fs::Metadata) -> i64 {
    m.mtime() * 1_000_000_000 + m.mtime_nsec()
}

/// Git mode of a file on disk.
fn disk_mode(m: &fs::Metadata) -> u32 {
    if m.file_type().is_symlink() {
        MODE_LINK
    } else if m.permissions().mode() & 0o111 != 0 {
        MODE_EXEC
    } else {
        MODE_FILE
    }
}

/// Reads a file (or a symlink's target) and returns its git mode, blob id and bytes.
fn hash_file(path: &Path, m: &fs::Metadata) -> Result<(u32, String, Vec<u8>)> {
    let mode = disk_mode(m);
    let data = if mode == MODE_LINK {
        use std::os::unix::ffi::OsStrExt;
        fs::read_link(path)?.as_os_str().as_bytes().to_vec()
    } else {
        fs::read(path)?
    };
    Ok((mode, git::object_id("blob", &data), data))
}

struct Lock(#[allow(dead_code)] fs::File);

fn lock(path: &Path) -> Result<Lock> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let f = fs::File::options().create(true).truncate(false).write(true).open(path)?;
    f.lock().with_context(|| format!("lock {}", path.display()))?;
    Ok(Lock(f))
}

/// A step log: one line per step, with the seconds since it was opened.
struct StepLog {
    path: PathBuf,
    file: fs::File,
    t0: Instant,
}

impl StepLog {
    fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let file = fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
        Ok(StepLog { path: path.to_path_buf(), file, t0: Instant::now() })
    }

    fn line(&mut self, text: &str) {
        use std::io::Write;
        let _ = writeln!(self.file, "{:8.3} s  {text}", self.t0.elapsed().as_secs_f64());
    }
}

impl Node {
    pub fn new(home: impl Into<PathBuf>, url: impl Into<String>) -> Self {
        Node { home: home.into(), url: url.into(), port_base: DEFAULT_PORT_BASE, port_block: DEFAULT_PORT_BLOCK }
    }

    /// `TREEYARD_HOME`, else `~/.treeyard`.
    pub fn default_home() -> PathBuf {
        if let Some(h) = std::env::var_os("TREEYARD_HOME") {
            return h.into();
        }
        PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".treeyard")
    }

    fn call(&self, op: &str, params: Value) -> Result<Value> {
        client::call(&self.url, op, params)
    }

    fn meta_path(&self, name: &str) -> PathBuf {
        self.home.join("meta").join(format!("{name}.json"))
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.home.join("meta").join(format!("{name}.state.json"))
    }

    pub fn ws_dir(&self, name: &str) -> PathBuf {
        self.home.join("ws").join(name)
    }

    fn base_dir(&self, cs: &str) -> PathBuf {
        self.home.join("bases").join(cs)
    }

    // ---- bases ---------------------------------------------------------------------------

    /// Materialises the tree of changeset `cs` once per node and returns its directory and its
    /// state (entries plus stat). A new base starts as a clone of the newest existing base and
    /// fetches only the blobs that differ.
    pub fn base(&self, cs: &str) -> Result<(PathBuf, State)> {
        let _l = lock(&self.home.join("node.lock"))?;
        let dir = self.base_dir(cs);
        let meta = dir.with_extension("json");
        if dir.exists() && meta.exists() {
            return Ok((dir, read_json(&meta)?));
        }
        let tree = self.call("tree", json!({"at": cs}))?;
        let mut want: BTreeMap<String, (u32, String)> = BTreeMap::new();
        for row in tree["entries"].as_array().context("tree has no entries")? {
            let path = row[0].as_str().context("bad tree row")?.to_string();
            check_rel(&path)?;
            let mode = row[1].as_u64().context("bad tree row")? as u32;
            want.insert(path, (mode, row[2].as_str().context("bad tree row")?.to_string()));
        }
        crate::safety::check_tree(want.keys().map(String::as_str))?;
        let tmp = dir.with_extension("partial");
        cow::remove_tree(&tmp)?;
        let mut have: State = State::new();
        match self.newest_base()? {
            Some((tdir, tstate)) => {
                cow::clone_tree(&tdir, &tmp, Strategy::Auto)?;
                // The template's git dir describes the template's commit, not this one.
                cow::remove_tree(&tmp.join(".git"))?;
                have = tstate;
            }
            None => fs::create_dir_all(&tmp)?,
        }
        // Remove what the new tree lacks or changes kind, then write what differs.
        for (path, st) in &have {
            if want.get(path).is_none_or(|(m, b)| (*m, b) != (st.mode, &st.blob)) {
                let p = tmp.join(path);
                if st.mode == MODE_GITLINK {
                    let _ = fs::remove_dir(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
            }
        }
        prune_empty_dirs(&tmp, have.keys().filter(|p| !want.contains_key(*p)))?;
        let missing: Vec<String> = want
            .iter()
            .filter(|(p, (m, b))| *m != MODE_GITLINK && have.get(*p).is_none_or(|s| (s.mode, &s.blob) != (*m, b)))
            .map(|(_, (_, b))| b.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let blobs = self.fetch_blobs(&missing)?;
        let mut state = State::new();
        for (path, (mode, blob)) in &want {
            let kept = have.get(path).is_some_and(|s| (s.mode, &s.blob) == (*mode, blob));
            let p = if kept { tmp.join(path) } else { crate::safety::dest(&tmp, path)? };
            if !kept {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)?;
                }
                match *mode {
                    MODE_GITLINK => fs::create_dir_all(&p)?,
                    MODE_LINK => {
                        let target = String::from_utf8_lossy(&blobs[blob]).into_owned();
                        std::os::unix::fs::symlink(target, &p)?;
                    }
                    m => {
                        fs::write(&p, &blobs[blob])?;
                        let perm = if m == MODE_EXEC { 0o755 } else { 0o644 };
                        fs::set_permissions(&p, fs::Permissions::from_mode(perm))?;
                    }
                }
            }
            if *mode == MODE_GITLINK {
                continue;
            }
            let md = fs::symlink_metadata(&p)?;
            state.insert(
                path.clone(),
                FileState { mode: *mode, blob: blob.clone(), size: md.len(), mtime_ns: mtime_ns(&md) },
            );
        }
        write_json(&tmp.with_extension("partial.json"), &state)?;
        fs::rename(&tmp, &dir)?;
        fs::rename(tmp.with_extension("partial.json"), &meta)?;
        Ok((dir, state))
    }

    /// Adds a `.git` to the base of changeset `cs`: index and HEAD at its commit, objects through
    /// an alternate to the node's bare cache (`git.cache`), fetched from the server's `/git`.
    pub fn git_base(&self, cs: &str) -> Result<()> {
        let dir = self.base_dir(cs);
        let ready = dir.join(".git").join("treeyard-ready");
        if ready.exists() {
            return Ok(());
        }
        let _l = lock(&self.home.join("node.lock"))?;
        if ready.exists() {
            return Ok(());
        }
        let status = self.call("status", json!({}))?;
        let branch = status["branch"].as_str().unwrap_or("main").to_string();
        let commit = match self.call("tree", json!({"at": cs}))?["commit"].as_str() {
            Some(c) => c.to_string(),
            None => bail!("base {} has no git commit yet (an unlanded change); create without --git", &cs[..12]),
        };
        let cache = self.home.join("git.cache");
        let run = |dir: &Path, args: &[&str]| git::run(Some(dir), args).map(|_| ());
        if !cache.exists() {
            git::run(None, &["init", "-q", "--bare", &cache.to_string_lossy()])?;
        }
        let have = |c: &str| git::run(Some(&cache), &["cat-file", "-e", &format!("{c}^{{commit}}")]).is_ok();
        if !have(&commit) {
            let url = format!("{}/git", self.url.trim_end_matches('/'));
            run(&cache, &["fetch", "-q", "--no-tags", &url, "+refs/heads/*:refs/heads/*"])?;
        }
        if !have(&commit) {
            bail!("commit {commit} is not on the server's branches; create without --git");
        }
        let gd = dir.join(".git");
        cow::remove_tree(&gd)?;
        git::run(None, &["init", "-q", &dir.to_string_lossy()])?;
        fs::write(gd.join("objects/info/alternates"), format!("{}\n", cache.join("objects").display()))?;
        fs::create_dir_all(gd.join("info"))?;
        fs::write(gd.join("info/exclude"), format!("/{PRIVATE}/\n"))?;
        // Clones get new inodes and ctimes; compare only mtime and size, like the sync stat cache.
        run(&gd, &["config", "core.checkStat", "minimal"])?;
        run(&gd, &["config", "core.trustctime", "false"])?;
        run(&gd, &["update-ref", &format!("refs/heads/{branch}"), &commit])?;
        run(&gd, &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])?;
        let wt = format!("--work-tree={}", dir.display());
        run(&gd, &[&wt, "read-tree", &commit])?;
        run(&gd, &[&wt, "update-index", "-q", "--refresh"])?;
        fs::write(ready, &commit)?;
        Ok(())
    }

    fn newest_base(&self) -> Result<Option<(PathBuf, State)>> {
        let root = self.home.join("bases");
        let Ok(rd) = fs::read_dir(&root) else { return Ok(None) };
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
        for e in rd {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "json") && p.with_extension("").is_dir() {
                let t = fs::metadata(&p)?.modified()?;
                if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                    best = Some((t, p));
                }
            }
        }
        match best {
            Some((_, meta)) => Ok(Some((meta.with_extension(""), read_json(&meta)?))),
            None => Ok(None),
        }
    }

    fn fetch_blobs(&self, ids: &[String]) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut out = BTreeMap::new();
        let mut todo: Vec<String> = ids.to_vec();
        while !todo.is_empty() {
            let chunk: Vec<String> = todo.drain(..todo.len().min(2000)).collect();
            let mut ask = chunk;
            while !ask.is_empty() {
                let r = self.call("blobs", json!({"ids": ask}))?;
                for (id, b) in r["blobs"].as_object().context("blobs reply")? {
                    out.insert(id.clone(), B64.decode(b.as_str().unwrap_or(""))?);
                }
                ask = serde_json::from_value(r["rest"].clone())?;
            }
        }
        Ok(out)
    }

    // ---- layers --------------------------------------------------------------------------

    fn layer_key(&self, ws: &Path, spec: &LayerSpec) -> Result<String> {
        let mut h = Sha256::new();
        h.update(format!("{}\0{}\0{}\0", spec.dir, std::env::consts::OS, std::env::consts::ARCH));
        for k in &spec.keys {
            h.update(k.as_bytes());
            h.update([0]);
            match fs::read(ws.join(k)) {
                Ok(d) => h.update(&d),
                Err(_) => h.update(b"(missing)"),
            }
            h.update([0]);
        }
        Ok(hex::encode(&h.finalize()[..8]))
    }

    fn layer_dir(&self, spec: &LayerSpec, key: &str) -> PathBuf {
        let safe = spec.dir.trim_start_matches('.');
        self.home.join("layers").join(format!("{safe}-{key}"))
    }

    /// Clones a saved layer into the workspace. Returns false if the layer does not exist yet.
    fn restore_layer(&self, ws: &Path, spec: &LayerSpec, key: &str) -> Result<bool> {
        let dir = self.layer_dir(spec, key);
        let Ok(paths) = read_json::<Vec<String>>(&dir.with_extension("json")) else { return Ok(false) };
        for rel in paths {
            check_rel(&rel)?;
            let dst = ws.join(&rel);
            cow::remove_tree(&dst)?;
            cow::clone_tree(&dir.join(&rel), &dst, Strategy::Auto)?;
        }
        Ok(true)
    }

    /// Saves every directory named `spec.dir` in the workspace (outside tracked directories)
    /// as the layer for `key`. Returns the saved relative paths.
    pub fn save_layer(&self, ws: &Path, spec: &LayerSpec, key: &str, tracked: &State) -> Result<Vec<String>> {
        let dir = self.layer_dir(spec, key);
        if dir.with_extension("json").exists() {
            return read_json(&dir.with_extension("json"));
        }
        let found = find_dirs(ws, &spec.dir, tracked)?;
        if found.is_empty() {
            return Ok(found);
        }
        let tmp = dir.with_extension("partial");
        cow::remove_tree(&tmp)?;
        for rel in &found {
            cow::clone_tree(&ws.join(rel), &tmp.join(rel), Strategy::Auto)?;
        }
        if dir.exists() {
            cow::remove_tree(&dir)?;
        }
        fs::rename(&tmp, &dir)?;
        write_json(&dir.with_extension("json"), &found)?;
        Ok(found)
    }

    // ---- workspaces ----------------------------------------------------------------------

    /// The workspace config for a tree at `root`: the repo file there, under the node file.
    pub fn config(&self, root: &Path) -> Result<WsConfig> {
        let read = |p: PathBuf| -> Result<WsConfig> {
            match fs::read_to_string(&p) {
                Ok(t) => WsConfig::parse(&t, &p.display().to_string()),
                Err(_) => Ok(WsConfig::default()),
            }
        };
        Ok(read(self.home.join("workspace.toml"))?.over(read(root.join(REPO_CONFIG))?))
    }

    /// A free block of `n` ports: (slot, first port). Slots are `port_block` apart; a bigger
    /// block takes several slots. A block whose first port another process holds is skipped.
    fn free_ports(&self, n: u16) -> Result<(u16, u16)> {
        if n == 0 {
            bail!("a workspace needs at least one port");
        }
        let used: Vec<(u16, u16)> = self.list()?.iter().map(|w| w.ports).collect();
        let max_slot = (u16::MAX - self.port_base) / self.port_block;
        for slot in 0..max_slot {
            let lo = self.port_base as u32 + slot as u32 * self.port_block as u32;
            let hi = lo + n as u32 - 1;
            if hi > u16::MAX as u32 {
                break;
            }
            if used.iter().any(|(a, b)| lo <= *b as u32 && *a as u32 <= hi) {
                continue;
            }
            if std::net::TcpListener::bind(("127.0.0.1", lo as u16)).is_ok() {
                return Ok((slot, lo as u16));
            }
        }
        bail!("no free block of {n} ports")
    }

    /// Opens a change and makes its workspace. The record is written first (state `creating`),
    /// so `list` shows a create in progress; a failed step leaves state `failed`, the error and
    /// `create_log`, and `remove` cleans up.
    pub fn create(&self, o: &CreateOptions) -> Result<(Workspace, Value)> {
        let t0 = Instant::now();
        check_rel(&o.name).map_err(|_| anyhow!("bad workspace name {:?}", o.name))?;
        if o.name.contains('/') {
            bail!("bad workspace name {:?}", o.name);
        }
        if self.meta_path(&o.name).exists() {
            bail!("workspace {} already exists", o.name);
        }
        let dir = self.ws_dir(&o.name);
        if dir.exists() {
            bail!("{} already exists", dir.display());
        }
        let ch = self.call(
            "open_change",
            json!({"agent": o.agent, "intent": o.intent, "base": o.base, "session": format!("workspace:{}", o.name)}),
        )?;
        let change = ch["id"].as_str().context("open_change reply")?.to_string();
        let base = ch["base"].as_str().context("open_change reply")?.to_string();
        let mut ws = Workspace {
            name: o.name.clone(),
            url: self.url.clone(),
            agent: o.agent.clone(),
            change: change.clone(),
            base: base.clone(),
            dir: dir.clone(),
            strategy: o.strategy,
            slot: 0,
            ports: (0, 0),
            layers: vec![],
            created: crate::repo::now(),
            teardown: None,
            run: None,
            state: WsState::Creating,
            error: None,
            create_log: None,
            setup_log: None,
            counts: Value::Null,
            synced: 0,
            mirror: None,
            paths: vec![],
        };
        let mut log = StepLog::open(&self.home.join("logs").join(format!("{}.create.log", o.name)))?;
        ws.create_log = Some(log.path.clone());
        {
            let _l = lock(&self.home.join("node.lock"))?;
            if self.meta_path(&o.name).exists() {
                bail!("workspace {} already exists", o.name);
            }
            fs::create_dir_all(self.home.join("meta"))?;
            write_json(&self.meta_path(&o.name), &ws)?;
        }
        log.line(&format!("change {change} on base {base}"));
        match self.create_steps(o, &mut ws, &mut log, t0) {
            Ok(timing) => Ok((ws, timing)),
            Err(e) => {
                log.line(&format!("failed: {e:#}"));
                ws.state = WsState::Failed;
                ws.error = Some(format!("{e:#}"));
                let _ = write_json(&self.meta_path(&o.name), &ws);
                Err(e.context(format!("workspace {} failed (log: {})", o.name, log.path.display())))
            }
        }
    }

    fn create_steps(&self, o: &CreateOptions, ws: &mut Workspace, log: &mut StepLog, t0: Instant) -> Result<Value> {
        let (base, dir) = (ws.base.clone(), ws.dir.clone());
        let (base_dir, state) = self.base(&base)?;
        if o.git {
            self.git_base(&base)?;
        }
        ws.paths = sparse_paths(&o.paths, &base_dir)?;
        if !ws.paths.is_empty() && o.strategy == Strategy::Overlay {
            bail!("--paths does not work with --strategy overlay (an overlay shows the whole base)");
        }
        // What the workspace holds; the base (and `state`) is always the whole tree.
        let ws_state: State =
            state.iter().filter(|(p, _)| in_cone(&ws.paths, p)).map(|(p, f)| (p.clone(), f.clone())).collect();
        let t_base = t0.elapsed().as_secs_f64();
        log.line(&format!("base ready: {} files", state.len()));
        // CLI options, then the node's workspace.toml, then the repo's .treeyard/workspace.toml.
        let cfg = self.config(&base_dir)?;
        let setup = o.setup.clone().or(cfg.setup.clone());
        let specs = match (o.layers.clone(), cfg.layer_specs()?) {
            (Some(l), _) | (None, Some(l)) => l,
            (None, None) => auto_layers(&state.keys().cloned().collect()),
        };
        let keys = specs.iter().map(|s| Ok((s.clone(), self.layer_key(&base_dir, s)?))).collect::<Result<Vec<_>>>()?;
        let spare = if matches!(o.strategy, Strategy::Auto | Strategy::Clone) && ws.paths.is_empty() {
            self.take_spare(&base, &keys, o.git, &dir)?
        } else {
            None
        };
        ws.strategy = if let Some(s) = spare {
            s
        } else if o.strategy == Strategy::Overlay {
            cow::overlay_mount(&base_dir, &dir)?;
            Strategy::Overlay
        } else {
            if o.strategy == Strategy::Hardlink {
                cow::make_read_only(&base_dir)?;
            }
            if ws.paths.is_empty() {
                cow::clone_tree(&base_dir, &dir, o.strategy)?
            } else {
                clone_sparse(&base_dir, &dir, &ws.paths, &ws_state, o.strategy, o.git)?
            }
        };
        let t_clone = t0.elapsed().as_secs_f64() - t_base;
        log.line(&format!("cloned {} files ({}, spare: {})", ws_state.len(), ws.strategy.as_str(), spare.is_some()));
        ws.teardown = o.teardown.clone().or(cfg.teardown.clone());
        ws.run = o.run.clone().or(cfg.run.clone());
        {
            let _l = lock(&self.home.join("node.lock"))?;
            let n = o.ports.or(cfg.ports).unwrap_or(self.port_block);
            let (slot, lo) = self.free_ports(n)?;
            (ws.slot, ws.ports) = (slot, (lo, lo + n - 1));
            write_json(&self.state_path(&o.name), &ws_state)?;
            write_json(&self.meta_path(&o.name), &*ws)?;
        }
        log.line(&format!("ports {}-{}", ws.ports.0, ws.ports.1));
        fs::create_dir_all(dir.join(PRIVATE))?;
        let env: String = ws.env().iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        fs::write(dir.join(PRIVATE).join("env"), env)?;

        // The uncommitted edits of a checkout, before the setup (it may need them); synced once ready.
        let seeded = match &o.from_dir {
            Some(from) => {
                let v = seed_from(from, &dir, &ws.paths, &ws_state)?;
                log.line(&format!(
                    "seeded {} files from {}",
                    v["written"].as_array().map_or(0, Vec::len),
                    from.display()
                ));
                v
            }
            None => Value::Null,
        };

        // Gitignored files (`.env`, certificates) from a source directory, before the setup needs them.
        let from = o.copy_from.clone().or(cfg.copy_from.as_deref().map(expand_home));
        let copied = self.copy_in(from.as_deref(), cfg.copy.as_deref().unwrap_or_default(), &dir, &state)?;
        let n_copied = copied["copied"].as_array().map_or(0, Vec::len);
        if !copied.is_null() {
            log.line(&format!("copied {n_copied} gitignored files"));
        }

        // Dependency layers: restore what exists (a spare has them), run the setup, save what was missing.
        ws.state = WsState::SettingUp;
        write_json(&self.meta_path(&o.name), &*ws)?;
        let mut layers = Vec::new();
        for (spec, key) in keys {
            // A saved layer is immutable (written to a temp dir, then renamed), so no lock.
            let restored = spare.is_some() || self.restore_layer(&dir, &spec, &key)?;
            log.line(&format!("layer {} {key}: {}", spec.dir, if restored { "restored" } else { "missing" }));
            layers.push((spec, key, restored));
        }
        let t_layers = t0.elapsed().as_secs_f64() - t_base - t_clone;
        let mut setup_s = 0.0;
        if let Some(cmd) = &setup {
            let t = Instant::now();
            let (ok, path) = self.run_logged(ws, "setup", cmd)?;
            setup_s = t.elapsed().as_secs_f64();
            ws.setup_log = Some(path.clone());
            log.line(&format!("setup {cmd:?}: {} in {setup_s:.3} s", if ok { "ok" } else { "failed" }));
            if !ok {
                let text = fs::read_to_string(&path).unwrap_or_default();
                let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
                bail!("setup {cmd:?} failed in {}: {last} (output: {})", dir.display(), path.display());
            }
        }
        let mut saved = 0;
        for (spec, key, restored) in &layers {
            let paths = if *restored || setup.is_none() {
                vec![]
            } else {
                let _l = lock(&self.home.join("node.lock"))?;
                self.save_layer(&dir, spec, key, &state)?
            };
            saved += usize::from(!paths.is_empty());
            ws.layers
                .push(json!({"dir": spec.dir, "keys": spec.keys, "key": key, "restored": restored, "saved": paths}));
        }
        let total = round(t0.elapsed().as_secs_f64());
        let restored = layers.iter().filter(|l| l.2).count();
        ws.counts = json!({
            "files": ws_state.len(), "layers_restored": restored, "layers_saved": saved, "files_copied": n_copied,
            "seconds": total,
        });
        ws.state = WsState::Ready;
        write_json(&self.meta_path(&o.name), &*ws)?;
        log.line(&format!("ready in {total} s"));
        let mut from_dir = seeded;
        if !from_dir.is_null() {
            // The seed is the change's first changeset. A failed sync is reported; the daemon retries.
            from_dir["sync"] = self.sync(&o.name).unwrap_or_else(|e| json!({"error": format!("{e:#}")}));
        }
        Ok(json!({
            "base_s": round(t_base), "clone_s": round(t_clone), "layers_s": round(t_layers), "setup_s": round(setup_s),
            "total_s": total, "spare": spare.is_some(), "copy": copied, "from_dir": from_dir,
        }))
    }

    /// Copies gitignored files that match `globs` or the lines of `.worktreeinclude` from `from`
    /// into the workspace. A file that is tracked, already there, or not ignored by the
    /// workspace's `.gitignore` (so a sync would send it to the server) is skipped with a reason.
    fn copy_in(&self, from: Option<&Path>, globs: &[String], ws: &Path, tracked: &State) -> Result<Value> {
        let mut patterns = globs.to_vec();
        for root in [Some(ws), from].into_iter().flatten() {
            if let Ok(t) = fs::read_to_string(root.join(WORKTREE_INCLUDE)) {
                let lines = t.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(['#', '!']));
                patterns.extend(lines.map(String::from));
            }
        }
        if patterns.is_empty() {
            return Ok(Value::Null);
        }
        let Some(from) = from else {
            return Ok(json!({"error": "copy globs are set, but no source: pass --copy-from DIR or set copy_from"}));
        };
        let set = crate::config::globs(&patterns)?;
        let (mut copied, mut skipped) = (Vec::new(), Vec::new());
        let mut stack = vec![String::new()];
        while let Some(rel) = stack.pop() {
            let rd = fs::read_dir(from.join(&rel)).with_context(|| format!("read {}", from.join(&rel).display()))?;
            for e in rd {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
                let ft = e.file_type()?;
                if ft.is_dir() {
                    if !COPY_SKIP.contains(&name.as_str()) {
                        stack.push(child);
                    }
                    continue;
                }
                if !set.is_match(&child) || check_rel(&child).is_err() {
                    continue;
                }
                let why = if tracked.contains_key(&child) {
                    Some("tracked")
                } else if fs::symlink_metadata(ws.join(&child)).is_ok() {
                    Some("already in the workspace")
                } else if !ignored(ws, &child) {
                    Some("not ignored by .gitignore, so a sync would send it")
                } else {
                    None
                };
                if let Some(why) = why {
                    skipped.push(json!({"path": child, "why": why}));
                    continue;
                }
                let dst = crate::safety::dest(ws, &child)?;
                if let Some(p) = dst.parent() {
                    fs::create_dir_all(p)?;
                }
                if ft.is_symlink() {
                    std::os::unix::fs::symlink(fs::read_link(e.path())?, &dst)?;
                } else {
                    fs::copy(e.path(), &dst)?;
                    fs::File::options().write(true).open(&dst)?.set_modified(e.metadata()?.modified()?)?;
                }
                copied.push(child);
            }
        }
        copied.sort();
        Ok(json!({"from": from, "copied": copied, "skipped": skipped}))
    }

    /// Runs a workspace command (setup, teardown) with the workspace environment; its output
    /// goes to `<home>/logs/<name>.<step>.log`. Returns whether it succeeded and the log path.
    fn run_logged(&self, ws: &Workspace, step: &str, cmd: &str) -> Result<(bool, PathBuf)> {
        let logs = self.home.join("logs");
        fs::create_dir_all(&logs)?;
        let path = logs.join(format!("{}.{step}.log", ws.name));
        let log = fs::File::create(&path)?;
        let status = std::process::Command::new("sh")
            .args(["-c", cmd])
            .current_dir(&ws.dir)
            .envs(ws.env())
            .stdout(log.try_clone()?)
            .stderr(log)
            .status()
            .with_context(|| format!("run {step}"))?;
        Ok((status.success(), path))
    }

    // ---- spares: a warm pool of ready directories ---------------------------------------

    /// Makes one spare workspace directory for base changeset `base`: a clone of the base with
    /// every layer that already exists restored. `create` takes it with one rename.
    pub fn make_spare(&self, base: &str, layers: Option<&[LayerSpec]>, git: bool) -> Result<Value> {
        let t0 = Instant::now();
        let (base_dir, state) = self.base(base)?;
        let specs = match (layers, self.config(&base_dir)?.layer_specs()?) {
            (Some(l), _) => l.to_vec(),
            (None, Some(l)) => l,
            (None, None) => auto_layers(&state.keys().cloned().collect()),
        };
        let root = self.home.join("spares");
        fs::create_dir_all(&root)?;
        let id = format!("{:016x}", rand::random::<u64>());
        let tmp = root.join(format!("{id}.partial"));
        if git {
            self.git_base(base)?;
        }
        let strategy = cow::clone_tree(&base_dir, &tmp, Strategy::Auto)?;
        let tmp_has_git = tmp.join(".git/treeyard-ready").exists();
        let mut keys = Vec::new();
        for spec in &specs {
            let key = self.layer_key(&base_dir, spec)?;
            if !self.restore_layer(&tmp, spec, &key)? {
                // Without the layer the spare saves little; a create builds it instead.
                cow::remove_tree(&tmp)?;
                bail!("layer {} ({key}) is not saved yet: create one workspace with --setup first", spec.dir);
            }
            keys.push(json!([spec.dir, spec.keys, key]));
        }
        fs::rename(&tmp, root.join(&id))?;
        let git = tmp_has_git;
        let rec = json!({"id": id, "base": base, "strategy": strategy, "layers": keys, "git": git});
        write_json(&root.join(format!("{id}.json")), &rec)?;
        let mut rec = rec;
        rec["seconds"] = json!(round(t0.elapsed().as_secs_f64()));
        Ok(rec)
    }

    /// Spare records: (id, base, record).
    pub fn spares(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(self.home.join("spares")) else { return Ok(out) };
        for e in rd {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Ok(v) = read_json::<Value>(&p)
            {
                out.push(v);
            }
        }
        Ok(out)
    }

    /// Moves a matching spare to `dir` and returns its strategy.
    fn take_spare(&self, base: &str, keys: &[(LayerSpec, String)], git: bool, dir: &Path) -> Result<Option<Strategy>> {
        let want: Vec<Value> = keys.iter().map(|(s, k)| json!([s.dir, s.keys, k])).collect();
        let _l = lock(&self.home.join("node.lock"))?;
        for rec in self.spares()? {
            if rec["base"] != base || rec["layers"] != json!(want) || (git && rec["git"] != true) {
                continue;
            }
            let id = rec["id"].as_str().unwrap_or_default();
            let root = self.home.join("spares");
            fs::remove_file(root.join(format!("{id}.json")))?;
            if let Some(p) = dir.parent() {
                fs::create_dir_all(p)?;
            }
            fs::rename(root.join(id), dir)?;
            return Ok(Some(serde_json::from_value(rec["strategy"].clone())?));
        }
        Ok(None)
    }

    /// Keeps `count` spares for the current main and deletes spares for older bases.
    pub fn fill_spares(&self, count: usize, layers: Option<&[LayerSpec]>, git: bool) -> Result<Value> {
        let main = self.call("status", json!({}))?["main"].as_str().context("status reply")?.to_string();
        let mut removed = 0;
        let mut have = 0;
        for rec in self.spares()? {
            if rec["base"] == main.as_str() && (!git || rec["git"] == true) {
                have += 1;
                continue;
            }
            let id = rec["id"].as_str().unwrap_or_default().to_string();
            let root = self.home.join("spares");
            let _l = lock(&self.home.join("node.lock"))?;
            if fs::remove_file(root.join(format!("{id}.json"))).is_ok() {
                cow::remove_tree(&root.join(&id))?;
                removed += 1;
            }
        }
        let mut made = Vec::new();
        while have + made.len() < count {
            made.push(self.make_spare(&main, layers, git)?);
        }
        Ok(json!({"main": main, "kept": have, "made": made, "removed": removed}))
    }

    pub fn get(&self, name: &str) -> Result<Workspace> {
        read_json(&self.meta_path(name)).map_err(|_| anyhow!("no workspace {name:?} in {}", self.home.display()))
    }

    pub fn list(&self) -> Result<Vec<Workspace>> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(self.home.join("meta")) else { return Ok(out) };
        for e in rd {
            let p = e?.path();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.ends_with(".json") && !name.ends_with(".state.json") {
                out.push(read_json(&p)?);
            }
        }
        out.sort_by(|a: &Workspace, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Removes a workspace without losing work. Under the workspace lock it syncs first, so an
    /// edit the daemon has not sent yet still reaches the change; if that sync fails, nothing is
    /// deleted unless `force`. `abandon` drops the change instead (no sync then). The record is
    /// marked `removing` before the directory goes, so `finish_removes` can end a remove that a
    /// crash cut short. Only a directory under `<home>/ws` is ever deleted.
    pub fn remove(&self, name: &str, abandon: bool, force: bool) -> Result<Value> {
        let ws = self.get(name)?;
        if !ws.dir.starts_with(self.home.join("ws")) {
            bail!(
                "refusing to delete {}: a workspace lives under {}",
                ws.dir.display(),
                self.home.join("ws").display()
            );
        }
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        let mut synced = Value::Null;
        // A create cut short in `creating` has no edits yet (and maybe no stat cache to compare).
        let has_edits = ws.dir.exists() && ws.state != WsState::Creating && self.state_path(name).exists();
        if !abandon && has_edits {
            match self.sync_locked(name, &ws) {
                Ok(s) => synced = s,
                Err(e) if force => synced = json!({"error": format!("{e:#}")}),
                Err(e) => bail!(
                    "could not send the last edits of workspace {name:?} to its change ({e:#}); nothing was deleted. \
                     Retry when the server answers, or pass --force to delete the edits"
                ),
            }
        }
        // The repo's teardown (stop services, drop the database): a failure is reported, not fatal.
        let mut teardown = Value::Null;
        if let Some(cmd) = ws.teardown.as_deref().filter(|_| ws.dir.exists()) {
            teardown = match self.run_logged(&ws, "teardown", cmd) {
                Ok((ok, log)) => json!({"ok": ok, "log": log}),
                Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
            };
        }
        fs::write(self.removing_path(name), b"")?;
        let mut ws = ws;
        ws.state = WsState::Removing;
        write_json(&self.meta_path(name), &ws)?;
        self.delete_files(&ws)?;
        let mut abandoned = false;
        if abandon {
            let st = self.call("change", json!({"change": ws.change}))?;
            if st["state"] == "open" {
                self.call("abandon", json!({"change": ws.change}))?;
                abandoned = true;
            }
        }
        self.forget(name)?;
        Ok(
            json!({"removed": name, "change": ws.change, "abandoned": abandoned, "synced": synced, "teardown": teardown}),
        )
    }

    fn removing_path(&self, name: &str) -> PathBuf {
        self.home.join("meta").join(format!("{name}.removing"))
    }

    fn delete_files(&self, ws: &Workspace) -> Result<()> {
        if ws.strategy == Strategy::Overlay && ws.dir.exists() {
            cow::overlay_unmount(&ws.dir)
        } else {
            cow::remove_tree(&ws.dir)
        }
    }

    fn forget(&self, name: &str) -> Result<()> {
        let _ = fs::remove_file(self.state_path(name));
        let _ = fs::remove_file(self.meta_path(name));
        let _ = fs::remove_file(self.removing_path(name));
        Ok(())
    }

    /// Ends removes that a crash cut short (a `removing` mark is left); returns their names.
    pub fn finish_removes(&self) -> Result<Vec<String>> {
        let mut done = Vec::new();
        let Ok(rd) = fs::read_dir(self.home.join("meta")) else { return Ok(done) };
        for e in rd {
            let p = e?.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".removing")) else {
                continue;
            };
            let name = name.to_string();
            let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
            if let Ok(ws) = self.get(&name)
                && ws.dir.starts_with(self.home.join("ws"))
            {
                self.delete_files(&ws)?;
            }
            self.forget(&name)?;
            done.push(name);
        }
        Ok(done)
    }

    /// Sends every file that differs from the last sync as one new changeset of the change.
    pub fn sync(&self, name: &str) -> Result<Value> {
        let ws = self.get(name)?;
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        self.sync_locked(name, &ws)
    }

    /// `sync` for a caller that holds the workspace lock.
    fn sync_locked(&self, name: &str, ws: &Workspace) -> Result<Value> {
        let t0 = Instant::now();
        if ws.state == WsState::Creating {
            bail!("workspace {name:?} is still being created; sync it when `list` shows it ready");
        }
        let mut state: State = read_json(&self.state_path(name))?;
        let layer_dirs: BTreeSet<String> =
            ws.layers.iter().filter_map(|l| l["dir"].as_str().map(String::from)).collect();
        let found = scan_cone(&ws.dir, &state, &layer_dirs, &ws.paths)?;
        let mut edits = Vec::new();
        let mut next: Vec<(String, Option<FileState>)> = Vec::new();
        for (path, f) in found.diffs {
            match f {
                None => edits.push((json!({"path": path, "delete": true}), 0)),
                Some((f, data)) => {
                    let mut e = json!({"path": path, "content_b64": B64.encode(&data)});
                    if f.mode == MODE_LINK {
                        e["symlink"] = json!(true);
                    } else {
                        e["executable"] = json!(f.mode == MODE_EXEC);
                    }
                    edits.push((e, data.len()));
                    next.push((path, Some(f)));
                    continue;
                }
            }
            next.push((path, None));
        }
        let refreshed = found.touched.len();
        state.extend(found.touched);
        let mut head = Value::Null;
        let mut overlaps = Vec::new();
        let mut changesets = 0;
        let paths: Vec<String> = next.iter().map(|(p, _)| p.clone()).collect();
        let mut batch = Vec::new();
        let mut size = 0;
        let mut flush = |batch: &mut Vec<Value>| -> Result<()> {
            if batch.is_empty() {
                return Ok(());
            }
            let r = self.call("apply", json!({"change": ws.change, "edits": std::mem::take(batch)}))?;
            head = r["head"].clone();
            overlaps = r["overlaps"].as_array().cloned().unwrap_or_default();
            changesets += 1;
            Ok(())
        };
        for (e, n) in edits {
            if size + n > BATCH_BYTES && !batch.is_empty() {
                flush(&mut batch)?;
                size = 0;
            }
            size += n;
            batch.push(e);
        }
        flush(&mut batch)?;
        for (path, f) in next {
            match f {
                Some(f) => state.insert(path, f),
                None => state.remove(&path),
            };
        }
        if changesets > 0 || refreshed > 0 {
            write_json(&self.state_path(name), &state)?;
        }
        if changesets > 0
            && let Ok(mut rec) = self.get(name)
        {
            rec.synced = crate::repo::now();
            write_json(&self.meta_path(name), &rec)?;
        }
        if !overlaps.is_empty() {
            fs::write(ws.dir.join(PRIVATE).join("overlaps.json"), serde_json::to_vec_pretty(&overlaps)?)?;
        }
        let mut mirror = Value::Null;
        if let Some(into) = ws.mirror.as_deref().filter(|_| changesets > 0) {
            // A mirror that cannot be written never fails the sync: the edits are on the server.
            mirror = write_mirror(name, &ws.dir, &state, into).unwrap_or_else(|e| json!({"error": format!("{e:#}")}));
        }
        Ok(json!({
            "workspace": name, "change": ws.change, "paths": paths, "changesets": changesets, "head": head,
            "overlaps": overlaps, "seconds": round(t0.elapsed().as_secs_f64()), "mirror": mirror,
        }))
    }

    /// Syncs, restores the change to `to` on the server (`restore_change`), then rewrites only
    /// the files that differ from the restored tree. Other files keep their mtimes.
    pub fn restore(&self, name: &str, to: &str) -> Result<Value> {
        let ws = self.get(name)?;
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        let sync = self.sync_locked(name, &ws)?;
        let to: Value = to.parse::<u64>().map_or_else(|_| json!(to), |n| json!(n));
        let restored = self.call("restore_change", json!({"change": ws.change, "to": to}))?;
        let head = restored["change"]["head"].as_str().context("restore_change reply")?.to_string();
        let (written, gone) = self.write_head(name, &ws, &head)?;
        Ok(json!({
            "workspace": name, "change": ws.change, "head": head, "written": written, "deleted": gone,
            "sync": sync, "restore": restored,
        }))
    }

    /// Brings the workspace up to its change as the server holds it now: after a land that hit a
    /// conflict (the server rebased the change onto main: other agents' landed work plus conflict
    /// markers) or a continuous rebase. Syncs first, so a local edit is never overwritten.
    pub fn update(&self, name: &str) -> Result<Value> {
        let mut ws = self.get(name)?;
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        let sync = self.sync_locked(name, &ws)?;
        let ch = self.call("change", json!({"change": ws.change}))?;
        let head = ch["head"].as_str().context("change reply has no head")?.to_string();
        let (written, gone) = self.write_head(name, &ws, &head)?;
        if let Some(base) = ch["base"].as_str()
            && base != ws.base
        {
            ws.base = base.to_string();
            write_json(&self.meta_path(name), &ws)?;
        }
        let conflicts =
            ch["conflicts"].as_array().map(|c| c.iter().filter_map(|x| x["path"].as_str()).collect::<Vec<_>>());
        Ok(json!({
            "workspace": name, "change": ws.change, "head": head, "written": written, "deleted": gone,
            "conflicts": conflicts, "sync": sync,
        }))
    }

    /// Rewrites the files of the workspace that differ from the tree at `head` (and deletes files
    /// it lacks); other files keep their mtimes. The caller holds the workspace lock.
    fn write_head(&self, name: &str, ws: &Workspace, head: &str) -> Result<(Vec<String>, Vec<String>)> {
        let tree = self.call("tree", json!({"at": head}))?;
        let mut want: BTreeMap<String, (u32, String)> = BTreeMap::new();
        for row in tree["entries"].as_array().context("tree has no entries")? {
            let path = row[0].as_str().context("bad tree row")?.to_string();
            check_rel(&path)?;
            let mode = row[1].as_u64().context("bad tree row")? as u32;
            if mode != MODE_GITLINK {
                want.insert(path, (mode, row[2].as_str().context("bad tree row")?.to_string()));
            }
        }
        crate::safety::check_tree(want.keys().map(String::as_str))?;
        let mut state: State = read_json(&self.state_path(name))?;
        let same = |st: Option<&FileState>, w: &(u32, String)| st.is_some_and(|s| (s.mode, &s.blob) == (w.0, &w.1));
        let gone: Vec<String> = state.keys().filter(|p| !want.contains_key(*p)).cloned().collect();
        for p in &gone {
            let _ = fs::remove_file(ws.dir.join(p));
            state.remove(p);
        }
        prune_empty_dirs(&ws.dir, gone.iter())?;
        let todo: Vec<(String, (u32, String))> = want.into_iter().filter(|(p, w)| !same(state.get(p), w)).collect();
        let ids: Vec<String> = todo.iter().map(|(_, (_, b))| b.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        let blobs = self.fetch_blobs(&ids)?;
        for (path, (mode, blob)) in &todo {
            let p = crate::safety::dest(&ws.dir, path)?;
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent)?;
            }
            if fs::symlink_metadata(&p).is_ok() {
                fs::remove_file(&p)?;
            }
            if *mode == MODE_LINK {
                std::os::unix::fs::symlink(String::from_utf8_lossy(&blobs[blob]).into_owned(), &p)?;
            } else {
                fs::write(&p, &blobs[blob])?;
                let perm = if *mode == MODE_EXEC { 0o755 } else { 0o644 };
                fs::set_permissions(&p, fs::Permissions::from_mode(perm))?;
            }
            let md = fs::symlink_metadata(&p)?;
            state.insert(
                path.clone(),
                FileState { mode: *mode, blob: blob.clone(), size: md.len(), mtime_ns: mtime_ns(&md) },
            );
        }
        write_json(&self.state_path(name), &state)?;
        Ok((todo.into_iter().map(|(p, _)| p).collect(), gone))
    }

    /// Starts (`into` = a directory) or stops (`None`) the mirror of a workspace: after every sync
    /// the change's files are written into that one stable directory, for apps that need a fixed
    /// path. The directory must not be under `<home>/ws` and must be empty or an earlier mirror.
    pub fn mirror(&self, name: &str, into: Option<&Path>) -> Result<Value> {
        let mut ws = self.get(name)?;
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        let Some(into) = into else {
            let was = ws.mirror.take();
            write_json(&self.meta_path(name), &ws)?;
            return Ok(json!({"workspace": name, "mirror": null, "was": was}));
        };
        let dir = std::path::absolute(into)?;
        if dir.exists() {
            let empty = fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?.next().is_none();
            if !empty && !dir.join(MIRROR_MARK).exists() {
                bail!(
                    "{} is not empty and not a mirror (no {MIRROR_MARK}); give an empty or new directory",
                    dir.display()
                );
            }
        }
        fs::create_dir_all(&dir)?;
        let ws_root = self.home.join("ws");
        fs::create_dir_all(&ws_root)?;
        let (real, root) = (dir.canonicalize()?, ws_root.canonicalize()?);
        if real.starts_with(&root) || root.starts_with(&real) {
            bail!("a mirror must be outside {}: {} is not", root.display(), real.display());
        }
        if let Ok(mark) = read_json::<Value>(&real.join(MIRROR_MARK))
            && let Some(other) = mark["workspace"].as_str().filter(|o| *o != name)
            && self.get(other).is_ok_and(|w| w.mirror.as_deref() == Some(dir.as_path()))
        {
            bail!("{} is the mirror of workspace {other:?}; stop it there first (--off)", dir.display());
        }
        let state: State = read_json(&self.state_path(name))?;
        let out = write_mirror(name, &ws.dir, &state, &dir)?;
        ws.mirror = Some(dir);
        write_json(&self.meta_path(name), &ws)?;
        Ok(out)
    }

    /// Removes workspaces until the node is within `max` workspaces and `max_bytes`. Workspaces
    /// whose change landed or was abandoned (or whose create failed) go first, then the least
    /// recently synced. Never removed: one younger than `min_age`, one being created, set up or
    /// removed, and one with edits not synced yet (the same scan as `sync`). Removal goes through
    /// `remove`, which syncs first. One gc at a time per node (`gc.lock`).
    pub fn gc(&self, o: &GcOptions) -> Result<Value> {
        let lock_path = self.home.join("gc.lock");
        fs::create_dir_all(&self.home)?;
        let f = fs::File::options().create(true).truncate(false).write(true).open(&lock_path)?;
        // A child forked by another thread holds a copy of the lock until it execs, so retry a little.
        let t = Instant::now();
        while f.try_lock().is_err() {
            if t.elapsed() > std::time::Duration::from_secs(2) {
                bail!("another gc runs on {} ({} is locked)", self.home.display(), lock_path.display());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let all = self.list()?;
        let mut count = all.len();
        let mut bytes = 0u64;
        let mut skipped = Vec::new();
        let mut candidates = Vec::new();
        let now = crate::repo::now();
        for ws in all {
            let size = dir_bytes(&ws.dir);
            bytes += size;
            let why = match ws.state {
                WsState::Creating | WsState::SettingUp => Some("being created".to_string()),
                WsState::Removing => Some("being removed".to_string()),
                _ if now - ws.created < o.min_age.as_secs() as i64 => {
                    Some(format!("younger than {} minutes", o.min_age.as_secs() / 60))
                }
                _ if git_busy(&ws.dir).is_some() => git_busy(&ws.dir),
                _ => match self.unsynced(&ws) {
                    Ok(0) => None,
                    Ok(n) => Some(format!("{n} edited files not synced yet")),
                    Err(e) => Some(format!("cannot scan: {e:#}")),
                },
            };
            if let Some(why) = why {
                skipped.push(json!({"workspace": ws.name, "why": why}));
                continue;
            }
            let change = match self.call("change", json!({"change": ws.change})) {
                Ok(c) => c["state"].as_str().unwrap_or("unknown").to_string(),
                Err(_) => "unknown".to_string(),
            };
            let done = matches!(change.as_str(), "landed" | "abandoned") || ws.state == WsState::Failed;
            candidates.push((!done, ws.synced.max(ws.created), ws, size, change));
        }
        candidates.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
        let (mut removed, mut freed) = (Vec::new(), 0u64);
        for (_, _, ws, size, change) in candidates {
            let over = o.max.is_some_and(|m| count > m) || o.max_bytes.is_some_and(|m| bytes > m);
            if !over {
                break;
            }
            if !o.dry_run
                && let Err(e) = self.remove(&ws.name, false, false)
            {
                skipped.push(json!({"workspace": ws.name, "why": format!("remove failed: {e:#}")}));
                continue;
            }
            count -= 1;
            bytes = bytes.saturating_sub(size);
            freed += size;
            removed.push(json!({"workspace": ws.name, "change": change, "bytes": size}));
        }
        Ok(json!({
            "removed": removed, "skipped": skipped, "freed_bytes": freed, "workspaces": count, "bytes": bytes,
            "dry_run": o.dry_run,
        }))
    }

    /// How many files of a workspace differ from its last sync (0 when it has no stat cache).
    fn unsynced(&self, ws: &Workspace) -> Result<usize> {
        let Ok(state) = read_json::<State>(&self.state_path(&ws.name)) else { return Ok(0) };
        if !ws.dir.exists() {
            return Ok(0);
        }
        let layer_dirs: BTreeSet<String> =
            ws.layers.iter().filter_map(|l| l["dir"].as_str().map(String::from)).collect();
        Ok(scan_cone(&ws.dir, &state, &layer_dirs, &ws.paths)?.diffs.len())
    }

    /// `land_with` and up to 10 minutes for the tests of the landing mode.
    pub fn land(&self, name: &str) -> Result<Value> {
        self.land_with(name, Duration::from_secs(600))
    }

    /// Syncs, then lands the change on main.
    /// - A change that has already landed or was dropped (for example reverted by optimistic
    ///   landing) is followed by a new change on today's main first (`reopen`).
    /// - When the land hits a conflict, the workspace is updated to the rebased change, so the
    ///   agent sees the markers and the work that landed first.
    /// - In queue and optimistic mode it waits up to `wait` for the verdict of the tests
    ///   (`land_status`): passed, rejected or reverted, with the failing output.
    pub fn land_with(&self, name: &str, wait: Duration) -> Result<Value> {
        let ws = self.get(name)?;
        let before = self.call("land_status", json!({"change": ws.change}))?;
        let reopened = if before["state"] != "open" { self.reopen(name)? } else { Value::Null };
        let sync = self.sync(name)?;
        let ws = self.get(name)?;
        let land = self.call("land", json!({"change": ws.change}))?;
        let update = if land["status"] == "conflict" { self.update(name)? } else { Value::Null };
        let mut verdict = Value::Null;
        if matches!(land["status"].as_str(), Some("landed" | "queued" | "already_on_main")) && !wait.is_zero() {
            let t0 = Instant::now();
            loop {
                let st = self.call("land_status", json!({"change": ws.change}))?;
                let done = !matches!(st["verdict"].as_str(), Some("pending" | "queued"));
                if done || t0.elapsed() >= wait {
                    verdict = st;
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        Ok(json!({"sync": sync, "land": land, "update": update, "verdict": verdict, "reopened": reopened}))
    }

    /// Binds the workspace to a new change on today's main when its change has landed or was
    /// dropped. The sync state is reset to main's tree, so the next sync sends every file that
    /// differs from main: the work in the directory stays whole even after a revert.
    pub fn reopen(&self, name: &str) -> Result<Value> {
        let mut ws = self.get(name)?;
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        let old = self.call("land_status", json!({"change": ws.change}))?;
        if old["state"] == "open" {
            return Ok(json!({"reopened": false, "change": ws.change}));
        }
        let opened = self.call(
            "open_change",
            json!({"agent": ws.agent, "intent": format!("follow-up of {} ({})", ws.change, old["verdict"].as_str().unwrap_or("closed"))}),
        )?;
        let id = opened["id"].as_str().context("open_change reply has no id")?.to_string();
        let base = opened["base"].as_str().context("open_change reply has no base")?.to_string();
        let tree = self.call("tree", json!({"at": base}))?;
        let mut state = State::new();
        for row in tree["entries"].as_array().context("tree has no entries")? {
            let (Some(path), Some(mode), Some(blob)) = (row[0].as_str(), row[1].as_u64(), row[2].as_str()) else {
                bail!("bad tree row {row}");
            };
            // Size and mtime that never match make the next scan hash the file and compare it.
            state.insert(
                path.to_string(),
                FileState { mode: mode as u32, blob: blob.to_string(), size: u64::MAX, mtime_ns: 0 },
            );
        }
        let from = std::mem::replace(&mut ws.change, id.clone());
        ws.base = base;
        write_json(&self.state_path(name), &state)?;
        write_json(&self.meta_path(name), &ws)?;
        let sync = self.sync_locked(name, &ws)?;
        Ok(json!({"reopened": true, "change": id, "from": from, "was": old["verdict"], "sync": sync}))
    }
}

fn round(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

/// Writes the synced files of workspace `name` (`state`, bytes from `ws_dir`) into the mirror
/// `into`. Only files that differ from what the mirror wrote last time are written, so unchanged
/// files keep their mtimes; files the mirror wrote that the change no longer has are deleted.
/// Files the mirror never wrote (a local `.env`) are left alone.
fn write_mirror(name: &str, ws_dir: &Path, state: &State, into: &Path) -> Result<Value> {
    let mark = into.join(MIRROR_MARK);
    let old: BTreeMap<String, (u32, String)> = match read_json::<Value>(&mark) {
        Ok(v) => serde_json::from_value(v["files"].clone()).unwrap_or_default(),
        Err(_) => BTreeMap::new(),
    };
    let mut files = BTreeMap::new();
    let (mut written, mut kept) = (Vec::new(), 0);
    for (path, f) in state {
        check_rel(path)?;
        let dst = into.join(path);
        let now = (f.mode, f.blob.clone());
        if old.get(path) == Some(&now) && fs::symlink_metadata(&dst).is_ok() {
            kept += 1;
            files.insert(path.clone(), now);
            continue;
        }
        let dst = crate::safety::dest(into, path)?;
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        if fs::symlink_metadata(&dst).is_ok_and(|m| m.is_dir()) {
            cow::remove_tree(&dst)?;
        } else {
            let _ = fs::remove_file(&dst);
        }
        let src = ws_dir.join(path);
        if f.mode == MODE_LINK {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else {
            fs::copy(&src, &dst).with_context(|| format!("mirror {path}"))?;
            fs::set_permissions(&dst, fs::Permissions::from_mode(if f.mode == MODE_EXEC { 0o755 } else { 0o644 }))?;
        }
        written.push(path.clone());
        files.insert(path.clone(), now);
    }
    let gone: Vec<String> = old.keys().filter(|p| !files.contains_key(*p)).cloned().collect();
    for p in &gone {
        if check_rel(p).is_ok() {
            let _ = fs::remove_file(into.join(p));
        }
    }
    prune_empty_dirs(into, gone.iter())?;
    write_json(&mark, &json!({"workspace": name, "files": files}))?;
    Ok(json!({"into": into, "written": written, "deleted": gone, "kept": kept}))
}

/// Checks and normalises `--paths`: relative directories of the base, at most `MAX_SPARSE_PATHS`,
/// sorted, without one inside another.
fn sparse_paths(paths: &[String], base: &Path) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let p = p.trim().trim_matches('/');
        if p.is_empty() {
            continue;
        }
        check_rel(p).map_err(|_| anyhow!("bad --paths entry {p:?}: give relative directories like src/,docs/"))?;
        if !fs::symlink_metadata(base.join(p)).is_ok_and(|m| m.is_dir()) {
            bail!("--paths entry {p:?} is not a directory of the base tree");
        }
        out.push(p.to_string());
    }
    out.sort();
    out.dedup();
    let all = out.clone();
    out.retain(|p| !all.iter().any(|q| p.starts_with(&format!("{q}/"))));
    if out.len() > MAX_SPARSE_PATHS {
        bail!("--paths has {} directories; at most {MAX_SPARSE_PATHS}", out.len());
    }
    Ok(out)
}

/// Whether file `rel` is in the cone of `paths` (git's cone mode): every root file, every file
/// under a listed directory, and the files directly in a parent of a listed directory.
pub fn in_cone(paths: &[String], rel: &str) -> bool {
    match rel.rsplit_once('/') {
        None => true,
        Some(_) if paths.is_empty() || rel == REPO_CONFIG => true,
        Some((parent, _)) => paths
            .iter()
            .any(|p| p == parent || p.starts_with(&format!("{parent}/")) || parent.starts_with(&format!("{p}/"))),
    }
}

/// Whether directory `rel` holds files of the cone (it is a listed directory, inside one, or above one).
fn dir_in_cone(paths: &[String], rel: &str) -> bool {
    paths.is_empty()
        || rel == PRIVATE
        || paths.iter().any(|p| p == rel || p.starts_with(&format!("{rel}/")) || rel.starts_with(&format!("{p}/")))
}

/// Makes a sparse workspace: each listed directory is one clone, and the other files of the cone
/// (root files, files in parents of listed directories) are cloned one by one. With `git`, the
/// `.git` is cloned too and set to the same sparse cone, so `git status` shows no deletions.
fn clone_sparse(base: &Path, dir: &Path, paths: &[String], cone: &State, s: Strategy, git: bool) -> Result<Strategy> {
    fs::create_dir_all(dir)?;
    let mut used = s;
    let mut one = |rel: &str| -> Result<()> {
        let dst = crate::safety::dest(dir, rel)?;
        used = cow::clone_tree(&base.join(rel), &dst, if used == Strategy::Auto { s } else { used })?;
        Ok(())
    };
    for p in paths {
        one(p)?;
    }
    for rel in cone.keys().filter(|r| !paths.iter().any(|p| r.starts_with(&format!("{p}/")))) {
        one(rel)?;
    }
    if git && base.join(".git").is_dir() {
        cow::clone_tree(&base.join(".git"), &dir.join(".git"), Strategy::Auto)?;
        let mut args = vec!["sparse-checkout", "set", "--cone"];
        args.extend(paths.iter().map(String::as_str));
        let out = crate::git::cmd().arg("-C").arg(dir).args(&args).output().context("run git")?;
        if !out.status.success() {
            bail!("git sparse-checkout in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr).trim());
        }
    }
    Ok(used)
}

/// Writes the uncommitted edits of the git checkout `from` into the workspace `ws`: files that
/// differ from its HEAD (staged or not), deletions, and untracked files that are not ignored.
/// Files outside the cone of `paths` are skipped. Git runs as a polite probe (`git_probe`).
fn seed_from(from: &Path, ws: &Path, paths: &[String], tracked: &State) -> Result<Value> {
    let top = String::from_utf8(git_probe(from, &["rev-parse", "--show-toplevel"])?)?.trim().to_string();
    let top = PathBuf::from(top);
    let head = String::from_utf8(git_probe(&top, &["rev-parse", "HEAD"])?)?.trim().to_string();
    let mut todo: Vec<(String, bool)> = Vec::new();
    let diff = git_probe(&top, &["diff", "--name-status", "--no-renames", "-z", "HEAD"])?;
    let mut parts = diff.split(|b| *b == 0).filter(|p| !p.is_empty());
    while let (Some(st), Some(path)) = (parts.next(), parts.next()) {
        todo.push((String::from_utf8_lossy(path).into_owned(), st == b"D"));
    }
    let others = git_probe(&top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    todo.extend(
        others.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| (String::from_utf8_lossy(p).into_owned(), false)),
    );
    let (mut written, mut deleted, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
    for (rel, gone) in todo {
        let why = if check_rel(&rel).is_err() {
            Some("unsafe or private path")
        } else if !in_cone(paths, &rel) {
            Some("outside --paths")
        } else {
            None
        };
        if let Some(why) = why {
            skipped.push(json!({"path": rel, "why": why}));
            continue;
        }
        let dst = crate::safety::dest(ws, &rel)?;
        let src = top.join(&rel);
        let md = fs::symlink_metadata(&src);
        if gone || md.is_err() {
            if tracked.contains_key(&rel) {
                fs::remove_file(&dst)?;
                deleted.push(rel);
            }
            continue;
        }
        let md = md?;
        if md.is_dir() {
            skipped.push(json!({"path": rel, "why": "a directory (submodule)"}));
            continue;
        }
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        if fs::symlink_metadata(&dst).is_ok() {
            fs::remove_file(&dst)?;
        }
        if md.file_type().is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else {
            fs::copy(&src, &dst).with_context(|| format!("copy {rel} from {}", top.display()))?;
        }
        written.push(rel);
    }
    Ok(json!({"from": top, "head": head, "written": written, "deleted": deleted, "skipped": skipped}))
}

/// Files in a git dir that mean a git operation is in progress in that checkout.
const GIT_BUSY: &[&str] = &["MERGE_HEAD", "rebase-merge", "rebase-apply", "CHERRY_PICK_HEAD", "REVERT_HEAD"];
/// An `index.lock` younger than this belongs to a running git command; an older one is stale.
const INDEX_LOCK_FRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a sync should wait for git in checkout `dir` (a merge, rebase, cherry-pick or revert in
/// progress, or a fresh `index.lock`), or `None`. A sync in the middle of a rebase would send
/// half-applied files and conflict markers.
pub fn git_busy(dir: &Path) -> Option<String> {
    let dot = dir.join(".git");
    // A linked worktree has a `.git` file: `gitdir: <path>`.
    let gd = match fs::read_to_string(&dot) {
        Ok(t) => PathBuf::from(t.strip_prefix("gitdir:")?.trim()),
        Err(_) if dot.is_dir() => dot,
        Err(_) => return None,
    };
    if let Some(f) = GIT_BUSY.iter().find(|f| gd.join(f).exists()) {
        return Some(format!("git operation in progress ({f})"));
    }
    let age = fs::metadata(gd.join("index.lock")).and_then(|m| m.modified()).ok()?.elapsed().unwrap_or_default();
    (age < INDEX_LOCK_FRESH).then(|| "git holds index.lock".to_string())
}

/// Runs a read-only git command (a status probe) in checkout `dir`. `GIT_OPTIONAL_LOCKS=0` keeps
/// it from taking `index.lock` to refresh the index, so it never blocks the agent's own git.
pub fn git_probe(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out =
        crate::git::cmd().arg("-C").arg(dir).args(args).env("GIT_OPTIONAL_LOCKS", "0").output().context("run git")?;
    if !out.status.success() {
        bail!("git {} in {}: {}", args.join(" "), dir.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// Bytes on disk under `dir` (allocated blocks, not following symlinks; 0 when it is missing).
fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.path().symlink_metadata() else { continue };
            total += md.blocks() * 512;
            if md.is_dir() {
                stack.push(e.path());
            }
        }
    }
    total
}

/// `~/x` -> `$HOME/x`; other paths as they are.
fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => PathBuf::from(h).join(rest),
        _ => PathBuf::from(p),
    }
}

/// Whether the `.gitignore` files of `root` (at every level, deeper ones win) ignore file `rel`.
fn ignored(root: &Path, rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let full = root.join(rel);
    let mut dir = root.to_path_buf();
    let mut verdict = false;
    for (i, part) in parts.iter().enumerate() {
        let gi = dir.join(".gitignore");
        if gi.is_file() {
            let (g, _) = ignore::gitignore::Gitignore::new(&gi);
            let m = g.matched_path_or_any_parents(&full, false);
            if m.is_ignore() {
                verdict = true;
            } else if m.is_whitelist() {
                verdict = false;
            }
        }
        if i + 1 < parts.len() {
            dir = dir.join(part);
        }
    }
    verdict
}

/// Removes now-empty parent directories of deleted paths, deepest first.
fn prune_empty_dirs<'a>(root: &Path, deleted: impl Iterator<Item = &'a String>) -> Result<()> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for p in deleted {
        let mut cur = p.as_str();
        while let Some((parent, _)) = cur.rsplit_once('/') {
            dirs.insert(parent.to_string());
            cur = parent;
        }
    }
    for d in dirs.iter().rev() {
        let _ = fs::remove_dir(root.join(d));
    }
    Ok(())
}

/// Directories named `name` under `root`, not inside a tracked directory, not nested in each other.
fn find_dirs(root: &Path, name: &str, tracked: &State) -> Result<Vec<String>> {
    let tracked_dir = |rel: &str| {
        let prefix = format!("{rel}/");
        tracked.range(prefix.clone()..).next().is_some_and(|(k, _)| k.starts_with(&prefix))
    };
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = fs::read_dir(root.join(&rel)) else { continue };
        for e in rd {
            let e = e?;
            if !e.file_type()?.is_dir() {
                continue;
            }
            let fname = e.file_name().to_string_lossy().into_owned();
            if fname == ".git" || fname == PRIVATE {
                continue;
            }
            let child = if rel.is_empty() { fname.clone() } else { format!("{rel}/{fname}") };
            if fname == name && !tracked_dir(&child) {
                out.push(child);
            } else if tracked_dir(&child) {
                // Untracked trees (other than layers) hold no layers worth sharing.
                stack.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A changed file: its new state and its bytes.
pub type Found = (FileState, Vec<u8>);

/// What `scan` found.
#[derive(Default)]
pub struct Scan {
    /// Changed or new files with their state and bytes; deleted files as `None`.
    pub diffs: Vec<(String, Option<Found>)>,
    /// Files whose stat moved but whose content did not (only the stat cache changes).
    pub touched: Vec<(String, FileState)>,
}

/// Compares the workspace with `state`. Tracked files are always checked; untracked files only
/// when no `.gitignore` excludes them (like git). Layer directories and `.treeyard` are skipped.
pub fn scan(root: &Path, state: &State, layer_dirs: &BTreeSet<String>) -> Result<Scan> {
    scan_cone(root, state, layer_dirs, &[])
}

/// `scan` of a sparse workspace: new files count only inside the cone of `paths` (empty = all).
/// Paths outside it are not in the workspace, so they are neither edits nor deletions.
pub fn scan_cone(root: &Path, state: &State, layer_dirs: &BTreeSet<String>, paths: &[String]) -> Result<Scan> {
    let mut out = Scan::default();
    for (path, st) in state {
        let p = root.join(path);
        let Ok(md) = fs::symlink_metadata(&p) else {
            out.diffs.push((path.clone(), None));
            continue;
        };
        if md.is_dir() {
            out.diffs.push((path.clone(), None));
            continue;
        }
        if md.len() == st.size && mtime_ns(&md) == st.mtime_ns && disk_mode(&md) == st.mode {
            continue;
        }
        let (mode, blob, data) = hash_file(&p, &md)?;
        let now = FileState { mode, blob, size: md.len(), mtime_ns: mtime_ns(&md) };
        if now.mode != st.mode || now.blob != st.blob {
            out.diffs.push((path.clone(), Some((now, data))));
        } else {
            out.touched.push((path.clone(), now));
        }
    }
    let skip = layer_dirs.clone();
    let (cone, base) = (paths.to_vec(), root.to_path_buf());
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .filter_entry(move |e| {
            let n = e.file_name().to_string_lossy();
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            if e.depth() > 0 && is_dir && (n == ".git" || n == PRIVATE || skip.contains(n.as_ref())) {
                return false;
            }
            let rel = e.path().strip_prefix(&base).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
            e.depth() == 0 || if is_dir { dir_in_cone(&cone, &rel) } else { in_cone(&cone, &rel) }
        })
        .build();
    for e in walker {
        let e = e?;
        if e.depth() == 0 || e.file_type().is_some_and(|t| t.is_dir()) {
            continue;
        }
        let rel = e.path().strip_prefix(root)?.to_string_lossy().into_owned();
        if state.contains_key(&rel) {
            continue;
        }
        let md = fs::symlink_metadata(e.path())?;
        let (mode, blob, data) = hash_file(e.path(), &md)?;
        out.diffs.push((rel, Some((FileState { mode, blob, size: md.len(), mtime_ns: mtime_ns(&md) }, data))));
    }
    // The walk skips `.treeyard/`, but the repo's workspace config there is a repo file.
    let cfg = root.join(REPO_CONFIG);
    if !state.contains_key(REPO_CONFIG)
        && let Ok(md) = fs::symlink_metadata(&cfg)
        && md.is_file()
    {
        let (mode, blob, data) = hash_file(&cfg, &md)?;
        let f = FileState { mode, blob, size: md.len(), mtime_ns: mtime_ns(&md) };
        out.diffs.push((REPO_CONFIG.to_string(), Some((f, data))));
    }
    out.diffs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_specs_parse_and_auto_detect() {
        let l: LayerSpec = "node_modules=pnpm-lock.yaml".parse().unwrap();
        assert_eq!(l, LayerSpec { dir: "node_modules".into(), keys: vec!["pnpm-lock.yaml".into()] });
        assert!("a/b=x".parse::<LayerSpec>().is_err());
        assert!("target=".parse::<LayerSpec>().is_err());
        let paths: BTreeSet<String> = ["Cargo.lock", "requirements.txt", "requirements-dev.txt", "src/main.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let auto = auto_layers(&paths);
        assert_eq!(auto.len(), 2);
        assert_eq!(auto[0].dir, "target");
        assert_eq!(auto[1].keys, vec!["requirements-dev.txt".to_string()]);
    }

    #[test]
    fn unsafe_server_paths_are_refused() {
        for p in ["../x", "/etc/passwd", "a/../../b", ".git/config", "a/.treeyard/env", "a//b"] {
            assert!(check_rel(p).is_err(), "{p}");
        }
        check_rel("src/main.rs").unwrap();
    }

    #[test]
    fn git_busy_sees_operations_and_fresh_index_locks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(git_busy(dir), None, "no .git");
        fs::create_dir_all(dir.join(".git/rebase-merge")).unwrap();
        assert!(git_busy(dir).unwrap().contains("rebase-merge"));
        fs::remove_dir(dir.join(".git/rebase-merge")).unwrap();
        let lock = dir.join(".git/index.lock");
        fs::write(&lock, "").unwrap();
        assert!(git_busy(dir).unwrap().contains("index.lock"));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        fs::File::options().write(true).open(&lock).unwrap().set_modified(old).unwrap();
        assert_eq!(git_busy(dir), None, "a stale index.lock does not block");
        // A linked worktree points at its git dir with a .git file.
        let wt = dir.join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", dir.join(".git").display())).unwrap();
        fs::write(dir.join(".git/MERGE_HEAD"), "x").unwrap();
        assert!(git_busy(&wt).unwrap().contains("MERGE_HEAD"));
    }

    #[test]
    fn sparse_cone_is_git_cone_mode() {
        let paths = vec!["a".to_string(), "p/q".to_string()];
        for (rel, want) in [
            ("README.md", true),
            ("a/x", true),
            ("a/b/c", true),
            ("p/top", true),
            ("p/q/x", true),
            ("p/other/x", false),
            ("b/x", false),
            ("ab/x", false),
            (REPO_CONFIG, true),
        ] {
            assert_eq!(in_cone(&paths, rel), want, "{rel}");
        }
        assert!(dir_in_cone(&paths, "p") && dir_in_cone(&paths, "a/b") && !dir_in_cone(&paths, "p/other"));
        assert!(in_cone(&[], "any/path"));
    }

    #[test]
    fn find_dirs_skips_tracked_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for d in ["target/debug", "crates/x/target", "src/target", "node_modules/a"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let mut st = State::new();
        let f = FileState { mode: MODE_FILE, blob: String::new(), size: 0, mtime_ns: 0 };
        st.insert("src/target/keep.rs".into(), f.clone());
        st.insert("crates/x/lib.rs".into(), f);
        assert_eq!(find_dirs(root, "target", &st).unwrap(), vec!["crates/x/target", "target"]);
    }
}
