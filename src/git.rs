//! Git bridge: object hashing, loose-object writes into the export repo, tree and commit
//! encoding, and reads through one long-lived `git cat-file --batch`.

use anyhow::{Context, Result, bail};
use flate2::{Compression, write::ZlibEncoder};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::model::{Entry, Manifest};

/// The git object id of `data` stored as an object of `kind` ("blob", "tree", "commit").
pub fn object_id(kind: &str, data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("{kind} {}\0", data.len()).as_bytes());
    h.update(data);
    hex::encode(h.finalize())
}

/// Runs git (against `git_dir` when given) and returns stdout.
pub fn run(git_dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(d) = git_dir {
        cmd.arg("--git-dir").arg(d);
    }
    let out = cmd.args(args).stdin(Stdio::null()).output().context("running git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// Every file of `commit`, as (path, entry). Submodules come back with mode 160000.
pub fn ls_tree(git_dir: &Path, commit: &str) -> Result<Vec<(String, Entry)>> {
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(["ls-tree", "-r", "-z", "--full-tree", commit])
        .output()?;
    if !out.status.success() {
        bail!("git ls-tree {commit} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut entries = Vec::new();
    for rec in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let rec = std::str::from_utf8(rec).context("non UTF-8 path in tree")?;
        let (meta, path) = rec.split_once('\t').context("bad ls-tree line")?;
        let mut parts = meta.split(' ');
        let (mode, _kind, id) = (parts.next(), parts.next(), parts.next());
        let (Some(mode), Some(id)) = (mode, id) else { bail!("bad ls-tree line: {rec}") };
        entries.push((path.to_string(), Entry { blob: id.to_string(), mode: u32::from_str_radix(mode, 8)? }));
    }
    Ok(entries)
}

/// Paths that differ between two commits (no rename detection), with the new entry or `None`.
pub fn diff_tree(git_dir: &Path, from: &str, to: &str) -> Result<Vec<(String, Option<Entry>)>> {
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(["diff-tree", "-r", "-z", "--no-renames", "--no-commit-id", from, to])
        .output()?;
    if !out.status.success() {
        bail!("git diff-tree {from} {to} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_raw(&out.stdout)
}

/// Parses `--raw -z` records (":oldmode newmode oldid newid status\0path\0"), no renames.
pub fn parse_raw(raw: &[u8]) -> Result<Vec<(String, Option<Entry>)>> {
    let mut fields = raw.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut changes = Vec::new();
    while let Some(meta) = fields.next() {
        let meta = std::str::from_utf8(meta)?;
        let path = std::str::from_utf8(fields.next().context("diff-tree record without path")?)?;
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let [_, new_mode, _, new_id, status] = parts.as_slice() else { bail!("bad diff-tree record {meta}") };
        let entry = if status.starts_with('D') {
            None
        } else {
            Some(Entry { blob: new_id.to_string(), mode: u32::from_str_radix(new_mode, 8)? })
        };
        changes.push((path.to_string(), entry));
    }
    Ok(changes)
}

/// Whether `a` is an ancestor of (or equal to) `b`.
pub fn is_ancestor(git_dir: &Path, a: &str, b: &str) -> Result<bool> {
    let st = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(["merge-base", "--is-ancestor", a, b])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running git merge-base")?;
    match st.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base --is-ancestor {a} {b} failed"),
    }
}

/// Commits from just after `from` up to `to` (oldest first), each a child of the one before it,
/// all descendants of `from`. Prefers first parents; takes a later parent where the first
/// parent does not descend from `from` (a person merged main into a side branch).
pub fn ancestry_chain(git_dir: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    let out = run(Some(git_dir), &["rev-list", "--parents", "--ancestry-path", &format!("{from}..{to}")])?;
    let parents: std::collections::HashMap<&str, Vec<&str>> = out
        .lines()
        .filter_map(|l| {
            let mut it = l.split(' ');
            it.next().map(|c| (c, it.collect()))
        })
        .collect();
    let mut chain = Vec::new();
    let mut cur = to;
    while cur != from {
        let ps = parents.get(cur).with_context(|| format!("{cur} does not descend from {from}"))?;
        chain.push(cur.to_string());
        cur = ps
            .iter()
            .copied()
            .find(|p| *p == from || parents.contains_key(p))
            .with_context(|| format!("no parent of {cur} descends from {from}"))?;
    }
    chain.reverse();
    Ok(chain)
}

/// A pack stored by `index_pack`: its name (the hash in `pack-<hash>.pack`) and object ids.
pub struct StoredPack {
    pub name: String,
    pub ids: Vec<String>,
}

impl StoredPack {
    /// Deletes the pack files again (a rejected push must not leave its objects behind).
    pub fn remove(&self, git_dir: &Path) -> Result<()> {
        for ext in ["pack", "idx", "rev", "keep", "promisor"] {
            let p = git_dir.join("objects/pack").join(format!("pack-{}.{ext}", self.name));
            if p.exists() {
                fs::remove_file(&p)?;
            }
        }
        Ok(())
    }
}

/// Stores a pack (thin packs allowed) in the repo with `git index-pack` and returns its name
/// and the ids of its objects.
pub fn index_pack(git_dir: &Path, pack: &[u8]) -> Result<StoredPack> {
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(["index-pack", "--stdin", "--fix-thin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting git index-pack")?;
    let mut stdin = child.stdin.take().context("index-pack stdin")?;
    let data = pack.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&data));
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let line = String::from_utf8(out.stdout)?;
    let hash = line.split_whitespace().last().context("index-pack printed no pack name")?;
    let idx = fs::File::open(git_dir.join("objects/pack").join(format!("pack-{hash}.idx")))?;
    let out = Command::new("git").arg("--git-dir").arg(git_dir).arg("show-index").stdin(idx).output()?;
    if !out.status.success() {
        bail!("git show-index failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let ids = String::from_utf8(out.stdout)?.lines().filter_map(|l| l.split(' ').nth(1).map(str::to_string)).collect();
    Ok(StoredPack { name: hash.to_string(), ids })
}

struct CatFile {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl CatFile {
    fn spawn(git_dir: &Path) -> Result<Self> {
        Self::spawn_mode(git_dir, "--batch")
    }

    fn spawn_check(git_dir: &Path) -> Result<Self> {
        Self::spawn_mode(git_dir, "--batch-check=%(objectname)")
    }

    /// With `--batch-check=%(objectname)`: whether the object exists.
    fn exists(&mut self, id: &str) -> Result<bool> {
        writeln!(self.stdin, "{id}")?;
        self.stdin.flush()?;
        let mut line = String::new();
        self.stdout.read_line(&mut line)?;
        Ok(!line.trim_end().ends_with(" missing") && line.trim_end() == id)
    }

    fn spawn_mode(git_dir: &Path, mode: &str) -> Result<Self> {
        let mut child = Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(["cat-file", mode])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting git cat-file")?;
        let stdin = child.stdin.take().context("cat-file stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("cat-file stdout")?);
        Ok(Self { child, stdin, stdout })
    }

    fn read(&mut self, id: &str) -> Result<Option<(String, Vec<u8>)>> {
        writeln!(self.stdin, "{id}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header)?;
        let parts: Vec<&str> = header.trim_end().split(' ').collect();
        // An answer for another id means the stream lost its place: never return that data.
        if parts.first() != Some(&id) {
            bail!("cat-file answered {header:?} for {id}");
        }
        match parts.as_slice() {
            [_, "missing"] => Ok(None),
            [_, kind, size] => {
                let mut data = vec![0; size.parse()?];
                self.stdout.read_exact(&mut data)?;
                let mut newline = [0u8; 1];
                self.stdout.read_exact(&mut newline)?;
                Ok(Some((kind.to_string(), data)))
            }
            _ => bail!("unexpected cat-file header {header:?}"),
        }
    }
}

impl Drop for CatFile {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A 20-byte object id; the cache keeps raw ids (a hex `String` costs about four times more).
type RawId = [u8; 20];

fn raw_id(id: &str) -> Option<RawId> {
    let mut out = [0u8; 20];
    hex::decode_to_slice(id, &mut out).ok()?;
    Some(out)
}

/// The export repo's object database. New objects are written loose; existing ones (packed
/// by the import clone) are never rewritten. Whether an object exists is asked of git (one
/// `cat-file --batch-check` process) on the first look and then cached, so opening a repo
/// with millions of objects costs nothing.
pub struct Odb {
    dir: PathBuf,
    known: HashSet<RawId>,
    cat: CatFile,
    check: CatFile,
}

impl Odb {
    pub fn open(dir: &Path) -> Result<Self> {
        Ok(Self {
            dir: dir.to_path_buf(),
            known: HashSet::new(),
            cat: CatFile::spawn(dir)?,
            check: CatFile::spawn_check(dir)?,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn has(&mut self, id: &str) -> bool {
        let Some(raw) = raw_id(id) else { return false };
        if self.known.contains(&raw) {
            return true;
        }
        match self.check.exists(id) {
            Ok(true) => {
                self.known.insert(raw);
                true
            }
            _ => false,
        }
    }

    /// Records objects that arrived outside `write` (a pushed pack).
    pub fn learn(&mut self, ids: impl IntoIterator<Item = String>) {
        self.known.extend(ids.into_iter().filter_map(|i| raw_id(&i)));
    }

    /// Forgets objects whose pack was removed; restarts the readers, which keep packs open.
    pub fn unlearn(&mut self, ids: &[String]) -> Result<()> {
        for id in ids {
            if let Some(r) = raw_id(id) {
                self.known.remove(&r);
            }
        }
        self.cat = CatFile::spawn(&self.dir)?;
        self.check = CatFile::spawn_check(&self.dir)?;
        Ok(())
    }

    /// Objects in the existence cache (for memory checks).
    pub fn cached(&self) -> usize {
        self.known.len()
    }

    /// Reads one object. A miss or a broken answer restarts the readers and asks once more:
    /// a long-lived `cat-file` can lose its place or miss a pack that appeared after it started.
    pub fn read(&mut self, id: &str) -> Result<Option<(String, Vec<u8>)>> {
        if let Ok(Some(found)) = self.cat.read(id) {
            return Ok(Some(found));
        }
        self.cat = CatFile::spawn(&self.dir)?;
        self.check = CatFile::spawn_check(&self.dir)?;
        self.cat.read(id)
    }

    pub fn write(&mut self, kind: &str, data: &[u8]) -> Result<String> {
        let id = object_id(kind, data);
        if self.has(&id) {
            return Ok(id);
        }
        let dir = self.dir.join("objects").join(&id[..2]);
        fs::create_dir_all(&dir)?;
        let path = dir.join(&id[2..]);
        if !path.exists() {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(format!("{kind} {}\0", data.len()).as_bytes())?;
            enc.write_all(data)?;
            let tmp = dir.join(format!("tmp_obj_{}", &id[2..]));
            fs::write(&tmp, enc.finish()?)?;
            crate::failpoint::hit("odb:after-tmp");
            fs::rename(&tmp, &path)?;
        }
        self.known.extend(raw_id(&id));
        Ok(id)
    }

    /// Points a loose ref (e.g. `refs/heads/main`) at `id`; a loose ref wins over packed-refs.
    pub fn update_ref(&self, name: &str, id: &str) -> Result<()> {
        let path = self.dir.join(name);
        fs::create_dir_all(path.parent().context("ref has no parent")?)?;
        let lock = path.with_extension("lock");
        fs::write(&lock, format!("{id}\n"))?;
        fs::rename(&lock, &path)?;
        Ok(())
    }
}

enum Node {
    Leaf(Entry),
    Dir(BTreeMap<String, Node>),
}

/// Writes the trees of `manifest` and returns the root tree id.
pub fn write_tree(odb: &mut Odb, manifest: &Manifest) -> Result<String> {
    let mut root = BTreeMap::new();
    for (path, entry) in manifest.iter() {
        insert(&mut root, path, path, entry.clone())?;
    }
    write_dir(odb, &root)
}

fn insert(dir: &mut BTreeMap<String, Node>, full: &str, path: &str, entry: Entry) -> Result<()> {
    match path.split_once('/') {
        None => {
            dir.insert(path.to_string(), Node::Leaf(entry));
        }
        Some((head, rest)) => match dir.entry(head.to_string()).or_insert_with(|| Node::Dir(BTreeMap::new())) {
            Node::Dir(sub) => insert(sub, full, rest, entry)?,
            Node::Leaf(_) => bail!("{full} sits under a file"),
        },
    }
    Ok(())
}

fn write_dir(odb: &mut Odb, dir: &BTreeMap<String, Node>) -> Result<String> {
    // Git sorts tree entries by name, comparing a directory as if its name ended in '/'.
    let mut items = Vec::with_capacity(dir.len());
    for (name, node) in dir {
        let (key, mode, id) = match node {
            Node::Leaf(e) => (name.clone(), format!("{:o}", e.mode), e.blob.clone()),
            Node::Dir(sub) => (format!("{name}/"), "40000".to_string(), write_dir(odb, sub)?),
        };
        items.push((key, mode, name, id));
    }
    items.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut buf = Vec::new();
    for (_, mode, name, id) in items {
        buf.extend_from_slice(mode.as_bytes());
        buf.push(b' ');
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(&hex::decode(&id)?);
    }
    odb.write("tree", &buf)
}

/// One entry of a stored tree: the mode as git wrote it, and the object id.
struct TreeItem {
    mode: String,
    id: String,
}

impl TreeItem {
    fn is_tree(&self) -> bool {
        self.mode == "40000"
    }
}

fn read_tree(odb: &mut Odb, id: &str) -> Result<BTreeMap<String, TreeItem>> {
    let (kind, data) = odb.read(id)?.with_context(|| format!("missing tree {id}"))?;
    if kind != "tree" {
        bail!("{id} is a {kind}, not a tree");
    }
    let mut out = BTreeMap::new();
    let mut rest = &data[..];
    while !rest.is_empty() {
        let sp = rest.iter().position(|b| *b == b' ').context("bad tree entry")?;
        let nul = rest.iter().position(|b| *b == 0).context("bad tree entry")?;
        let mode = std::str::from_utf8(&rest[..sp])?.to_string();
        let name = std::str::from_utf8(&rest[sp + 1..nul]).context("non UTF-8 name in tree")?.to_string();
        let raw = rest.get(nul + 1..nul + 21).context("truncated tree entry")?;
        out.insert(name, TreeItem { mode, id: hex::encode(raw) });
        rest = &rest[nul + 21..];
    }
    Ok(out)
}

fn encode_tree(items: &BTreeMap<String, TreeItem>) -> Result<Vec<u8>> {
    let mut sorted: Vec<(String, &String, &TreeItem)> =
        items.iter().map(|(n, it)| (if it.is_tree() { format!("{n}/") } else { n.clone() }, n, it)).collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut buf = Vec::new();
    for (_, name, it) in sorted {
        buf.extend_from_slice(it.mode.as_bytes());
        buf.push(b' ');
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(&hex::decode(&it.id)?);
    }
    Ok(buf)
}

/// The root tree of `parent_tree` with `changes` (paths relative to the root) applied. Only
/// the directories on the path of a change are read and written again; every other subtree
/// keeps its id. For a tree that `write_tree` wrote, the result equals `write_tree` of the
/// changed manifest byte for byte.
pub fn write_tree_changed(
    odb: &mut Odb,
    parent_tree: &str,
    changes: &BTreeMap<String, Option<Entry>>,
) -> Result<String> {
    let rel: Vec<(&str, Option<&Entry>)> = changes.iter().map(|(p, e)| (p.as_str(), e.as_ref())).collect();
    match rewrite(odb, Some(parent_tree), &rel)? {
        Some(id) => Ok(id),
        None => odb.write("tree", &[]),
    }
}

/// Rewrites one directory; `None` when it ends up empty (git stores no empty trees).
fn rewrite(odb: &mut Odb, tree: Option<&str>, changes: &[(&str, Option<&Entry>)]) -> Result<Option<String>> {
    let mut items = match tree {
        Some(t) => read_tree(odb, t)?,
        None => BTreeMap::new(),
    };
    let mut nested: BTreeMap<&str, Vec<(&str, Option<&Entry>)>> = BTreeMap::new();
    for (path, e) in changes {
        match path.split_once('/') {
            None => match e {
                Some(e) => {
                    items.insert(path.to_string(), TreeItem { mode: format!("{:o}", e.mode), id: e.blob.clone() });
                }
                None => {
                    // A deleted file; a directory of the same name (file became dir) stays.
                    if items.get(*path).is_some_and(|it| !it.is_tree()) {
                        items.remove(*path);
                    }
                }
            },
            Some((head, rest)) => nested.entry(head).or_default().push((rest, *e)),
        }
    }
    for (head, sub) in nested {
        let old = items.get(head).filter(|it| it.is_tree()).map(|it| it.id.clone());
        let added = sub.iter().any(|(_, e)| e.is_some());
        if old.is_none() && !added {
            continue;
        }
        match rewrite(odb, old.as_deref(), &sub)? {
            Some(id) => {
                if items.get(head).is_some_and(|it| !it.is_tree()) && !sub.is_empty() {
                    // A file and a directory of one name: the manifest forbids it.
                    bail!("{head} is a file and a directory");
                }
                items.insert(head.to_string(), TreeItem { mode: "40000".into(), id });
            }
            None => {
                if items.get(head).is_some_and(|it| it.is_tree()) {
                    items.remove(head);
                }
            }
        }
    }
    if items.is_empty() {
        return Ok(None);
    }
    Ok(Some(odb.write("tree", &encode_tree(&items)?)?))
}

/// The tree id of a commit.
pub fn commit_tree(odb: &mut Odb, commit: &str) -> Result<String> {
    let (kind, data) = odb.read(commit)?.with_context(|| format!("missing commit {commit}"))?;
    if kind != "commit" {
        bail!("{commit} is a {kind}, not a commit");
    }
    let first = data.split(|b| *b == b'\n').next().unwrap_or_default();
    let tree = first.strip_prefix(b"tree ").with_context(|| format!("commit {commit} has no tree line"))?;
    Ok(std::str::from_utf8(tree)?.to_string())
}

pub struct Signature {
    pub name: String,
    pub email: String,
    pub time: i64,
}

pub fn commit_bytes(
    tree: &str,
    parents: &[String],
    author: &Signature,
    committer: &Signature,
    message: &str,
) -> Vec<u8> {
    let mut s = format!("tree {tree}\n");
    for p in parents {
        s.push_str(&format!("parent {p}\n"));
    }
    s.push_str(&format!("author {} <{}> {} +0000\n", author.name, author.email, author.time));
    s.push_str(&format!("committer {} <{}> {} +0000\n\n", committer.name, committer.email, committer.time));
    s.push_str(message);
    if !message.ends_with('\n') {
        s.push('\n');
    }
    s.into_bytes()
}
