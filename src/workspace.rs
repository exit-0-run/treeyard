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
//! ```

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
use std::time::Instant;

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
}

/// One node: a home directory and the server it talks to.
pub struct Node {
    pub home: PathBuf,
    pub url: String,
    pub port_base: u16,
    pub port_block: u16,
}

fn check_rel(path: &str) -> Result<()> {
    let bad = crate::safety::check_path(path).is_err() || path.split('/').any(|c| c == PRIVATE);
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

    fn free_slot(&self) -> Result<u16> {
        let used: BTreeSet<u16> = self.list()?.iter().map(|w| w.slot).collect();
        let max_slot = (u16::MAX - self.port_base) / self.port_block;
        for slot in 0..max_slot {
            if used.contains(&slot) {
                continue;
            }
            // Skip a block whose first port another process already holds.
            let port = self.port_base + slot * self.port_block;
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return Ok(slot);
            }
        }
        bail!("no free port block")
    }

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
        let (base_dir, state) = self.base(&base)?;
        if o.git {
            self.git_base(&base)?;
        }
        let t_base = t0.elapsed().as_secs_f64();
        let specs = match &o.layers {
            Some(l) => l.clone(),
            None => auto_layers(&state.keys().cloned().collect()),
        };
        let keys = specs.iter().map(|s| Ok((s.clone(), self.layer_key(&base_dir, s)?))).collect::<Result<Vec<_>>>()?;
        let spare = if matches!(o.strategy, Strategy::Auto | Strategy::Clone) {
            self.take_spare(&base, &keys, o.git, &dir)?
        } else {
            None
        };
        let strategy = if let Some(s) = spare {
            s
        } else if o.strategy == Strategy::Overlay {
            cow::overlay_mount(&base_dir, &dir)?;
            Strategy::Overlay
        } else {
            if o.strategy == Strategy::Hardlink {
                cow::make_read_only(&base_dir)?;
            }
            cow::clone_tree(&base_dir, &dir, o.strategy)?
        };
        let t_clone = t0.elapsed().as_secs_f64() - t_base;
        let mut ws = {
            let _l = lock(&self.home.join("node.lock"))?;
            let slot = self.free_slot()?;
            let lo = self.port_base + slot * self.port_block;
            let ws = Workspace {
                name: o.name.clone(),
                url: self.url.clone(),
                agent: o.agent.clone(),
                change: change.clone(),
                base: base.clone(),
                dir: dir.clone(),
                strategy,
                slot,
                ports: (lo, lo + self.port_block - 1),
                layers: vec![],
                created: crate::repo::now(),
            };
            fs::create_dir_all(self.home.join("meta"))?;
            write_json(&self.state_path(&o.name), &state)?;
            write_json(&self.meta_path(&o.name), &ws)?;
            ws
        };
        fs::create_dir_all(dir.join(PRIVATE))?;
        let env: String = ws.env().iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        fs::write(dir.join(PRIVATE).join("env"), env)?;

        // Dependency layers: restore what exists (a spare has them), run the setup, save what was missing.
        let mut layers = Vec::new();
        for (spec, key) in keys {
            // A saved layer is immutable (written to a temp dir, then renamed), so no lock.
            let restored = spare.is_some() || self.restore_layer(&dir, &spec, &key)?;
            layers.push((spec, key, restored));
        }
        let t_layers = t0.elapsed().as_secs_f64() - t_base - t_clone;
        let mut setup_s = 0.0;
        if let Some(cmd) = &o.setup {
            let t = Instant::now();
            let out = std::process::Command::new("sh")
                .args(["-c", cmd])
                .current_dir(&dir)
                .envs(ws.env())
                .output()
                .context("run setup")?;
            setup_s = t.elapsed().as_secs_f64();
            if !out.status.success() {
                bail!(
                    "setup {cmd:?} failed in {}: {}",
                    dir.display(),
                    String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")
                );
            }
        }
        for (spec, key, restored) in &layers {
            let paths = if *restored || o.setup.is_none() {
                vec![]
            } else {
                let _l = lock(&self.home.join("node.lock"))?;
                self.save_layer(&dir, spec, key, &state)?
            };
            ws.layers
                .push(json!({"dir": spec.dir, "keys": spec.keys, "key": key, "restored": restored, "saved": paths}));
        }
        write_json(&self.meta_path(&o.name), &ws)?;
        let timing = json!({
            "base_s": round(t_base), "clone_s": round(t_clone), "layers_s": round(t_layers), "setup_s": round(setup_s),
            "total_s": round(t0.elapsed().as_secs_f64()), "spare": spare.is_some(),
        });
        Ok((ws, timing))
    }

    // ---- spares: a warm pool of ready directories ---------------------------------------

    /// Makes one spare workspace directory for base changeset `base`: a clone of the base with
    /// every layer that already exists restored. `create` takes it with one rename.
    pub fn make_spare(&self, base: &str, layers: Option<&[LayerSpec]>, git: bool) -> Result<Value> {
        let t0 = Instant::now();
        let (base_dir, state) = self.base(base)?;
        let specs = match layers {
            Some(l) => l.to_vec(),
            None => auto_layers(&state.keys().cloned().collect()),
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

    /// Removes the workspace directory and its record; `abandon` also drops the change.
    pub fn remove(&self, name: &str, abandon: bool) -> Result<Value> {
        let ws = self.get(name)?;
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        if ws.strategy == Strategy::Overlay {
            cow::overlay_unmount(&ws.dir)?;
        } else {
            cow::remove_tree(&ws.dir)?;
        }
        let mut abandoned = false;
        if abandon {
            let st = self.call("change", json!({"change": ws.change}))?;
            if st["state"] == "open" {
                self.call("abandon", json!({"change": ws.change}))?;
                abandoned = true;
            }
        }
        let _ = fs::remove_file(self.state_path(name));
        fs::remove_file(self.meta_path(name))?;
        Ok(json!({"removed": name, "change": ws.change, "abandoned": abandoned}))
    }

    /// Sends every file that differs from the last sync as one new changeset of the change.
    pub fn sync(&self, name: &str) -> Result<Value> {
        let t0 = Instant::now();
        let ws = self.get(name)?;
        let _l = lock(&self.home.join("meta").join(format!("{name}.lock")))?;
        let mut state: State = read_json(&self.state_path(name))?;
        let layer_dirs: BTreeSet<String> =
            ws.layers.iter().filter_map(|l| l["dir"].as_str().map(String::from)).collect();
        let found = scan(&ws.dir, &state, &layer_dirs)?;
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
        if !overlaps.is_empty() {
            fs::write(ws.dir.join(PRIVATE).join("overlaps.json"), serde_json::to_vec_pretty(&overlaps)?)?;
        }
        Ok(json!({
            "workspace": name, "change": ws.change, "paths": paths, "changesets": changesets, "head": head,
            "overlaps": overlaps, "seconds": round(t0.elapsed().as_secs_f64()),
        }))
    }

    /// Syncs, then lands the change on main.
    pub fn land(&self, name: &str) -> Result<Value> {
        let sync = self.sync(name)?;
        let ws = self.get(name)?;
        let land = self.call("land", json!({"change": ws.change}))?;
        Ok(json!({"sync": sync, "land": land}))
    }
}

fn round(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
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
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .filter_entry(move |e| {
            let n = e.file_name().to_string_lossy();
            !(e.depth() > 0
                && e.file_type().is_some_and(|t| t.is_dir())
                && (n == ".git" || n == PRIVATE || skip.contains(n.as_ref())))
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
