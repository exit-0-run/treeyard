//! `treeyard connect`: attach TreeYard to the agent tools of one git repository, and undo it.
//!
//! Connect imports the repository (once), starts `serve` on loopback, writes a project-scoped MCP
//! entry for each harness (Claude Code `.mcp.json`, Codex `.codex/config.toml`, OpenCode
//! `opencode.json`) and a marked instruction block in AGENTS.md / CLAUDE.md. It never edits
//! user-level configuration. Its state lives outside the tree, in `<git dir>/treeyard/`:
//! `data/`, `connect.json` (pid, port, one undo record per touched file), `backup/`, `serve.log`.
//! `disconnect` restores every file it touched to the bytes it had before (or deletes the files
//! it created) and stops the server; the data directory stays unless `--purge`.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::git;

pub const MD_START: &str = "<!-- treeyard:connect:start -->";
pub const MD_END: &str = "<!-- treeyard:connect:end -->";
pub const HASH_START: &str = "# treeyard:connect:start";
pub const HASH_END: &str = "# treeyard:connect:end";
/// Seconds between two pushes of the mirror that `connect --mirror` starts.
pub const MIRROR_EVERY_S: u64 = 30;

/// The instruction block for AGENTS.md and CLAUDE.md (between the markers).
pub const INSTRUCTIONS: &str = "\
## TreeYard: other agents work on this repository at the same time

Use the tools of the `treeyard` MCP server for every change to this repository:

1. Call `open_change` with a one-line `intent` and the `paths` you plan to edit. Read `overlaps` and `advice` in the reply.
2. Edit files only through `apply` (or `replace` edits in `apply`). Do not write the files on disk.
3. When `apply` returns `collisions` of kind `overlap` or `conflict`, fix it at once: read the other change with `change`, adjust your edit, apply again.
4. Finish with `land`. On status `conflict`, write the final content of each task in `change.conflicts` with `apply` (or `resolve`), then `land` again.
";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    Opencode,
}

impl Harness {
    pub const ALL: [Harness; 3] = [Harness::Claude, Harness::Codex, Harness::Opencode];

    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Opencode => "opencode",
        }
    }

    /// The project-scoped config file, relative to the repository root.
    pub fn config_path(self) -> &'static str {
        match self {
            Harness::Claude => ".mcp.json",
            Harness::Codex => ".codex/config.toml",
            Harness::Opencode => "opencode.json",
        }
    }

    /// The instruction file this harness reads.
    fn instruction_file(self) -> &'static str {
        match self {
            Harness::Claude => "CLAUDE.md",
            Harness::Codex | Harness::Opencode => "AGENTS.md",
        }
    }

    pub fn parse_list(s: &str) -> Result<Vec<Harness>> {
        let mut out = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part {
                "all" => out.extend(Harness::ALL),
                "claude" | "claude-code" => out.push(Harness::Claude),
                "codex" => out.push(Harness::Codex),
                "opencode" => out.push(Harness::Opencode),
                other => bail!("unknown harness {other:?} (claude, codex, opencode or all)"),
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

/// What a touched file needs on undo.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A JSON MCP config (`.mcp.json` or `opencode.json`): key under `mcpServers` or `mcp`.
    Json,
    /// `.codex/config.toml`: a `#` marked block.
    Toml,
    /// AGENTS.md or CLAUDE.md: an HTML-comment marked block.
    Markdown,
    /// `.git/info/exclude`: a `#` marked block.
    Exclude,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileRecord {
    pub path: PathBuf,
    pub kind: Kind,
    /// Connect created the file (it did not exist before).
    pub created: bool,
    /// File name in `backup/` that holds the original bytes (pre-existing files only).
    pub backup: Option<String>,
    /// SHA-256 of the bytes connect wrote last.
    pub written: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MirrorState {
    pub pid: u32,
    pub remote: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct State {
    pub version: u32,
    pub repo: PathBuf,
    pub data: PathBuf,
    pub branch: String,
    pub pid: u32,
    pub port: u16,
    pub url: String,
    pub harnesses: Vec<Harness>,
    /// Agent names per harness (the MCP bridge takes the first free one).
    pub agents: BTreeMap<String, Vec<String>>,
    pub mirror: Option<MirrorState>,
    pub files: Vec<FileRecord>,
    /// Directories connect created (removed again when empty).
    pub dirs: Vec<PathBuf>,
}

pub struct Options {
    /// None: the harnesses whose binary is on PATH.
    pub harnesses: Option<Vec<Harness>>,
    pub agents: Option<usize>,
    pub port: Option<u16>,
    /// Some(true) = --mirror, Some(false) = --no-mirror, None = ask on a TTY.
    pub mirror: Option<bool>,
}

/// Where one repository's connect state lives.
pub struct Ctx {
    pub top: PathBuf,
    pub state_dir: PathBuf,
    pub exclude: PathBuf,
}

impl Ctx {
    pub fn find(dir: &Path) -> Result<Ctx> {
        let out = git::cmd()
            .args(["rev-parse", "--show-toplevel", "--absolute-git-dir", "--git-path", "info/exclude"])
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .context("running git")?;
        if !out.status.success() {
            bail!("{} is not inside a git repository (run connect in the repository directory)", dir.display());
        }
        let text = String::from_utf8(out.stdout)?;
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() < 3 {
            bail!("a bare repository has no working tree to connect");
        }
        let top = PathBuf::from(lines[0]);
        let git_dir = PathBuf::from(lines[1]);
        let exclude = PathBuf::from(lines[2]);
        let exclude = if exclude.is_absolute() { exclude } else { dir.join(exclude) };
        Ok(Ctx { top, state_dir: git_dir.join("treeyard"), exclude })
    }

    fn state_file(&self) -> PathBuf {
        self.state_dir.join("connect.json")
    }

    fn data(&self) -> PathBuf {
        self.state_dir.join("data")
    }

    pub fn load(&self) -> Result<Option<State>> {
        match fs::read(self.state_file()) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b).context("reading connect.json")?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, st: &State) -> Result<()> {
        fs::create_dir_all(&self.state_dir)?;
        let tmp = self.state_dir.join("connect.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(st)? + "\n")?;
        fs::rename(&tmp, self.state_file())?;
        Ok(())
    }
}

pub fn sha256(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn read_opt(p: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
    }
}

// ---------------------------------------------------------------- file edits (pure)

/// Puts `body` between `start` and `end` marker lines: replaces an existing block, or appends
/// one after a blank line.
pub fn block_add(existing: &[u8], start: &str, end: &str, body: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(existing).into_owned();
    let block = format!("{start}\n{body}{}{end}\n", if body.ends_with('\n') || body.is_empty() { "" } else { "\n" });
    if let (Some(a), Some(b)) = (text.find(start), text.find(end))
        && a < b
    {
        let mut e = b + end.len();
        if text[e..].starts_with('\n') {
            e += 1;
        }
        return format!("{}{block}{}", &text[..a], &text[e..]).into_bytes();
    }
    let mut out = text;
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&block);
    out.into_bytes()
}

/// Removes the marked block (and the blank line `block_add` put before it).
pub fn block_remove(existing: &[u8], start: &str, end: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(existing).into_owned();
    let (Some(a), Some(b)) = (text.find(start), text.find(end)) else {
        return existing.to_vec();
    };
    if a > b {
        return existing.to_vec();
    }
    let mut e = b + end.len();
    if text[e..].starts_with('\n') {
        e += 1;
    }
    let mut head = text[..a].to_string();
    if e == text.len() && head.ends_with("\n\n") {
        head.pop();
    }
    format!("{head}{}", &text[e..]).into_bytes()
}

fn json_parse(existing: Option<&[u8]>, path: &Path) -> Result<serde_json::Map<String, Value>> {
    match existing {
        None => Ok(serde_json::Map::new()),
        Some(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(serde_json::Map::new()),
        Some(b) => match serde_json::from_slice::<Value>(b) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => bail!("{} is not a JSON object; add the treeyard entry by hand (docs/mcp.md)", path.display()),
            Err(e) => bail!("{} is not plain JSON ({e}); add the treeyard entry by hand (docs/mcp.md)", path.display()),
        },
    }
}

/// Sets `root[key]["treeyard"] = entry` in a JSON config. `schema` is added to a new file.
pub fn json_add(
    existing: Option<&[u8]>,
    path: &Path,
    key: &str,
    entry: Value,
    schema: Option<&str>,
) -> Result<Vec<u8>> {
    let mut m = json_parse(existing, path)?;
    if m.is_empty()
        && let Some(s) = schema
    {
        m.insert("$schema".into(), json!(s));
    }
    let servers = m.entry(key.to_string()).or_insert_with(|| json!({}));
    let Value::Object(servers) = servers else {
        bail!("{}: `{key}` is not an object", path.display());
    };
    servers.insert("treeyard".into(), entry);
    Ok((serde_json::to_string_pretty(&Value::Object(m))? + "\n").into_bytes())
}

/// Removes `root[key]["treeyard"]` (and `key` when it is left empty).
pub fn json_remove(existing: &[u8], path: &Path, key: &str) -> Result<Vec<u8>> {
    let mut m = json_parse(Some(existing), path)?;
    if let Some(Value::Object(servers)) = m.get_mut(key) {
        servers.remove("treeyard");
        if servers.is_empty() {
            m.remove(key);
        }
    }
    Ok((serde_json::to_string_pretty(&Value::Object(m))? + "\n").into_bytes())
}

fn json_has_treeyard(existing: Option<&[u8]>, key: &str) -> bool {
    existing.and_then(|b| serde_json::from_slice::<Value>(b).ok()).is_some_and(|v| v[key].get("treeyard").is_some())
}

fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The Codex block: `[mcp_servers.treeyard]` with command, args and env.
pub fn codex_block(command: &str, args: &[String], url: &str) -> String {
    let args: Vec<String> = args.iter().map(|a| toml_str(a)).collect();
    format!(
        "[mcp_servers.treeyard]\ncommand = {}\nargs = [{}]\n\n[mcp_servers.treeyard.env]\nTREEYARD_URL = {}\n",
        toml_str(command),
        args.join(", "),
        toml_str(url)
    )
}

// ---------------------------------------------------------------- processes

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill with signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn cmdline(pid: u32) -> Option<String> {
    let out = Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()]).stdin(Stdio::null()).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// True when `pid` runs `treeyard <sub> ... <data> ...` (guards against a reused pid).
fn is_ours(pid: u32, sub: &str, data: &Path) -> bool {
    pid_alive(pid)
        && cmdline(pid).is_some_and(|c| c.contains(&format!(" {sub} ")) && c.contains(&*data.to_string_lossy()))
}

fn answers(url: &str) -> bool {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(3)).build();
    let mut req = agent.post(&format!("{url}/api/status"));
    if let Ok(t) = std::env::var("TREEYARD_TOKEN")
        && !t.is_empty()
    {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    req.send_json(json!({})).is_ok()
}

fn server_alive(st: &State) -> bool {
    is_ours(st.pid, "serve", &st.data) && answers(&st.url)
}

fn exe() -> Result<PathBuf> {
    std::env::current_exe().context("finding the treeyard binary")
}

/// `treeyard` when the one on PATH is this binary, else the absolute path of this binary.
fn command_name() -> Result<String> {
    let me = exe()?;
    let me_c = fs::canonicalize(&me).unwrap_or(me.clone());
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            let c = d.join("treeyard");
            if c.is_file() && fs::canonicalize(&c).ok().as_deref() == Some(me_c.as_path()) {
                return Ok("treeyard".into());
            }
        }
    }
    Ok(me.to_string_lossy().into_owned())
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
}

fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe; it only starts a new session for the child.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

fn log_file(ctx: &Ctx, name: &str) -> Result<fs::File> {
    fs::create_dir_all(&ctx.state_dir)?;
    Ok(fs::OpenOptions::new().create(true).append(true).open(ctx.state_dir.join(name))?)
}

fn tail(p: &Path) -> String {
    let s = fs::read_to_string(p).unwrap_or_default();
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// Starts `serve` detached on 127.0.0.1:port (0 = a free port); returns (pid, port).
fn start_server(ctx: &Ctx, port: u16) -> Result<(u32, u16)> {
    let port_file = ctx.state_dir.join("port");
    let _ = fs::remove_file(&port_file);
    let log = log_file(ctx, "serve.log")?;
    let mut cmd = Command::new(exe()?);
    cmd.arg("serve")
        .arg("--data")
        .arg(ctx.data())
        .arg("--addr")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--port-file")
        .arg(&port_file)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    detach(&mut cmd);
    let mut child = cmd.spawn().context("starting treeyard serve")?;
    let pid = child.id();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(60) {
        if let Some(status) = child.try_wait()? {
            bail!("treeyard serve exited ({status}):\n{}", tail(&ctx.state_dir.join("serve.log")));
        }
        if let Ok(s) = fs::read_to_string(&port_file)
            && let Ok(p) = s.trim().parse::<u16>()
            && answers(&format!("http://127.0.0.1:{p}"))
        {
            return Ok((pid, p));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: the pid is the child spawned above.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    bail!("treeyard serve did not answer within 60 s:\n{}", tail(&ctx.state_dir.join("serve.log")))
}

fn start_mirror(ctx: &Ctx, remote: &str) -> Result<u32> {
    let log = log_file(ctx, "mirror.log")?;
    let mut cmd = Command::new(exe()?);
    cmd.arg("mirror")
        .arg("--data")
        .arg(ctx.data())
        .arg("--remote")
        .arg(remote)
        .arg("--every")
        .arg(MIRROR_EVERY_S.to_string())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    detach(&mut cmd);
    Ok(cmd.spawn().context("starting treeyard mirror")?.id())
}

/// SIGTERM to a pid that still runs `treeyard <sub>` on this data; waits up to 5 s.
fn stop(pid: u32, sub: &str, data: &Path) -> bool {
    if !is_ours(pid, sub, data) {
        return false;
    }
    // SAFETY: the pid runs our own treeyard command (checked above).
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: as above.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    true
}

fn origin(top: &Path) -> Option<String> {
    let out = git::cmd()
        .args(["remote", "get-url", "origin"])
        .current_dir(top)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn branch(top: &Path) -> Result<String> {
    let out = git::cmd()
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(top)
        .stdin(Stdio::null())
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!("HEAD is detached: check out a branch first");
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

// ---------------------------------------------------------------- connect

/// Agent names `<harness>-1 .. <harness>-n`.
pub fn agent_names(h: Harness, n: usize) -> Vec<String> {
    (1..=n.max(1)).map(|i| format!("{}-{i}", h.name())).collect()
}

/// The `treeyard mcp` arguments of one harness.
fn mcp_args(names: &[String], pool_dir: &Path) -> Vec<String> {
    if names.len() == 1 {
        return vec!["mcp".into(), "--agent".into(), names[0].clone()];
    }
    vec!["mcp".into(), "--agent-pool".into(), names.join(","), "--pool-dir".into(), pool_dir.to_string_lossy().into()]
}

/// The JSON key, entry and new-file schema of a harness with a JSON config.
fn json_entry(h: Harness, command: &str, args: &[String], url: &str) -> (&'static str, Value, Option<&'static str>) {
    match h {
        Harness::Opencode => {
            let mut cmd = vec![command.to_string()];
            cmd.extend(args.iter().cloned());
            let entry = json!({"type": "local", "command": cmd, "enabled": true, "environment": {"TREEYARD_URL": url}});
            ("mcp", entry, Some("https://opencode.ai/config.json"))
        }
        _ => ("mcpServers", json!({"command": command, "args": args, "env": {"TREEYARD_URL": url}}), None),
    }
}

/// Refuses to take over an entry that someone else wrote.
fn check_foreign(kind: Kind, path: &Path, current: Option<&[u8]>, h: Option<Harness>) -> Result<()> {
    let taken = match (kind, h) {
        (Kind::Json, Some(Harness::Opencode)) => json_has_treeyard(current, "mcp"),
        (Kind::Json, _) => json_has_treeyard(current, "mcpServers"),
        (Kind::Toml, _) => current.is_some_and(|b| {
            let s = String::from_utf8_lossy(b);
            !s.contains(HASH_START) && s.contains("[mcp_servers.treeyard")
        }),
        _ => false,
    };
    if taken {
        bail!("{} already has a `treeyard` MCP server that connect did not write; remove it first", path.display());
    }
    Ok(())
}

/// Turns the current bytes of a file (None: missing) into the bytes connect wants there.
type Edit<'a> = dyn Fn(Option<&[u8]>) -> Result<Vec<u8>> + 'a;

struct Writer<'a> {
    ctx: &'a Ctx,
    st: &'a mut State,
    changed: Vec<String>,
}

impl Writer<'_> {
    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.ctx.top).map_or_else(|_| p.display().to_string(), |r| r.display().to_string())
    }

    fn mkdirs(&mut self, dir: &Path) -> Result<()> {
        let mut missing = Vec::new();
        let mut d = dir.to_path_buf();
        while !d.exists() {
            missing.push(d.clone());
            if !d.pop() {
                break;
            }
        }
        for m in missing.into_iter().rev() {
            fs::create_dir(&m).with_context(|| format!("creating {}", m.display()))?;
            self.st.dirs.push(m);
        }
        Ok(())
    }

    /// Writes `path` so it holds connect's entry; records how to undo it.
    fn put(&mut self, path: &Path, kind: Kind, make: &Edit<'_>) -> Result<()> {
        let current = read_opt(path)?;
        let idx = self.st.files.iter().position(|f| f.path == path);
        let want = make(current.as_deref())?;
        if current.as_deref() == Some(want.as_slice()) {
            return Ok(());
        }
        match idx {
            Some(i) => self.st.files[i].written = sha256(&want),
            None => {
                let backup = match &current {
                    Some(b) => {
                        let name = format!("{:03}-{}", self.st.files.len(), sha256(path.to_string_lossy().as_bytes()));
                        let dir = self.ctx.state_dir.join("backup");
                        fs::create_dir_all(&dir)?;
                        fs::write(dir.join(&name), b)?;
                        Some(name)
                    }
                    None => None,
                };
                self.st.files.push(FileRecord {
                    path: path.to_path_buf(),
                    kind,
                    created: current.is_none(),
                    backup,
                    written: sha256(&want),
                });
            }
        }
        if let Some(parent) = path.parent() {
            self.mkdirs(parent)?;
        }
        // Save the undo record before the write, so a crash leaves a state disconnect can undo.
        self.ctx.save(self.st)?;
        fs::write(path, &want).with_context(|| format!("writing {}", path.display()))?;
        self.changed.push(self.rel(path));
        Ok(())
    }
}

pub fn connect(dir: &Path, o: &Options) -> Result<()> {
    let ctx = Ctx::find(dir)?;
    let branch = branch(&ctx.top)?;
    let data = ctx.data();
    let mut notes = Vec::new();
    if !data.join("treeyard.db").exists() {
        fs::create_dir_all(&ctx.state_dir)?;
        let root = crate::repo::Repo::init(&data, &ctx.top, &branch, None)?;
        notes.push(format!("imported {branch} as changeset {}", &root[..12]));
    }
    let prev = ctx.load()?;
    let mut st = prev.clone().unwrap_or_else(|| State {
        version: 1,
        repo: ctx.top.clone(),
        data: data.clone(),
        branch: branch.clone(),
        pid: 0,
        port: 0,
        url: String::new(),
        harnesses: vec![],
        agents: BTreeMap::new(),
        mirror: None,
        files: vec![],
        dirs: vec![],
    });

    // Server.
    let mut restarted = false;
    if prev.is_none() || !server_alive(&st) || o.port.is_some_and(|p| p != st.port) {
        if prev.is_some() && is_ours(st.pid, "serve", &st.data) {
            stop(st.pid, "serve", &st.data);
        }
        let want = o.port.unwrap_or(st.port);
        let (pid, port) = match start_server(&ctx, want) {
            Ok(r) => r,
            Err(e) if o.port.is_none() && want != 0 => {
                notes.push(format!("port {want} is not free ({e:#}); took a free port"));
                start_server(&ctx, 0)?
            }
            Err(e) => return Err(e),
        };
        st.pid = pid;
        st.port = port;
        st.url = format!("http://127.0.0.1:{port}");
        restarted = true;
        ctx.save(&st)?;
    }

    // Harnesses and agent names.
    let mut hs = match &o.harnesses {
        Some(h) => h.clone(),
        None if prev.is_some() => st.harnesses.clone(),
        None => Harness::ALL.into_iter().filter(|h| on_path(h.name())).collect(),
    };
    hs.extend(st.harnesses.iter().copied());
    hs.sort();
    hs.dedup();
    if hs.is_empty() {
        notes.push("no harness found on PATH (claude, codex, opencode); pass --harness".into());
    }
    let n = o.agents.unwrap_or_else(|| st.agents.values().map(Vec::len).max().unwrap_or(1));
    st.harnesses = hs.clone();
    st.agents = hs.iter().map(|h| (h.name().to_string(), agent_names(*h, n))).collect();

    let command = command_name()?;
    let pool_dir = ctx.state_dir.join("agents");
    let url = st.url.clone();
    let mut w = Writer { ctx: &ctx, st: &mut st, changed: vec![] };
    let mut created_rel: Vec<String> = Vec::new();
    for h in &hs {
        let path = ctx.top.join(h.config_path());
        let names = agent_names(*h, n);
        let args = mcp_args(&names, &pool_dir);
        let kind = if *h == Harness::Codex { Kind::Toml } else { Kind::Json };
        let known = w.st.files.iter().any(|f| f.path == path);
        if !known {
            check_foreign(kind, &path, read_opt(&path)?.as_deref(), Some(*h))?;
        }
        if kind == Kind::Toml {
            let body = codex_block(&command, &args, &url);
            w.put(&path, kind, &|cur| Ok(block_add(cur.unwrap_or_default(), HASH_START, HASH_END, &body)))?;
        } else {
            let (key, entry, schema) = json_entry(*h, &command, &args, &url);
            w.put(&path, kind, &|cur| json_add(cur, &path, key, entry.clone(), schema))?;
        }
    }
    let mut md: Vec<&str> = vec!["AGENTS.md", "CLAUDE.md"];
    md.retain(|f| ctx.top.join(f).exists() || hs.iter().any(|h| h.instruction_file() == *f));
    for f in md {
        let path = ctx.top.join(f);
        w.put(&path, Kind::Markdown, &|cur| Ok(block_add(cur.unwrap_or_default(), MD_START, MD_END, INSTRUCTIONS)))?;
    }
    for f in &w.st.files {
        if f.created
            && f.kind != Kind::Exclude
            && let Ok(r) = f.path.strip_prefix(&ctx.top)
        {
            created_rel.push(format!("/{}", r.display()));
        }
    }
    if !created_rel.is_empty() {
        let lines = created_rel.join("\n") + "\n";
        let ex = ctx.exclude.clone();
        w.put(&ex, Kind::Exclude, &|cur| Ok(block_add(cur.unwrap_or_default(), HASH_START, HASH_END, &lines)))?;
    }
    let changed = std::mem::take(&mut w.changed);

    // Mirror.
    let mut mirror_note = None;
    let mirror_alive = st.mirror.as_ref().is_some_and(|m| is_ours(m.pid, "mirror", &st.data));
    let mut mirror_changed = false;
    if let Some(remote) = origin(&ctx.top) {
        let want = match o.mirror {
            Some(v) => v,
            None if mirror_alive => true,
            None if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() => {
                eprint!("push landed changes to origin ({remote}) every {MIRROR_EVERY_S} s? [y/N] ");
                std::io::stderr().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                matches!(line.trim(), "y" | "Y" | "yes")
            }
            None => {
                mirror_note = Some(format!(
                    "origin is {remote}; mirror is off. `treeyard connect --mirror` pushes landed changes to it."
                ));
                false
            }
        };
        if want && !mirror_alive {
            let pid = start_mirror(&ctx, &remote)?;
            st.mirror = Some(MirrorState { pid, remote });
            mirror_changed = true;
        } else if !want && let Some(m) = st.mirror.take() {
            stop(m.pid, "mirror", &st.data);
            mirror_changed = true;
        }
    } else if o.mirror == Some(true) {
        bail!("--mirror needs an `origin` remote");
    }

    if prev.is_some() && !restarted && changed.is_empty() && !mirror_changed {
        println!("already connected: {} (pid {})", st.url, st.pid);
        if let Some(n) = mirror_note {
            println!("{n}");
        }
        return Ok(());
    }
    ctx.save(&st)?;
    for n in &notes {
        println!("{n}");
    }
    let data_rel = data.strip_prefix(&ctx.top).unwrap_or(&data);
    println!("server: {} (pid {}, data {})", st.url, st.pid, data_rel.display());
    for h in &hs {
        println!("{}: {} (agents {})", h.name(), h.config_path(), st.agents[h.name()].join(", "));
    }
    if !changed.is_empty() {
        println!("wrote: {}", changed.join(", "));
    }
    if let Some(m) = &st.mirror {
        println!("mirror: pid {} pushes {} to {} every {MIRROR_EVERY_S} s", m.pid, st.branch, m.remote);
    }
    if let Some(n) = mirror_note {
        println!("{n}");
    }
    println!("landed work: git pull {}/git {}", st.url, st.branch);
    if hs.contains(&Harness::Claude) {
        println!("claude: approve the project server `treeyard` when Claude Code asks (or `/mcp`)");
    }
    if hs.contains(&Harness::Codex) {
        println!("codex: reads .codex/config.toml only in a trusted project");
    }
    println!("undo: treeyard disconnect");
    Ok(())
}

// ---------------------------------------------------------------- disconnect

fn remove_entry(f: &FileRecord, cur: &[u8]) -> Result<(Vec<u8>, bool)> {
    let out = match f.kind {
        Kind::Json => {
            let key = if f.path.file_name().is_some_and(|n| n == "opencode.json") { "mcp" } else { "mcpServers" };
            json_remove(cur, &f.path, key)?
        }
        Kind::Toml | Kind::Exclude => block_remove(cur, HASH_START, HASH_END),
        Kind::Markdown => block_remove(cur, MD_START, MD_END),
    };
    let empty = match f.kind {
        Kind::Json => serde_json::from_slice::<Value>(&out)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .is_some_and(|m| m.keys().all(|k| k == "$schema")),
        _ => out.iter().all(u8::is_ascii_whitespace),
    };
    Ok((out, empty))
}

pub fn disconnect(dir: &Path, purge: bool) -> Result<()> {
    let ctx = Ctx::find(dir)?;
    let Some(st) = ctx.load()? else {
        if purge && ctx.state_dir.exists() {
            fs::remove_dir_all(&ctx.state_dir)?;
            println!("not connected; removed the TreeYard data");
        } else {
            println!("not connected");
        }
        return Ok(());
    };
    let mut warnings = Vec::new();
    let rel = |p: &Path| p.strip_prefix(&ctx.top).map_or_else(|_| p.display().to_string(), |r| r.display().to_string());
    for f in st.files.iter().rev() {
        let cur = read_opt(&f.path)?;
        let original = match &f.backup {
            Some(name) => Some(fs::read(ctx.state_dir.join("backup").join(name)).context("reading a backup")?),
            None => None,
        };
        match cur {
            None => {
                if let Some(b) = original {
                    fs::write(&f.path, b)?;
                    warnings.push(format!("{} was deleted; restored the version from before connect", rel(&f.path)));
                }
            }
            Some(cur) if sha256(&cur) == f.written => match original {
                Some(b) => fs::write(&f.path, b)?,
                None => fs::remove_file(&f.path)?,
            },
            Some(cur) => {
                let (out, empty) = remove_entry(f, &cur)?;
                if f.created && empty {
                    fs::remove_file(&f.path)?;
                } else if out != cur {
                    fs::write(&f.path, &out)?;
                }
                warnings.push(format!("{} changed after connect; removed only the treeyard part", rel(&f.path)));
            }
        }
    }
    for d in st.dirs.iter().rev() {
        let _ = fs::remove_dir(d); // only when empty
    }
    if let Some(m) = &st.mirror
        && stop(m.pid, "mirror", &st.data)
    {
        println!("stopped mirror (pid {})", m.pid);
    }
    if stop(st.pid, "serve", &st.data) {
        println!("stopped server {} (pid {})", st.url, st.pid);
    } else {
        println!("server {} (pid {}) was not running", st.url, st.pid);
    }
    for w in &warnings {
        println!("warning: {w}");
    }
    let _ = fs::remove_dir_all(ctx.state_dir.join("backup"));
    let _ = fs::remove_dir_all(ctx.state_dir.join("agents"));
    let _ = fs::remove_file(ctx.state_dir.join("port"));
    fs::remove_file(ctx.state_file())?;
    if purge {
        fs::remove_dir_all(&ctx.state_dir)?;
        println!("removed the TreeYard data ({})", rel(&ctx.state_dir));
    } else {
        println!("kept the data in {} (`disconnect --purge` removes it)", rel(&st.data));
    }
    println!("disconnected: {} files restored", st.files.len());
    Ok(())
}

// ---------------------------------------------------------------- status

pub fn status_json(dir: &Path) -> Result<Value> {
    let ctx = Ctx::find(dir)?;
    let Some(st) = ctx.load()? else {
        return Ok(json!({"connected": false, "repo": ctx.top}));
    };
    let alive = server_alive(&st);
    let harnesses: Vec<Value> = st
        .harnesses
        .iter()
        .map(|h| {
            let p = ctx.top.join(h.config_path());
            let ok = st
                .files
                .iter()
                .any(|f| f.path == p && read_opt(&p).ok().flatten().is_some_and(|b| sha256(&b) == f.written));
            json!({"name": h.name(), "config": h.config_path(), "agents": st.agents.get(h.name()), "config_intact": ok})
        })
        .collect();
    let instructions: Vec<String> = st
        .files
        .iter()
        .filter(|f| f.kind == Kind::Markdown)
        .filter_map(|f| f.path.strip_prefix(&ctx.top).ok().map(|r| r.display().to_string()))
        .collect();
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    if alive {
        let list = crate::client::call(&st.url, "changes", json!({}))?;
        for c in list["changes"].as_array().cloned().unwrap_or_default() {
            let id = c["id"].as_str().unwrap_or_default();
            let full = crate::client::call(&st.url, "change", json!({"change": id}))?;
            for col in full["collisions"].as_array().cloned().unwrap_or_default() {
                for f in col["files"].as_array().cloned().unwrap_or_default() {
                    warnings.push(json!({
                        "change": id, "agent": c["agent"], "kind": f["kind"],
                        "path": f["path"].as_str().or(f["symbol"].as_str()),
                        "other": col["change"], "other_agent": col["agent"],
                    }));
                }
            }
            if full["conflicted"] == true {
                for t in full["conflicts"].as_array().cloned().unwrap_or_default() {
                    warnings
                        .push(json!({"change": id, "agent": c["agent"], "kind": "land_conflict", "path": t["path"]}));
                }
            }
            changes.push(json!({
                "id": id, "agent": c["agent"], "intent": c["intent"], "paths": full["paths"],
                "overlaps": full["overlaps"], "collisions": full["collisions"],
            }));
        }
    }
    Ok(json!({
        "connected": true,
        "repo": ctx.top,
        "url": st.url,
        "pid": st.pid,
        "server_alive": alive,
        "data": st.data,
        "branch": st.branch,
        "harnesses": harnesses,
        "instructions": instructions,
        "mirror": st.mirror.as_ref().map(|m| json!({"pid": m.pid, "remote": m.remote, "alive": is_ours(m.pid, "mirror", &st.data)})),
        "changes": changes,
        "warnings": warnings,
    }))
}

pub fn status_text(v: &Value) -> String {
    let mut out = String::new();
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    if v["connected"] != true {
        out.push_str("not connected (run `treeyard connect`)\n");
        return out;
    }
    let alive = if v["server_alive"] == true { "running" } else { "NOT RUNNING (run `treeyard connect` again)" };
    out.push_str(&format!("server: {} (pid {}, {alive})\n", s(&v["url"]), v["pid"]));
    for h in v["harnesses"].as_array().into_iter().flatten() {
        let agents: Vec<String> = h["agents"].as_array().into_iter().flatten().map(&s).collect();
        let intact = if h["config_intact"] == true { "" } else { " (edited after connect)" };
        out.push_str(&format!("{}: {}{intact}, agents {}\n", s(&h["name"]), s(&h["config"]), agents.join(", ")));
    }
    let ins: Vec<String> = v["instructions"].as_array().into_iter().flatten().map(&s).collect();
    out.push_str(&format!("instructions: {}\n", if ins.is_empty() { "none".into() } else { ins.join(", ") }));
    match v["mirror"].as_object() {
        Some(m) => out.push_str(&format!("mirror: pid {} to {}\n", m["pid"], s(&m["remote"]))),
        None => out.push_str("mirror: off\n"),
    }
    let changes = v["changes"].as_array().cloned().unwrap_or_default();
    out.push_str(&format!("open changes: {}\n", changes.len()));
    for c in &changes {
        let paths: Vec<String> = c["paths"].as_array().into_iter().flatten().map(&s).collect();
        out.push_str(&format!(
            "  {} {} \"{}\" paths: {}\n",
            s(&c["id"]),
            s(&c["agent"]),
            s(&c["intent"]),
            if paths.is_empty() { "-".into() } else { paths.join(", ") }
        ));
        for o in c["overlaps"].as_array().into_iter().flatten() {
            let p: Vec<String> = o["paths"].as_array().into_iter().flatten().map(&s).collect();
            out.push_str(&format!("    shares {} with {} ({})\n", p.join(", "), s(&o["change"]), s(&o["agent"])));
        }
    }
    let warnings = v["warnings"].as_array().cloned().unwrap_or_default();
    out.push_str(&format!("warnings: {}\n", warnings.len()));
    for w in &warnings {
        let other = if w["other"].is_null() {
            String::new()
        } else {
            format!(" with {} ({})", s(&w["other"]), s(&w["other_agent"]))
        };
        out.push_str(&format!(
            "  {}: {} {} in {}{other}\n",
            s(&w["kind"]),
            s(&w["agent"]),
            s(&w["change"]),
            s(&w["path"])
        ));
    }
    out
}

// ---------------------------------------------------------------- agent pool (MCP bridge)

/// Holds one agent name of a pool; the lock file goes away on drop.
pub struct PoolName {
    pub name: String,
    lock: Option<PathBuf>,
}

impl Drop for PoolName {
    fn drop(&mut self) {
        if let Some(l) = &self.lock {
            let _ = fs::remove_file(l);
        }
    }
}

/// Takes the first free name of `pool` with an O_EXCL lock file `<dir>/<name>.lock` that holds
/// the pid. A lock of a dead pid is free. With every name taken: `<first>-<pid>`.
pub fn claim_name(pool: &[String], dir: &Path) -> Result<PoolName> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let me = std::process::id();
    for name in pool {
        if name.is_empty() || name.contains('/') || name.contains("..") {
            bail!("bad agent name {name:?}");
        }
        let lock = dir.join(format!("{name}.lock"));
        for _ in 0..2 {
            match fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(mut f) => {
                    writeln!(f, "{me}")?;
                    return Ok(PoolName { name: name.clone(), lock: Some(lock) });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let owner = fs::read_to_string(&lock).ok().and_then(|s| s.trim().parse::<u32>().ok());
                    match owner {
                        Some(pid) if pid_alive(pid) => break,
                        // An empty file may be a claim still being written: leave it.
                        None if lock.metadata().is_ok_and(|m| {
                            m.modified().ok().and_then(|t| t.elapsed().ok()).is_none_or(|e| e < Duration::from_secs(5))
                        }) =>
                        {
                            break;
                        }
                        _ => {
                            let _ = fs::remove_file(&lock);
                        }
                    }
                }
                Err(e) => return Err(e).with_context(|| format!("creating {}", lock.display())),
            }
        }
    }
    let first = pool.first().map_or("agent", String::as_str);
    Ok(PoolName { name: format!("{first}-{me}"), lock: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_block_round_trip_keeps_bytes() {
        for orig in ["", "# Notes\n", "# Notes\nno newline at end", "# A\n\n\n"] {
            let added = block_add(orig.as_bytes(), MD_START, MD_END, INSTRUCTIONS);
            assert!(String::from_utf8_lossy(&added).starts_with(orig));
            let again = block_add(&added, MD_START, MD_END, INSTRUCTIONS);
            assert_eq!(added, again, "adding twice changes nothing");
            let removed = block_remove(&added, MD_START, MD_END);
            if orig.ends_with('\n') || orig.is_empty() {
                assert_eq!(removed, orig.as_bytes(), "{orig:?}");
            }
        }
    }

    #[test]
    fn json_entry_add_and_remove() {
        let p = Path::new(".mcp.json");
        let orig = br#"{"mcpServers": {"other": {"command": "x"}}}"#;
        let added = json_add(Some(orig), p, "mcpServers", json!({"command": "treeyard"}), None).unwrap();
        let v: Value = serde_json::from_slice(&added).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["treeyard"]["command"], "treeyard");
        let removed: Value = serde_json::from_slice(&json_remove(&added, p, "mcpServers").unwrap()).unwrap();
        assert_eq!(removed, serde_json::from_slice::<Value>(orig).unwrap());
        assert!(json_add(Some(b"// comment\n{}"), p, "mcpServers", json!({}), None).is_err());
    }

    #[test]
    fn codex_block_is_valid_toml() {
        let b = codex_block("treeyard", &["mcp".into(), "--agent".into(), "codex-1".into()], "http://127.0.0.1:1");
        let v: toml::Value = toml::from_str(&b).unwrap();
        assert_eq!(v["mcp_servers"]["treeyard"]["env"]["TREEYARD_URL"].as_str(), Some("http://127.0.0.1:1"));
    }
}
