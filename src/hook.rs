//! `treeyard workspace-hook create|remove`: the command an orchestrator runs when it creates or
//! removes an agent's workspace. It is a thin layer over [`Node`]: `create` opens a change and
//! makes its TreeYard workspace, `remove` sends the last edits and deletes the directory.
//!
//! Made for scripts: every input is an argument or an environment variable, stdout gets only
//! the workspace path (one line), everything else goes to stderr. Exit codes: 0 done, 1 error,
//! 2 usage (bad name, no server address), 3 the server does not answer.
//!
//! Server address: `--url` / `TREEYARD_URL`, else the `connect.json` of the repository at the
//! working directory (`treeyard connect`). There is no fixed default port: a hook that guessed
//! one could open changes on an unrelated server.

use crate::workspace::{CreateOptions, Node, Workspace, WsState};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub const EXIT_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_NO_SERVER: i32 = 3;

/// A failed hook: the exit code and the text for stderr.
#[derive(Debug)]
pub struct HookError {
    pub code: i32,
    pub msg: String,
}

impl HookError {
    fn new(code: i32, msg: impl Into<String>) -> Self {
        HookError { code, msg: msg.into() }
    }
}

impl From<anyhow::Error> for HookError {
    fn from(e: anyhow::Error) -> Self {
        HookError::new(EXIT_ERROR, format!("{e:#}"))
    }
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

/// What both subcommands share.
pub struct Common {
    /// `--url` / `TREEYARD_URL`.
    pub url: Option<String>,
    /// The node home (`--home` / `TREEYARD_HOME`, else `~/.treeyard`).
    pub home: PathBuf,
    /// The working directory: where `connect.json` and the default name come from.
    pub cwd: PathBuf,
}

pub struct CreateArgs {
    pub name: Option<String>,
    pub agent: Option<String>,
    pub intent: Option<String>,
    pub setup: Option<String>,
    pub git: bool,
    /// A checkout (the orchestrator's worktree) whose uncommitted edits seed the change.
    pub from_dir: Option<PathBuf>,
}

pub struct RemoveArgs {
    /// A workspace name or a workspace path.
    pub target: Option<String>,
    pub from_dir: Option<PathBuf>,
    pub abandon: bool,
    pub force: bool,
}

/// True for a name of letters, digits, `.`, `_` and `-` (not `.` or `..`).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The default name: the base name of `dir`, with every other character replaced by `-`.
pub fn name_from_dir(dir: &Path) -> Option<String> {
    let base = dir.file_name()?.to_string_lossy();
    let name: String =
        base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    let name = name.trim_start_matches('.').to_string();
    valid_name(&name).then_some(name)
}

/// The server URL that `treeyard connect` recorded for the repository that holds `dir`. A
/// linked worktree finds it too: the state lives in the common git directory.
pub fn connect_url(dir: &Path) -> Option<String> {
    let out = crate::git::cmd()
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    let st: crate::connect::State =
        serde_json::from_slice(&std::fs::read(common.join("treeyard").join("connect.json")).ok()?).ok()?;
    (!st.url.is_empty()).then_some(st.url)
}

/// `explicit`, else the connect state of the repository at `dir`, else exit 2.
pub fn resolve_url(explicit: Option<&str>, dir: &Path) -> Result<String, HookError> {
    if let Some(u) = explicit.map(str::trim).filter(|u| !u.is_empty()) {
        return Ok(u.trim_end_matches('/').to_string());
    }
    connect_url(dir).ok_or_else(|| {
        HookError::new(
            EXIT_USAGE,
            format!(
                "no server address: set TREEYARD_URL (or --url), or run `treeyard connect` in the repository \
                 (no connect state found from {})",
                dir.display()
            ),
        )
    })
}

/// One `status` call with a short timeout and no retries. A server that does not answer gives
/// exit 3; a server that refuses the token gives exit 1.
pub fn probe(url: &str) -> Result<(), HookError> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(3)).build();
    let mut req = agent.post(&format!("{url}/api/status"));
    if let Ok(t) = std::env::var("TREEYARD_TOKEN")
        && !t.is_empty()
    {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    match req.send_json(serde_json::json!({})) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => Err(HookError::new(
            EXIT_ERROR,
            format!("treeyard at {url} refused the request (HTTP {code}): check TREEYARD_TOKEN"),
        )),
        Err(ureq::Error::Status(code, _)) => {
            Err(HookError::new(EXIT_NO_SERVER, format!("treeyard at {url} answered HTTP {code} to status")))
        }
        Err(ureq::Error::Transport(t)) => {
            Err(HookError::new(EXIT_NO_SERVER, format!("treeyard does not answer at {url}: {t}")))
        }
    }
}

/// Opens a change and makes its workspace; returns the workspace directory. A ready workspace of
/// the same name is returned as it is (no second change). A failed one is removed (its change
/// abandoned) and made again.
pub fn create(c: &Common, a: &CreateArgs) -> Result<PathBuf, HookError> {
    let from_dir = a.from_dir.as_deref().map(|d| absolute(&c.cwd, d));
    let name = match &a.name {
        Some(n) if valid_name(n) => n.clone(),
        Some(n) => {
            return Err(HookError::new(EXIT_USAGE, format!("bad workspace name {n:?}: use letters, digits, . _ -")));
        }
        None => name_from_dir(from_dir.as_deref().unwrap_or(&c.cwd))
            .ok_or_else(|| HookError::new(EXIT_USAGE, "no workspace name: pass --name or set TREEYARD_HOOK_NAME"))?,
    };
    let url = resolve_url(c.url.as_deref(), from_dir.as_deref().unwrap_or(&c.cwd))?;
    probe(&url)?;
    let node = Node::new(&c.home, &url);
    if let Ok(ws) = node.get(&name) {
        match ws.state {
            WsState::Ready if ws.dir.is_dir() => {
                eprintln!("workspace-hook: workspace {name} exists (change {}), not created again", ws.change);
                return Ok(ws.dir);
            }
            WsState::Failed => {
                // A failed create synced nothing yet, so its change is empty: drop it.
                eprintln!("workspace-hook: workspace {name} failed before; removing it and creating it again");
                node.remove(&name, true, true)?;
            }
            WsState::Ready => {
                // The directory is gone; synced edits stay on the open change.
                eprintln!(
                    "workspace-hook: workspace {name} has no directory; forgetting it (change {} stays open)",
                    ws.change
                );
                node.remove(&name, false, true)?;
            }
            s => {
                return Err(HookError::new(
                    EXIT_ERROR,
                    format!("workspace {name} is {s:?} (another create or remove runs, or one was cut short)"),
                ));
            }
        }
    }
    let o = CreateOptions {
        name: name.clone(),
        agent: a.agent.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("hook-{name}")),
        intent: a.intent.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("work in workspace {name}")),
        setup: a.setup.clone().filter(|s| !s.is_empty()),
        git: a.git,
        copy_from: from_dir.clone(),
        from_dir,
        ..Default::default()
    };
    let (ws, timing) = node.create(&o)?;
    if let Some(err) = timing["from_dir"]["sync"]["error"].as_str() {
        eprintln!("workspace-hook: the seed edits are in the workspace, but the sync failed: {err}");
    }
    eprintln!(
        "workspace-hook: workspace {name}: change {} for agent {} on {} ({} s)",
        ws.change, ws.agent, url, timing["total_s"]
    );
    Ok(ws.dir)
}

/// Sends the last edits, then deletes the workspace; returns its former directory, or None
/// when no such workspace exists (exit 0: a repeated remove is not an error).
pub fn remove(c: &Common, a: &RemoveArgs) -> Result<Option<PathBuf>, HookError> {
    let probe_node = Node::new(&c.home, "");
    let ws = match (&a.target, &a.from_dir) {
        (Some(t), _) if t.contains('/') => find_by_dir(&probe_node, &absolute(&c.cwd, Path::new(t)))?,
        (Some(t), _) if valid_name(t) => probe_node.get(t).ok(),
        (Some(t), _) => return Err(HookError::new(EXIT_USAGE, format!("bad workspace name {t:?}"))),
        (None, Some(d)) => name_from_dir(&absolute(&c.cwd, d)).and_then(|n| probe_node.get(&n).ok()),
        (None, None) => match find_by_dir(&probe_node, &c.cwd)? {
            Some(ws) => Some(ws),
            None => name_from_dir(&c.cwd).and_then(|n| probe_node.get(&n).ok()),
        },
    };
    let Some(ws) = ws else {
        eprintln!("workspace-hook: workspace not found in {}; nothing to remove", c.home.display());
        return Ok(None);
    };
    // The recorded URL is the server the change lives on; an explicit one or connect state wins.
    let url = match resolve_url(c.url.as_deref(), &c.cwd) {
        Ok(u) => u,
        Err(_) if !ws.url.is_empty() => ws.url.clone(),
        Err(e) => return Err(e),
    };
    if let Err(e) = probe(&url) {
        if !a.force {
            return Err(HookError::new(
                e.code,
                format!("{}; nothing was deleted (retry when the server answers, or pass --force)", e.msg),
            ));
        }
        eprintln!("workspace-hook: {} (--force: deleting anyway, unsent edits are lost)", e.msg);
    }
    let node = Node::new(&c.home, &url);
    let v = node.remove(&ws.name, a.abandon, a.force)?;
    let sent = v["synced"]["changesets"].as_u64().unwrap_or(0);
    eprintln!(
        "workspace-hook: removed workspace {} (change {}, {sent} changesets sent{})",
        ws.name,
        ws.change,
        if v["abandoned"] == true { ", change abandoned" } else { ", change stays open" }
    );
    Ok(Some(ws.dir))
}

fn absolute(cwd: &Path, p: &Path) -> PathBuf {
    let p = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn find_by_dir(node: &Node, dir: &Path) -> Result<Option<Workspace>, HookError> {
    let want = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    for ws in node.list()? {
        let have = std::fs::canonicalize(&ws.dir).unwrap_or_else(|_| ws.dir.clone());
        if have == want || ws.dir == dir {
            return Ok(Some(ws));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("fix-auth_2.b"));
        for bad in ["", ".", "..", "a/b", "a b", "ä"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert_eq!(name_from_dir(Path::new("/x/feat auth")).as_deref(), Some("feat-auth"));
        assert_eq!(name_from_dir(Path::new("/x/.hidden")).as_deref(), Some("hidden"));
        assert_eq!(name_from_dir(Path::new("/")), None);
    }
}
