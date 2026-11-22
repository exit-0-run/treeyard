//! The native tool set of the stdio bridge (`treeyard mcp --tools native`): the agent works in a
//! real directory with its own tools, and TreeYard only receives the result.
//!
//! `start` opens the agent's change and writes a full checkout of it to a directory of its own
//! (`workspace::checkout_dir`). The manifest of that checkout (path -> mode, blob, stat; the head
//! it was written from) lives beside the directory, never inside it: `<dir>.native.json`, with
//! the lock `<dir>.native.lock`. `sync` scans the directory against the manifest and sends what
//! changed (modified, added, deleted files; `.gitignore` rules and caches are honoured) through
//! `apply` with `seen` = the head the directory was written from, so a whole-file write on a
//! change that moved under it merges 3-way. The reply is `apply`'s, in the agent view. Then the
//! directory is refreshed to the change's head. `land` syncs, lands with the usual semantics, and
//! refreshes the directory: after a conflict it holds the rebased files with conflict markers
//! (`<<<<<<< main` .. `=======` .. `>>>>>>> change`); a conflicted file that still has markers is
//! never sent, and `land` refuses while one is left.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeSet;

use crate::lean::{Lean, prune};
use crate::workspace::{self, Place};
use crate::{client, conflict_view, rpc};

/// Untracked directories that `sync` never sends (caches and build output of common tools), on
/// top of the repository's `.gitignore` rules. Tracked files in them are still sent.
pub const SKIP_DIRS: &[&str] = &[
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".hypothesis",
    ".tox",
    ".nox",
    ".venv",
    "node_modules",
    ".git",
];
/// Untracked file names (`*` = any prefix) that `sync` never sends.
const SKIP_FILES: &[&str] = &["*.pyc", "*.pyo", ".DS_Store", ".coverage"];

const MARKERS_ADVICE: &str = "These files in dir still hold conflict markers, so they were not sent: keep the work \
on main (between <<<<<<< main and =======) and yours (between ======= and >>>>>>> change), remove the markers, \
then sync or land again.";
const CONFLICT_ADVICE: &str = "Not landed: main moved and the files in `conflicted` now hold conflict markers in \
dir: <<<<<<< main .. ======= is the work on main, ======= .. >>>>>>> change is yours. Keep both, remove the \
markers, run the tests in dir, then land again.";
const MOVED_ADVICE: &str = "Work landed on main in files you edit (main_moved_in); land merges it. To test with it \
first, call sync with rebase: true (dir then holds main plus your change).";
const LANDED_NOTE: &str = "dir now holds main with your change.";

/// The tools of the native set: `start`, `sync`, `land`, then `change` and `diff` of the agent set.
pub fn tools() -> Vec<Value> {
    let change = json!({"type": "string", "description": "your change id (ch_..., from start)"});
    let mut out = vec![
        json!({
            "name": "start",
            "description": "Call first. Opens your change and writes a full copy of the repository to `dir` (reply: \
            `id` = your change, `dir`). Work only in dir with your own tools and run the tests there.",
            "inputSchema": {"type": "object", "properties": {
                "intent": {"type": "string", "description": "one line: what this change does"},
                "paths": {"type": "array", "items": {"type": "string"}, "description": "files or dir/ you plan to edit (optional)"},
            }, "required": ["intent"], "additionalProperties": false},
        }),
        json!({
            "name": "sync",
            "description": "Sends what you changed in dir (edited, new, deleted files) into your change. Reply: \
            collisions with other open changes, undoes (another agent's landed lines you removed: put its part back), \
            main_moved_in. Optional; land syncs too.",
            "inputSchema": {"type": "object", "properties": {
                "change": change,
                "rebase": {"type": "boolean", "description": "also rebase onto main and rewrite dir (main plus your change)"},
            }, "required": ["change"], "additionalProperties": false},
        }),
        json!({
            "name": "land",
            "description": "Syncs dir, then lands your change on main. status: landed; conflict (fix the files in \
            `conflicted` in dir, land again); undoes (not landed: you remove another agent's landed work, put its part \
            back); refused (conflict markers left). undo = true with reason only when your task names those lines for removal.",
            "inputSchema": {"type": "object", "properties": {
                "change": change,
                "undo": {"type": "boolean", "description": "land although the change removes lines other agents landed; needs reason"},
                "reason": {"type": "string", "description": "why the task removes those lines; required with undo = true"},
            }, "required": ["change"], "additionalProperties": false},
        }),
    ];
    out.extend(rpc::agent_tools().into_iter().filter(|t| t["name"] == "change" || t["name"] == "diff"));
    out
}

/// The native tool names the bridge answers itself.
pub const OWN: &[&str] = &["start", "sync", "land"];

/// The checkout of `change` for this set: the directory `checkout` would use, the manifest and
/// lock beside it.
pub fn place(change: &str) -> Result<Place> {
    let dir = workspace::checkout_dir(change)?;
    let side = |ext: &str| dir.with_file_name(format!("{change}.{ext}"));
    Ok(Place { mark: side("native.json"), lock: side("native.lock"), dir })
}

/// One native call. `agent` names the agent for `start` (unless a token names it).
pub fn call(url: &str, agent: Option<&str>, lean: &mut Lean, name: &str, args: Value) -> Result<Value> {
    match name {
        "start" => start(url, agent, args),
        "sync" => {
            let change = change_arg(&args)?;
            let rebase = flag(&args, "rebase")?;
            let p = place(&change)?;
            let _l = p.lock()?;
            sync(url, &p, &change, rebase, lean)
        }
        "land" => {
            let change = change_arg(&args)?;
            let p = place(&change)?;
            let _l = p.lock()?;
            land(url, &p, &change, &args, lean)
        }
        _ => bail!("{name} is not a native tool"),
    }
}

fn change_arg(args: &Value) -> Result<String> {
    Ok(args["change"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("give change: your change id (ch_..., from start)")?
        .to_string())
}

fn flag(args: &Value, key: &str) -> Result<bool> {
    match &args[key] {
        Value::Null => Ok(false),
        Value::Bool(b) => Ok(*b),
        other => bail!("{key} must be true or false, not {other}"),
    }
}

fn start(url: &str, agent: Option<&str>, args: Value) -> Result<Value> {
    let intent =
        args["intent"].as_str().map(str::trim).filter(|s| !s.is_empty()).context("start needs intent: one line")?;
    let mut open = json!({"intent": intent});
    if let Some(p) = args.get("paths").filter(|p| !p.is_null()) {
        open["paths"] = p.clone();
    }
    if std::env::var_os("TREEYARD_TOKEN").is_none()
        && let Some(a) = agent
    {
        open["agent"] = json!(a);
    }
    let ch = client::call(url, "open_change", open)?;
    let id = ch["id"].as_str().context("open_change reply has no id")?.to_string();
    let p = place(&id)?;
    let written = (|| {
        p.check()?;
        let _l = p.lock()?;
        workspace::checkout_locked(url, &id, &p, &BTreeSet::new())
    })();
    let co = match written {
        Ok(co) => co,
        Err(e) => {
            let _ = client::call(url, "abandon", json!({"change": id}));
            return Err(e.context(format!("could not write the checkout; change {id} was abandoned, call start again")));
        }
    };
    let mut out =
        json!({"id": id, "dir": co["dir"], "files": co["files"], "overlaps": ch["overlaps"], "advice": ch["advice"]});
    prune(&mut out);
    Ok(out)
}

fn skipped(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let (dirs, name) = parts.split_at(parts.len() - 1);
    let glob = |n: &str, pat: &str| match pat.strip_prefix('*') {
        Some(suffix) => n.ends_with(suffix),
        None => n == pat,
    };
    dirs.iter().any(|d| SKIP_DIRS.contains(d) || d.ends_with(".egg-info"))
        || SKIP_FILES.iter().any(|pat| glob(name[0], pat))
}

fn has_markers(text: &[u8]) -> bool {
    let text = String::from_utf8_lossy(text);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    !conflict_view::blocks(&lines).is_empty()
}

/// What one sync did.
struct Synced {
    /// `apply`'s reply (or `change`'s when nothing was sent), as the server sent it.
    reply: Value,
    /// `apply` ran (`reply` is an apply reply).
    applied: bool,
    sent: Vec<String>,
    deleted: Vec<String>,
    /// Conflicted files that still hold markers: not sent, left as they are in dir.
    markers: Vec<String>,
}

/// Sends the edits made in `p.dir` since the last write or sync, for a caller that holds the lock.
/// Leaves the manifest at what was sent; the caller refreshes the directory.
fn send(url: &str, p: &Place, change: &str) -> Result<Synced> {
    let mut mark = p
        .load()
        .map_err(|_| anyhow::anyhow!("no native checkout of {change} here ({}): call start first", p.dir.display()))?;
    if mark.change != change {
        bail!("{} holds change {}, not {change}", p.dir.display(), mark.change);
    }
    let ch = client::call(url, "change", json!({"change": change}))?;
    if ch["state"] != "open" {
        bail!(
            "change {change} is {}: nothing to sync; call start for new work",
            ch["state"].as_str().unwrap_or("closed")
        );
    }
    let conflicted: BTreeSet<String> = ch["conflicts"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| c["path"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let skip: BTreeSet<String> = SKIP_DIRS.iter().map(|s| s.to_string()).collect();
    let found = workspace::scan(&p.dir, &mark.files, &skip)?;
    let mut edits = Vec::new();
    let mut next = Vec::new();
    let mut markers = Vec::new();
    let mut seen_paths = BTreeSet::new();
    for (path, f) in found.diffs {
        if f.is_some() && !mark.files.contains_key(&path) && skipped(&path) {
            continue;
        }
        seen_paths.insert(path.clone());
        if conflicted.contains(&path) && f.as_ref().is_some_and(|(_, d)| has_markers(d)) {
            markers.push(path);
            continue;
        }
        edits.push(workspace::edit_of(&path, f.as_ref()));
        next.push((path, f.map(|(f, _)| f)));
    }
    // A conflicted file left as written: with markers it is not settled; without (a delete or
    // mode conflict), its content as it stands is the resolution. Only when dir holds the
    // change's head: else the refresh writes the conflict first.
    let current = ch["head"].as_str() == Some(mark.head.as_str());
    for path in conflicted.iter().filter(|c| current && !seen_paths.contains(*c)) {
        let full = p.dir.join(path);
        match std::fs::symlink_metadata(&full) {
            Ok(md) if md.is_file() => {
                let data = std::fs::read(&full)?;
                if has_markers(&data) {
                    markers.push(path.clone());
                } else if let Some(st) = mark.files.get(path) {
                    let f = workspace::FileState {
                        mode: st.mode,
                        blob: st.blob.clone(),
                        size: st.size,
                        mtime_ns: st.mtime_ns,
                    };
                    edits.push(workspace::edit_of(path, Some(&(f, data))));
                }
            }
            Ok(_) => {}
            Err(_) => edits.push(workspace::edit_of(path, None)),
        }
    }
    for (path, f) in found.touched {
        mark.files.insert(path, f);
    }
    let sent: Vec<String> = edits
        .iter()
        .filter(|(e, _)| e["delete"] != true)
        .map(|(e, _)| e["path"].as_str().unwrap_or_default().to_string())
        .collect();
    let deleted: Vec<String> = edits
        .iter()
        .filter(|(e, _)| e["delete"] == true)
        .map(|(e, _)| e["path"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut reply = ch;
    let applied = !edits.is_empty();
    let mut batch = Vec::new();
    let mut size = 0;
    let mut flush = |batch: &mut Vec<Value>| -> Result<()> {
        if !batch.is_empty() {
            // `seen`: the head dir was written from, so a change that moved since merges 3-way.
            reply = client::call(
                url,
                "apply",
                json!({"change": change, "edits": std::mem::take(batch), "seen": mark.head}),
            )?;
        }
        Ok(())
    };
    for (e, n) in edits {
        if size + n > workspace::BATCH_BYTES && !batch.is_empty() {
            flush(&mut batch)?;
            size = 0;
        }
        size += n;
        batch.push(e);
    }
    flush(&mut batch)?;
    for (path, f) in next {
        match f {
            Some(f) => mark.files.insert(path, f),
            None => mark.files.remove(&path),
        };
    }
    p.save(&mark)?;
    Ok(Synced { reply, applied, sent, deleted, markers })
}

/// A change reply in the native view: conflicts as the list of files that hold markers in dir.
fn native_change(v: &mut Value) {
    let Some(m) = v.as_object_mut() else { return };
    let paths: Vec<Value> = m
        .get("conflicts")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| c.get("path").cloned()).collect())
        .unwrap_or_default();
    for k in ["conflicts", "resolve", "conflicts_cut", "conflicted"] {
        m.remove(k);
    }
    if !paths.is_empty() {
        m.insert("conflicted".into(), Value::Array(paths));
    }
}

/// The directory refresh: dir becomes the change's head (files not sent stay as they are).
fn refresh(url: &str, p: &Place, change: &str, keep: &[String]) -> Value {
    let keep: BTreeSet<String> = keep.iter().cloned().collect();
    workspace::checkout_locked(url, change, p, &keep)
        .unwrap_or_else(|e| json!({"error": format!("dir not rewritten: {e:#}")}))
}

fn sync(url: &str, p: &Place, change: &str, rebase: bool, lean: &mut Lean) -> Result<Value> {
    let s = send(url, p, change)?;
    let mut v = s.reply;
    let mut op = if s.applied { "apply" } else { "change" };
    if rebase {
        if !s.markers.is_empty() {
            bail!("rebase needs the conflicts settled first; still marked in dir: {}", s.markers.join(", "));
        }
        v = client::call(url, "rebase", json!({"change": change}))?;
        op = "rebase";
    }
    let co = refresh(url, p, change, &s.markers);
    lean.shape(op, &mut v);
    native_change(&mut v);
    if let Some(m) = v.as_object_mut() {
        // The agent has the change id and dir; these are noise in this view.
        for k in ["agent", "intent", "paths", "declared", "reviews", "behind_main"] {
            m.remove(k);
        }
    }
    v["sent"] = workspace::capped(s.sent);
    v["deleted"] = workspace::capped(s.deleted);
    if !s.markers.is_empty() {
        v["markers_left"] = json!(s.markers);
        v["markers_advice"] = json!(MARKERS_ADVICE);
    }
    if v.get("conflicted").is_some() {
        v["conflict_advice"] = json!(CONFLICT_ADVICE.replacen("Not landed: main moved and the", "The", 1));
    }
    if co["main_moved"] == true {
        v["main_moved_in"] = co["main_moved_in"].clone();
        v["main_moved_advice"] = json!(MOVED_ADVICE);
    }
    if let Some(e) = co.get("error") {
        v["dir_error"] = e.clone();
    }
    prune(&mut v);
    Ok(v)
}

fn land(url: &str, p: &Place, change: &str, args: &Value, lean: &mut Lean) -> Result<Value> {
    let s = send(url, p, change)?;
    if !s.markers.is_empty() {
        let co = refresh(url, p, change, &s.markers);
        let mut v = json!({
            "status": "refused", "change": change, "markers_left": s.markers, "advice": MARKERS_ADVICE,
            "sent": workspace::capped(s.sent), "deleted": workspace::capped(s.deleted), "dir_error": co.get("error"),
        });
        prune(&mut v);
        return Ok(v);
    }
    let mut req = json!({"change": change});
    for k in ["undo", "reason"] {
        if let Some(x) = args.get(k).filter(|x| !x.is_null()) {
            req[k] = x.clone();
        }
    }
    let mut v = client::call(url, "land", req)?;
    let co = refresh(url, p, change, &[]);
    lean.shape("land", &mut v);
    let status = v["status"].as_str().unwrap_or_default().to_string();
    if v["change"].is_object() {
        let mut c = v["change"].take();
        native_change(&mut c);
        if let Some(paths) = c.get("conflicted") {
            v["conflicted"] = paths.clone();
        }
        v["change"] = c["id"].clone();
    }
    if let Some(m) = v.as_object_mut() {
        m.remove("merged_from");
        m.remove("changeset");
    }
    match status.as_str() {
        "conflict" => v["advice"] = json!(CONFLICT_ADVICE),
        "landed" | "already_on_main" => v["note"] = json!(LANDED_NOTE),
        _ => {}
    }
    v["sent"] = workspace::capped(s.sent);
    v["deleted"] = workspace::capped(s.deleted);
    if let Some(e) = co.get("error") {
        v["dir_error"] = e.clone();
    }
    prune(&mut v);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_are_skipped_and_sources_are_not() {
        for p in [
            "__pycache__/m.cpython-312.pyc",
            "src/__pycache__/x.pyc",
            "a.pyc",
            "pkg.egg-info/PKG-INFO",
            ".DS_Store",
            "node_modules/x/i.js",
        ] {
            assert!(skipped(p), "{p}");
        }
        for p in ["src/main.py", "build.rs", "docs/pycache.md", "target.txt", "a.pyc.txt"] {
            assert!(!skipped(p), "{p}");
        }
    }

    #[test]
    fn markers_are_marker_blocks() {
        assert!(has_markers(b"a\n<<<<<<< main\nx\n=======\ny\n>>>>>>> change\nb\n"));
        assert!(!has_markers(b"a\n=======\nb\n"));
        assert!(!has_markers(b"<<<<<<< main\nx\n"));
    }

    #[test]
    fn the_manifest_sits_beside_the_tree() {
        let p = place("ch_0123456789ab").unwrap();
        assert_eq!(p.mark.parent(), p.dir.parent());
        assert!(!p.mark.starts_with(&p.dir));
        assert!(!p.lock.starts_with(&p.dir));
    }
}
