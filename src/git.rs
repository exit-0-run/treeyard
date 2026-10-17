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

struct CatFile {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl CatFile {
    fn spawn(git_dir: &Path) -> Result<Self> {
        let mut child = Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(["cat-file", "--batch"])
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

/// The export repo's object database. New objects are written loose; existing ones (packed
/// by the import clone) are known by id and never rewritten.
pub struct Odb {
    dir: PathBuf,
    known: HashSet<String>,
    cat: CatFile,
}

impl Odb {
    pub fn open(dir: &Path) -> Result<Self> {
        let list = run(Some(dir), &["cat-file", "--batch-all-objects", "--batch-check=%(objectname)", "--unordered"])?;
        Ok(Self {
            dir: dir.to_path_buf(),
            known: list.lines().map(str::to_string).collect(),
            cat: CatFile::spawn(dir)?,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn has(&self, id: &str) -> bool {
        self.known.contains(id)
    }

    pub fn read(&mut self, id: &str) -> Result<Option<(String, Vec<u8>)>> {
        self.cat.read(id)
    }

    pub fn write(&mut self, kind: &str, data: &[u8]) -> Result<String> {
        let id = object_id(kind, data);
        if self.known.contains(&id) {
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
            fs::rename(&tmp, &path)?;
        }
        self.known.insert(id.clone());
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
