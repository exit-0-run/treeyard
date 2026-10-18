//! `treeyard daemon`: watches every workspace of a node and syncs one after its edits settle
//! (debounce), so the server sees each agent's work live as new changesets of its change.

use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::workspace::{GcOptions, Node, PRIVATE, WsState, git_busy};

pub struct Options {
    /// Quiet time after the last event before a workspace syncs.
    pub debounce: Duration,
    /// Also sync every workspace this often (a safety net for missed events); zero = never.
    pub rescan: Duration,
    /// Spare directories to keep ready for current main (see `Node::fill_spares`); zero = none.
    pub spares: usize,
    /// Run `Node::gc` at start and then this often; `None` = never.
    pub gc: Option<(Duration, GcOptions)>,
    /// Waits before the next try when git is busy in a workspace (the last one repeats).
    pub git_backoff: Vec<Duration>,
}

/// Default waits while git is busy in a workspace: 1 s, 5 s, 30 s, then every 120 s.
pub const GIT_BACKOFF_S: [u64; 4] = [1, 5, 30, 120];

impl Default for Options {
    fn default() -> Self {
        Options {
            debounce: Duration::from_millis(300),
            rescan: Duration::from_secs(30),
            spares: 0,
            gc: None,
            git_backoff: GIT_BACKOFF_S.iter().map(|s| Duration::from_secs(*s)).collect(),
        }
    }
}

/// Directory names whose events never trigger a sync (build output, layers, VCS data).
const QUIET: &[&str] = &[PRIVATE, ".git", "target", "node_modules", ".venv", "__pycache__", ".pytest_cache"];

/// The workspace an event path belongs to, unless the path is under a quiet directory.
fn workspace_of(ws_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(ws_root).ok()?;
    let mut parts = rel.components().filter_map(|c| match c {
        Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    });
    let name = parts.next()?;
    if parts.any(|p| QUIET.contains(&p.as_str())) {
        return None;
    }
    Some(name)
}

/// One gc pass; logs what it removed, or its error.
fn run_gc(node: &Node, gc: &GcOptions, log: &mut dyn FnMut(String)) {
    match node.gc(gc) {
        Ok(v) if v["removed"].as_array().is_some_and(|r| !r.is_empty()) => log(json!({"gc": v}).to_string()),
        Ok(_) => {}
        Err(e) => log(json!({"gc": "error", "error": format!("{e:#}")}).to_string()),
    }
}

/// Runs until `stop` is set. `log` receives one JSON line per sync.
pub fn run(node: &Node, o: &Options, stop: Arc<AtomicBool>, log: &mut dyn FnMut(String)) -> Result<()> {
    let ws_root = node.home.join("ws");
    std::fs::create_dir_all(&ws_root)?;
    // FSEvents reports canonical paths (/private/var/... on macOS).
    let ws_root = ws_root.canonicalize()?;
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })?;
    watcher.watch(&ws_root, RecursiveMode::Recursive)?;
    log(json!({"daemon": "watching", "dir": ws_root}).to_string());
    let finished = node.finish_removes()?;
    if !finished.is_empty() {
        log(json!({"daemon": "finished_removes", "workspaces": finished}).to_string());
    }
    // The warm pool fills in its own thread, so a slow clone never delays a sync.
    let pool = (o.spares > 0).then(|| {
        let (home, url, count, stop) = (node.home.clone(), node.url.clone(), o.spares, stop.clone());
        std::thread::spawn(move || {
            let node = Node::new(home, url);
            let mut last_err = String::new();
            while !stop.load(Ordering::Relaxed) {
                match node.fill_spares(count, None, false) {
                    Ok(_) => last_err.clear(),
                    Err(e) if format!("{e:#}") != last_err => {
                        last_err = format!("{e:#}");
                        eprintln!("{}", json!({"spares": "error", "error": last_err}));
                    }
                    Err(_) => {}
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
    });
    let mut dirty: BTreeMap<String, Instant> = BTreeMap::new();
    // Workspaces whose sync waits for git: (tries so far, next try).
    let mut busy: BTreeMap<String, (usize, Instant)> = BTreeMap::new();
    let mut last_rescan = Instant::now();
    let mut last_gc = Instant::now();
    if let Some((_, gc)) = &o.gc {
        run_gc(node, gc, log);
    }
    while !stop.load(Ordering::Relaxed) {
        if let Some((every, gc)) = &o.gc
            && last_gc.elapsed() >= *every
        {
            last_gc = Instant::now();
            run_gc(node, gc, log);
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(p) => {
                if let Some(name) = workspace_of(&ws_root, &p) {
                    dirty.insert(name, Instant::now());
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !o.rescan.is_zero() && last_rescan.elapsed() >= o.rescan {
            for ws in node.list()? {
                dirty.entry(ws.name).or_insert_with(|| Instant::now() - o.debounce);
            }
            last_rescan = Instant::now();
        }
        let now = Instant::now();
        for (name, (_, due)) in &busy {
            if *due <= now {
                dirty.entry(name.clone()).or_insert(now - o.debounce);
            }
        }
        let ready: Vec<String> =
            dirty.iter().filter(|(_, t)| t.elapsed() >= o.debounce).map(|(n, _)| n.clone()).collect();
        for name in ready {
            dirty.remove(&name);
            // Only ready workspaces: a create or setup in progress writes files that are not edits yet.
            let Some(ws) = node.get(&name).ok().filter(|w| w.state == WsState::Ready) else {
                busy.remove(&name);
                continue;
            };
            // A merge or rebase in the workspace's git: wait, with backoff, instead of sending its middle.
            if let Some(why) = git_busy(&ws.dir) {
                let tries = busy.get(&name).map_or(0, |b| b.0);
                let wait = o.git_backoff.get(tries).or(o.git_backoff.last()).copied().unwrap_or(Duration::from_secs(1));
                busy.insert(name.clone(), (tries + 1, Instant::now() + wait));
                log(json!({"workspace": name, "skipped": why, "retry_s": wait.as_secs_f64()}).to_string());
                continue;
            }
            busy.remove(&name);
            match node.sync(&name) {
                Ok(v) if v["changesets"].as_u64() == Some(0) => {}
                Ok(v) => log(v.to_string()),
                Err(e) => log(json!({"workspace": name, "error": format!("{e:#}")}).to_string()),
            }
        }
    }
    if let Some(p) = pool {
        let _ = p.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_map_to_workspaces_except_quiet_dirs() {
        let root = Path::new("/n/ws");
        assert_eq!(workspace_of(root, Path::new("/n/ws/a/src/main.rs")).as_deref(), Some("a"));
        assert_eq!(workspace_of(root, Path::new("/n/ws/a/target/debug/x")), None);
        assert_eq!(workspace_of(root, Path::new("/n/ws/b/.treeyard/env")), None);
        assert_eq!(workspace_of(root, Path::new("/n/other/a/x")), None);
    }
}
