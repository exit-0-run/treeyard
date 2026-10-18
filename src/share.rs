//! `treeyard share`: the client side of a directory that several agents edit at once. `join`
//! opens one change per agent on the share base and records it in `DIR/.treeyard/share.json`;
//! `sync` sends the files that differ from the base (`assign`) and then writes each agent's own
//! hunks into its change (`apply_assigned`). Ownership lives on the server (src/hunks.rs).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::client;
use crate::git::object_id;
use crate::model::MODE_GITLINK;
use crate::workspace::PRIVATE;

/// What `DIR/.treeyard/share.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Joined {
    pub share: String,
    pub base: String,
    /// Agent name -> change id.
    pub agents: BTreeMap<String, String>,
}

fn file(dir: &Path) -> PathBuf {
    dir.join(PRIVATE).join("share.json")
}

pub fn load(dir: &Path) -> Result<Option<Joined>> {
    match fs::read(file(dir)) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b).context("reading .treeyard/share.json")?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn save(dir: &Path, j: &Joined) -> Result<()> {
    fs::create_dir_all(dir.join(PRIVATE))?;
    fs::write(file(dir), serde_json::to_vec_pretty(j)?)?;
    Ok(())
}

fn str_of(v: &Value, key: &str) -> Result<String> {
    v[key].as_str().map(str::to_string).with_context(|| format!("reply has no {key}: {v}"))
}

/// Opens a change for `agent` on the share base (main when this is the first agent).
pub fn join(url: &str, dir: &Path, agent: &str, intent: &str, paths: &[String], symbols: &[String]) -> Result<Value> {
    let dir = dir.canonicalize().with_context(|| format!("no directory {}", dir.display()))?;
    let mut j = match load(&dir)? {
        Some(j) => j,
        None => Joined {
            share: dir.to_string_lossy().into_owned(),
            base: str_of(&client::call(url, "status", json!({}))?, "main")?,
            agents: BTreeMap::new(),
        },
    };
    if let Some(ch) = j.agents.get(agent) {
        bail!("agent {agent} already joined {} as change {ch}", dir.display());
    }
    let v = client::call(
        url,
        "open_change",
        json!({"agent": agent, "intent": intent, "base": j.base, "paths": paths, "symbols": symbols}),
    )?;
    j.agents.insert(agent.to_string(), str_of(&v, "id")?);
    save(&dir, &j)?;
    Ok(json!({"share": j.share, "base": j.base, "agent": agent, "change": v["id"], "overlaps": v["overlaps"]}))
}

/// The files of `dir` that differ from the base tree, as `assign` takes them. Tracked paths are
/// read directly (a `.gitignore` never hides them); new files only when no `.gitignore` excludes
/// them. Binary files and symlinks are left out (listed in `skipped`).
pub fn changed_files(url: &str, dir: &Path, base: &str) -> Result<(Vec<Value>, Vec<String>)> {
    let tree = client::call(url, "tree", json!({"at": base}))?;
    let mut tracked = BTreeSet::new();
    let (mut files, mut skipped) = (Vec::new(), Vec::new());
    let mut add = |rel: String, data: Vec<u8>, files: &mut Vec<Value>| match String::from_utf8(data) {
        Ok(t) if !t.as_bytes().iter().take(8000).any(|b| *b == 0) => files.push(json!({"path": rel, "content": t})),
        _ => skipped.push(rel),
    };
    for row in tree["entries"].as_array().context("tree reply has no entries")? {
        let (Some(path), Some(mode), Some(blob)) = (row[0].as_str(), row[1].as_u64(), row[2].as_str()) else {
            continue;
        };
        tracked.insert(path.to_string());
        if mode as u32 == MODE_GITLINK {
            continue;
        }
        let p = dir.join(path);
        match fs::symlink_metadata(&p) {
            Ok(md) if md.is_file() => {
                let data = fs::read(&p)?;
                if object_id("blob", &data) != blob {
                    add(path.to_string(), data, &mut files);
                }
            }
            Ok(md) if md.file_type().is_symlink() => {}
            _ => files.push(json!({"path": path, "delete": true})),
        }
    }
    let walker = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .ignore(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()) && (n == ".git" || n == PRIVATE))
        })
        .build();
    for e in walker {
        let e = e?;
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = e.path().strip_prefix(dir)?.to_string_lossy().into_owned();
        if !tracked.contains(&rel) {
            add(rel, fs::read(e.path())?, &mut files);
        }
    }
    Ok((files, skipped))
}

/// Sends the directory to `assign`, then `apply_assigned` for every joined agent. With
/// `rebase`, first rebases every agent's change onto main and moves the share there.
pub fn sync(url: &str, dir: &Path, rebase: bool) -> Result<Value> {
    let dir = dir.canonicalize().with_context(|| format!("no directory {}", dir.display()))?;
    let mut j =
        load(&dir)?.with_context(|| format!("{} has no share: run treeyard share join first", dir.display()))?;
    if rebase {
        for ch in j.agents.values() {
            if client::call(url, "change", json!({"change": ch}))?["state"] != "open" {
                continue;
            }
            let v = client::call(url, "rebase", json!({"change": ch}))?;
            if v["conflicts"].as_array().is_some_and(|c| !c.is_empty()) {
                bail!("rebase of {ch} left conflicts; resolve them first");
            }
        }
        j.base = str_of(&client::call(url, "status", json!({}))?, "main")?;
    }
    let (files, skipped) = changed_files(url, &dir, &j.base)?;
    let changes: Vec<&String> = j.agents.values().collect();
    let assigned =
        client::call(url, "assign", json!({"share": j.share, "base": j.base, "changes": changes, "files": files}))?;
    save(&dir, &j)?;
    let mut applied = serde_json::Map::new();
    for (agent, ch) in &j.agents {
        let state = assigned["changes"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["change"] == ch.as_str()))
            .map(|c| c["state"].clone());
        if state.is_some_and(|s| s != "open") {
            continue;
        }
        let v = client::call(url, "apply_assigned", json!({"share": j.share, "change": ch}))?;
        applied.insert(
            agent.clone(),
            json!({"change": ch, "applied": v["applied"], "held": v["held"], "paths": v["paths"], "conflicts": v["conflicts"]}),
        );
    }
    Ok(json!({
        "share": j.share, "base": j.base, "agents": applied,
        "unassigned": assigned["unassigned"], "skipped": skipped.into_iter().chain(
            assigned["skipped"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string))
        ).collect::<Vec<_>>(),
    }))
}

/// `claim` for a joined agent.
pub fn claim(url: &str, dir: &Path, agent: &str, hunks: &[String]) -> Result<Value> {
    let dir = dir.canonicalize()?;
    let j = load(&dir)?.with_context(|| format!("{} has no share", dir.display()))?;
    let ch = j.agents.get(agent).with_context(|| format!("agent {agent} has not joined {}", dir.display()))?;
    client::call(url, "claim", json!({"share": j.share, "change": ch, "hunks": hunks}))
}
