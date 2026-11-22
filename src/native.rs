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
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::lean::{Lean, prune};
use crate::workspace::{self, Place};
use crate::{client, conflict_view, gate, rpc};

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

/// New files that look like editor or tool backups (`sed -i -e` leaves `name-e`): never sent.
const BACKUP_SUFFIXES: &[&str] = &[".orig", ".rej", "~", ".bak", ".swp"];

const MARKERS_ADVICE: &str = "These files in dir still hold conflict markers, so they were not sent: keep the work \
on main (between <<<<<<< main and =======) and yours (between ======= and >>>>>>> change), remove the markers, \
then sync or land again.";
const CONFLICT_ADVICE: &str = "Not landed: main moved and the files in `conflicted` now hold conflict markers in \
dir: <<<<<<< main .. ======= is the work on main, ======= .. >>>>>>> change is yours. Keep both, remove the \
markers, run the tests in dir, then land again.";
const DECIDE_ADVICE: &str = "These conflicts have no markers to edit (a file deleted on one side, a mode or binary \
change): nothing was sent for them. Decide each one: land (or sync) with resolve = {\"path\": \"mine\"} to keep \
the file as it is in dir (absent = deleted), or {\"path\": \"main\"} to take main's side.";
const STALE_ADVICE: &str = "dir was behind your change for these conflicted files: your copies were written before \
the conflict, so they were not sent. dir now holds the conflict with markers; your copies are in `stale_copies`. \
Redo your edits on the marked files, then land again.";
const MOVED_ADVICE: &str = "Work landed on main in files you edit (main_moved_in); land merges it. To test with it \
first, call sync with rebase: true (dir then holds main plus your change).";
const LANDED_NOTE: &str = "dir now holds main with your change.";
/// The one rule for a held land, in the block, the `land` schema and this reply alike.
pub const HOLD_ADVICE: &str = "Not landed: your change removes what another agent landed in these lines for its own \
task. Put its part back in dir (the words in `contributed`, else the whole line) while keeping your own edit of the \
same lines, and land again. Only when your own task names those lines for removal, land with undo = true and a \
reason that quotes your task. Otherwise, when both cannot hold, keep its part, land the rest of your work, and say \
in your last message what could not hold.";

/// The tools of the native set: `start`, `sync`, `land`, then `change` and `diff` of the agent set.
pub fn tools() -> Vec<Value> {
    let change = json!({"type": "string", "description": "your change id (ch_..., from start)"});
    let resolve = json!({"type": "object", "additionalProperties": {"type": "string", "enum": ["mine", "main"]},
        "description": "your decision for each path listed in `decide`: mine = the file as it is in dir (absent = deleted), main = main's side"});
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
            added and skipped new files, collisions with other open changes, undoes (another agent's landed lines you \
            removed: put its part back), main_moved_in. Optional; land syncs too.",
            "inputSchema": {"type": "object", "properties": {
                "change": change,
                "rebase": {"type": "boolean", "description": "also rebase onto main and rewrite dir (main plus your change)"},
                "resolve": resolve,
            }, "required": ["change"], "additionalProperties": false},
        }),
        json!({
            "name": "land",
            "description": "Syncs dir, then lands your change on main. status: landed; conflict (fix the files in \
            `conflicted` in dir, land again); undoes (not landed: you remove another agent's landed work; put its part \
            back and land again); tests_failed (the repository's tests failed on your change on top of main: fix it \
            in dir, land again); refused (markers left, or conflicts to decide). undo = true with a reason only when \
            your own task names those lines for removal.",
            "inputSchema": {"type": "object", "properties": {
                "change": change,
                "undo": {"type": "boolean", "description": "only when your own task names those lines for removal; needs reason"},
                "reason": {"type": "string", "description": "the words of your task that name those lines for removal; required with undo = true"},
                "resolve": resolve,
            }, "required": ["change"], "additionalProperties": false},
        }),
    ];
    out.extend(rpc::agent_tools().into_iter().filter(|t| t["name"] == "change" || t["name"] == "diff"));
    out
}

/// The native tool names the bridge answers itself.
pub const OWN: &[&str] = &["start", "sync", "land"];

/// The checkout of `change` for this set: the directory `checkout` would use (or, for a follow-up
/// change, the directory its manifest names), the manifest and the directory's lock beside it.
pub fn place(change: &str) -> Result<Place> {
    let own = workspace::checkout_dir(change)?;
    let mark = own.with_file_name(format!("{change}.native.json"));
    let dir = workspace::Place { dir: own.clone(), mark: mark.clone(), lock: own.clone(), keep_edits: true }
        .load()
        .ok()
        .and_then(|m| m.dir)
        .unwrap_or(own);
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Place { lock: dir.with_file_name(format!("{name}.native.lock")), mark, dir, keep_edits: true })
}

/// One native call. `agent` names the agent for `start` and for a follow-up change.
pub fn call(url: &str, agent: Option<&str>, lean: &mut Lean, name: &str, args: Value) -> Result<Value> {
    match name {
        "start" => start(url, agent, args),
        "sync" | "land" => {
            let change = change_arg(&args)?;
            let p = place(&change)?;
            let _l = p.lock()?;
            let (change, p, follow) = follow_up(url, agent, change, p)?;
            let resolve = resolve_arg(&args)?;
            let mut v = if name == "sync" {
                sync(url, &p, &change, flag(&args, "rebase")?, &resolve, lean)?
            } else {
                land(url, &p, &change, &args, &resolve, lean)?
            };
            if let Some(f) = follow {
                v["follow_up"] = f;
            }
            Ok(v)
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

fn resolve_arg(args: &Value) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    match &args["resolve"] {
        Value::Null => {}
        Value::Object(m) => {
            for (k, v) in m {
                match v.as_str() {
                    Some(x @ ("mine" | "main")) => {
                        out.insert(k.clone(), x.to_string());
                    }
                    _ => bail!("resolve.{k} must be \"mine\" or \"main\", not {v}"),
                }
            }
        }
        other => bail!("resolve must be an object {{path: \"mine\" | \"main\"}}, not {other}"),
    }
    Ok(out)
}

fn start(url: &str, agent: Option<&str>, args: Value) -> Result<Value> {
    let intent =
        args["intent"].as_str().map(str::trim).filter(|s| !s.is_empty()).context("start needs intent: one line")?;
    let mut open = json!({"intent": intent});
    if let Some(p) = args.get("paths").filter(|p| !p.is_null()) {
        open["paths"] = p.clone();
    }
    name_agent(&mut open, agent);
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

fn name_agent(open: &mut Value, agent: Option<&str>) {
    if std::env::var_os("TREEYARD_TOKEN").is_none()
        && let Some(a) = agent
    {
        open["agent"] = json!(a);
    }
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

/// A new file that looks like a backup: `*.orig`, `*.rej`, `*~`, `*.bak`, `*.swp`, or `name-e`
/// next to a file `name` (what BSD `sed -i -e` leaves).
fn backup(rel: &str, dir: &Path, known: &workspace::State) -> bool {
    if BACKUP_SUFFIXES.iter().any(|s| rel.ends_with(s)) {
        return true;
    }
    rel.strip_suffix("-e")
        .is_some_and(|stem| !stem.is_empty() && (known.contains_key(stem) || dir.join(stem).is_file()))
}

fn has_markers(text: &[u8]) -> bool {
    let text = String::from_utf8_lossy(text);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    !conflict_view::blocks(&lines).is_empty()
}

/// Conflicts with marker text to edit; the other kinds need a decision (`resolve`).
fn has_marker_kind(kind: &str) -> bool {
    matches!(kind, "content" | "add_add")
}

fn side(v: &Value) -> String {
    if v.is_null() {
        "deleted".into()
    } else {
        format!("has it (mode {}, {} bytes)", v["mode"].as_str().unwrap_or("?"), v["size"])
    }
}

/// The state of a file in dir as the manifest records it (so a rewrite replaces it), or none.
fn disk_state(dir: &Path, path: &str) -> Result<Option<workspace::FileState>> {
    let full = dir.join(path);
    let Ok(md) = std::fs::symlink_metadata(&full) else { return Ok(None) };
    if md.is_dir() {
        return Ok(None);
    }
    let (mode, blob, _) = workspace::hash_file(&full, &md)?;
    Ok(Some(workspace::FileState { mode, blob, size: md.len(), mtime_ns: workspace::mtime_ns(&md) }))
}

/// Marks `path` in the manifest as written as it is on disk, so the next rewrite of dir puts the
/// change's content there (instead of keeping it as an unsent edit).
fn let_rewrite(mark: &mut workspace::CheckoutMark, dir: &Path, path: &str) -> Result<()> {
    match disk_state(dir, path)? {
        Some(st) => mark.files.insert(path.to_string(), st),
        None => mark.files.remove(path),
    };
    Ok(())
}

/// The new and changed files of dir against the manifest (skipped and backup files left out).
fn unsent(p: &Place, mark: &workspace::CheckoutMark) -> Result<Vec<String>> {
    let skip: BTreeSet<String> = SKIP_DIRS.iter().map(|s| s.to_string()).collect();
    let found = workspace::scan_racy(&p.dir, &mark.files, &skip, &[], workspace::racy_from(&p.mark))?;
    Ok(found
        .diffs
        .into_iter()
        .filter(|(path, f)| {
            f.is_none() || mark.files.contains_key(path) || !(skipped(path) || backup(path, &p.dir, &mark.files))
        })
        .map(|(path, _)| path)
        .collect())
}

/// When `change` has landed (or was abandoned) and dir holds edits it never sent, a follow-up
/// change on main as it is now takes over the same directory; a later call with the old id goes
/// to it. Returns the change to work on, its place, and the note for the reply.
fn follow_up(url: &str, agent: Option<&str>, change: String, p: Place) -> Result<(String, Place, Option<Value>)> {
    let mut mark = p
        .load()
        .map_err(|_| anyhow::anyhow!("no native checkout of {change} here ({}): call start first", p.dir.display()))?;
    if let Some(next) = mark.followed_by.clone() {
        let (c, q, f) = follow_up(url, agent, next.clone(), place(&next)?)?;
        let note = f.unwrap_or_else(|| json!({"from": change, "id": next, "note": format!("change {change} has landed; dir belongs to change {next} now: use change: {next}")}));
        return Ok((c, q, Some(note)));
    }
    let ch = client::call(url, "change", json!({"change": change}))?;
    if ch["state"] == "open" {
        return Ok((change, p, None));
    }
    let state = ch["state"].as_str().unwrap_or("closed").to_string();
    let files = unsent(&p, &mark)?;
    if files.is_empty() {
        bail!("change {change} is {state} and dir holds no unsent edits: nothing to sync; call start for new work");
    }
    let intent = format!("follow-up of {change}: {}", ch["intent"].as_str().unwrap_or_default());
    let mut open = json!({"intent": intent, "paths": files});
    name_agent(&mut open, agent);
    let next = client::call(url, "open_change", open)?;
    let id = next["id"].as_str().context("open_change reply has no id")?.to_string();
    let q = place(&id)?;
    let follow = workspace::CheckoutMark {
        change: id.clone(),
        head: mark.head.clone(),
        files: mark.files.clone(),
        dir: Some(p.dir.clone()),
        followed_by: None,
    };
    let q = Place { dir: p.dir.clone(), lock: p.lock.clone(), ..q };
    q.save(&follow)?;
    mark.followed_by = Some(id.clone());
    p.save(&mark)?;
    let note = json!({
        "from": change, "id": id, "files": workspace::capped(files),
        "note": format!("change {change} had already {state}: the unsent edits in dir go into a new change {id} \
        (same dir), merged with main as it is now. Use change: {id} from now on."),
    });
    Ok((id, q, Some(note)))
}

/// What one sync did.
#[derive(Default)]
struct Synced {
    /// `apply`'s reply (or `change`'s when nothing was sent), as the server sent it.
    reply: Value,
    /// `apply` ran (`reply` is an apply reply).
    applied: bool,
    sent: Vec<String>,
    added: Vec<String>,
    deleted: Vec<String>,
    /// New files not sent: caches, backups.
    skipped: Vec<String>,
    /// Conflicted files that still hold markers: not sent, left as they are in dir.
    markers: Vec<String>,
    /// Conflicts without markers that wait for the agent's `resolve`.
    decide: Vec<Value>,
    /// Conflicted files edited in a dir older than the conflict: not sent, copies kept here.
    stale: Vec<String>,
    stale_dir: Option<std::path::PathBuf>,
    /// The repository's land test, from the `change` reply (`land_test`), if any.
    gate: Option<Value>,
    /// The repository's land queue, from the `change` reply (`land_queue`), if any.
    queue: Option<Value>,
}

impl Synced {
    /// Why a land cannot go on now, for the reply (empty: it can).
    fn blocked(&self) -> bool {
        !self.markers.is_empty() || !self.decide.is_empty() || !self.stale.is_empty()
    }

    fn report(&self, v: &mut Value) {
        v["sent"] = workspace::capped(self.sent.clone());
        v["added"] = workspace::capped(self.added.clone());
        v["deleted"] = workspace::capped(self.deleted.clone());
        v["skipped"] = workspace::capped(self.skipped.clone());
        if !self.markers.is_empty() {
            v["markers_left"] = json!(self.markers);
            v["markers_advice"] = json!(MARKERS_ADVICE);
        }
        if !self.decide.is_empty() {
            v["decide"] = json!(self.decide);
            v["decide_advice"] = json!(DECIDE_ADVICE);
        }
        if !self.stale.is_empty() {
            v["stale_conflicts"] = json!(self.stale);
            v["stale_copies"] = json!(self.stale_dir);
            v["stale_advice"] = json!(STALE_ADVICE);
        }
    }
}

/// Sends the edits made in `p.dir` since the last write or sync, for a caller that holds the lock.
/// Leaves the manifest at what was sent; the caller refreshes the directory.
fn send(url: &str, p: &Place, change: &str, resolve: &BTreeMap<String, String>) -> Result<Synced> {
    let mut mark = p
        .load()
        .map_err(|_| anyhow::anyhow!("no native checkout of {change} here ({}): call start first", p.dir.display()))?;
    if mark.change != change {
        bail!("{} holds change {}, not {change}", p.dir.display(), mark.change);
    }
    let mut ch = client::call(url, "change", json!({"change": change}))?;
    if ch["state"] != "open" {
        bail!(
            "change {change} is {}: nothing to sync; call start for new work",
            ch["state"].as_str().unwrap_or("closed")
        );
    }
    let kinds = |ch: &Value| -> BTreeMap<String, Value> {
        ch["conflicts"]
            .as_array()
            .map(|a| a.iter().filter_map(|c| Some((c["path"].as_str()?.to_string(), c.clone()))).collect())
            .unwrap_or_default()
    };
    let mut out = Synced::default();
    // Decisions first: `main` takes main's side on the server, and dir gets it on the rewrite.
    for (path, choice) in resolve {
        let Some(c) = kinds(&ch).get(path).cloned() else {
            bail!("resolve names {path}, which has no conflict in change {change}");
        };
        if has_marker_kind(c["kind"].as_str().unwrap_or_default()) {
            bail!("{path} has conflict markers to edit in dir: fix the file instead of resolve");
        }
        if choice == "main" {
            client::call(url, "resolve", json!({"change": change, "path": path, "take": "theirs"}))?;
            let_rewrite(&mut mark, &p.dir, path)?;
        }
    }
    if resolve.values().any(|c| c == "main") {
        ch = client::call(url, "change", json!({"change": change}))?;
    }
    out.gate = ch.get("land_test").cloned();
    out.queue = ch.get("land_queue").cloned();
    let conflicts = kinds(&ch);
    // dir holds the change's head (else it was written before the change moved, a refresh failed).
    let current = ch["head"].as_str() == Some(mark.head.as_str());
    let skip: BTreeSet<String> = SKIP_DIRS.iter().map(|s| s.to_string()).collect();
    let found = workspace::scan_racy(&p.dir, &mark.files, &skip, &[], workspace::racy_from(&p.mark))?;
    let mut edits = Vec::new();
    let mut next = Vec::new();
    let mut seen_paths = BTreeSet::new();
    for (path, f) in found.diffs {
        let new = f.is_some() && !mark.files.contains_key(&path);
        if new && (skipped(&path) || backup(&path, &p.dir, &mark.files)) {
            out.skipped.push(path);
            continue;
        }
        seen_paths.insert(path.clone());
        if let Some(c) = conflicts.get(&path) {
            let kind = c["kind"].as_str().unwrap_or_default();
            if !current {
                // Edited before the conflict existed: never a resolution. Keep a copy, let the rewrite
                // put the conflict in its place.
                let to = p.dir.with_file_name(format!(
                    "{}.native-stale",
                    p.dir.file_name().unwrap_or_default().to_string_lossy()
                ));
                if let Some((_, data)) = &f {
                    let dst = to.join(&path);
                    std::fs::create_dir_all(dst.parent().unwrap_or(&to))?;
                    std::fs::write(&dst, data)?;
                }
                let_rewrite(&mut mark, &p.dir, &path)?;
                out.stale.push(path);
                out.stale_dir = Some(to);
                continue;
            }
            if !has_marker_kind(kind) {
                if resolve.get(&path).map(String::as_str) != Some("mine") {
                    out.decide.push(decision(&path, c));
                    continue;
                }
            } else if f.as_ref().is_some_and(|(_, d)| has_markers(d)) {
                out.markers.push(path);
                continue;
            }
        }
        if new {
            out.added.push(path.clone());
        }
        edits.push(workspace::edit_of(&path, f.as_ref()));
        next.push((path, f.map(|(f, _)| f)));
    }
    // Conflicted files left as written.
    for (path, c) in conflicts.iter().filter(|(c, _)| !seen_paths.contains(*c)) {
        let kind = c["kind"].as_str().unwrap_or_default();
        if !current {
            continue; // the rewrite writes the conflict
        }
        if !has_marker_kind(kind) {
            if resolve.get(path).map(String::as_str) == Some("mine") {
                match disk_state(&p.dir, path)? {
                    Some(st) => {
                        let data = std::fs::read(p.dir.join(path))?;
                        edits.push(workspace::edit_of(path, Some(&(st, data))));
                    }
                    None => edits.push(workspace::edit_of(path, None)),
                }
            } else {
                out.decide.push(decision(path, c));
            }
            continue;
        }
        match std::fs::read(p.dir.join(path)) {
            Ok(data) if has_markers(&data) => out.markers.push(path.clone()),
            Ok(data) => {
                if let Some(st) = disk_state(&p.dir, path)? {
                    edits.push(workspace::edit_of(path, Some(&(st, data))));
                }
            }
            Err(_) => edits.push(workspace::edit_of(path, None)),
        }
    }
    for (path, f) in found.touched {
        mark.files.insert(path, f);
    }
    for (e, _) in &edits {
        let path = e["path"].as_str().unwrap_or_default().to_string();
        if e["delete"] == true { out.deleted.push(path) } else { out.sent.push(path) }
    }
    out.reply = ch;
    out.applied = !edits.is_empty();
    let mut batch = Vec::new();
    let mut size = 0;
    let mut reply = Value::Null;
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
    if !reply.is_null() {
        out.reply = reply;
    }
    for (path, f) in next {
        match f {
            Some(f) => mark.files.insert(path, f),
            None => mark.files.remove(&path),
        };
    }
    p.save(&mark)?;
    Ok(out)
}

fn decision(path: &str, c: &Value) -> Value {
    json!({"path": path, "kind": c["kind"], "mine": side(&c["ours"]), "main": side(&c["theirs"]),
        "main_by": c["theirs_by"].as_array().map(|a| a.iter().filter_map(|t| t.get("agent").cloned()).collect::<Vec<_>>())})
}

/// A change reply in the native view: conflicts as the list of files that hold markers in dir.
fn native_change(v: &mut Value) {
    let Some(m) = v.as_object_mut() else { return };
    let paths: Vec<Value> = m
        .get("conflicts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|c| has_marker_kind(c["kind"].as_str().unwrap_or_default()))
                .filter_map(|c| c.get("path").cloned())
                .collect()
        })
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

/// Conflicts of the change that need a decision, from a fresh `change` reply.
fn decisions(url: &str, change: &str) -> Vec<Value> {
    let ch = client::call(url, "change", json!({"change": change})).unwrap_or_default();
    ch["conflicts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|c| !has_marker_kind(c["kind"].as_str().unwrap_or_default()))
                .filter_map(|c| Some(decision(c["path"].as_str()?, c)))
                .collect()
        })
        .unwrap_or_default()
}

fn sync(
    url: &str,
    p: &Place,
    change: &str,
    rebase: bool,
    resolve: &BTreeMap<String, String>,
    lean: &mut Lean,
) -> Result<Value> {
    let mut s = send(url, p, change, resolve)?;
    let mut v = std::mem::take(&mut s.reply);
    let mut op = if s.applied { "apply" } else { "change" };
    if rebase {
        if s.blocked() {
            bail!("rebase needs the conflicts settled first (markers, decisions or stale files in dir)");
        }
        v = client::call(url, "rebase", json!({"change": change}))?;
        op = "rebase";
        s.decide = decisions(url, change);
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
    s.report(&mut v);
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

fn land(
    url: &str,
    p: &Place,
    change: &str,
    args: &Value,
    resolve: &BTreeMap<String, String>,
    lean: &mut Lean,
) -> Result<Value> {
    let s = send(url, p, change, resolve)?;
    if s.blocked() {
        let co = refresh(url, p, change, &s.markers);
        let mut v = json!({"status": "refused", "change": change, "dir_error": co.get("error")});
        s.report(&mut v);
        if !s.markers.is_empty() {
            v["advice"] = json!(MARKERS_ADVICE);
        }
        prune(&mut v);
        return Ok(v);
    }
    let mut req = json!({"change": change});
    for k in ["undo", "reason"] {
        if let Some(x) = args.get(k).filter(|x| !x.is_null()) {
            req[k] = x.clone();
        }
    }
    let gate = s.gate.as_ref().and_then(|g| gate::Gate::of(&json!({"land_test": g})));
    let queue = s.queue.as_ref().and_then(|q| gate::Queue::of(&json!({"land_queue": q})));
    let mut v = match (&gate, &queue) {
        (Some(g), _) => {
            // Rebase, write dir to exactly that tree, test it, land with the proof.
            let mut prepare = || -> Result<Option<Value>> {
                let rb = client::call(url, "rebase", json!({"change": change}))?;
                if rb["conflicted"] == true {
                    return Ok(Some(json!({"status": "conflict", "change": rb})));
                }
                let co = refresh(url, p, change, &[]);
                if let Some(e) = co.get("error") {
                    anyhow::bail!("the land test did not run: {e}");
                }
                let left = unsent(p, &p.load()?)?;
                if !left.is_empty() {
                    return Ok(Some(json!({"status": "refused", "change": change, "unsent": left,
                        "advice": "files in dir changed while land ran: land again"})));
                }
                Ok(None)
            };
            // The tree that passed must be the tree that lands: dir as it was written from head.
            let mut unchanged = || -> Result<Option<Value>> {
                let left = workspace::changed_since_written(&p.dir, &p.load()?);
                Ok((!left.is_empty()).then(|| gate::changed_during_test(change, left)))
            };
            gate::land(url, change, g, &p.dir, &req, &mut prepare, &mut unchanged)?
        }
        (None, Some(q)) => {
            // The server tests the change (in its test slots); a rejected change comes back
            // rebased onto main as it is now, so dir holds what to fix.
            let mut on_reject = || -> Result<Value> {
                let mut rb = client::call(url, "rebase", json!({"change": change}))?;
                native_change(&mut rb);
                Ok(match rb.get("conflicted") {
                    Some(paths) => json!({"conflicted": paths}),
                    None => json!({}),
                })
            };
            gate::land_queued(url, change, q, &req, &mut on_reject)?
        }
        (None, None) => client::call(url, "land", req)?,
    };
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
    let mut s = s;
    match status.as_str() {
        "conflict" => {
            s.decide = decisions(url, change);
            if v.get("conflicted").is_some() {
                v["advice"] = json!(CONFLICT_ADVICE);
            }
        }
        "undoes" => v["advice"] = json!(HOLD_ADVICE),
        "tests_failed" if v.get("conflicted").is_some() => {
            // Sent back by the land queue, then rebased onto a main that moved in the same lines.
            s.decide = decisions(url, change);
            v["conflict_advice"] =
                json!(CONFLICT_ADVICE.replacen("Not landed: main moved and the", "Main moved too: the", 1));
        }
        "tests_failed" | "untested" => {}
        "landed" | "already_on_main" => v["note"] = json!(LANDED_NOTE),
        _ => {}
    }
    s.report(&mut v);
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
