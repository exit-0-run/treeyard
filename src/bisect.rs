//! Server-side bisect: binary search over the first-parent changesets between a good and a
//! bad revision. Each step moves one temporary materialisation to the next changeset (only
//! changed files are written; files the test edited are restored) and runs a test command
//! from `treeyard.toml`. Tests run outside the repo lock, so agents keep landing.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::landing::run_cmd;
use crate::model::{MODE_EXEC, MODE_GITLINK, MODE_LINK};
use crate::repo::{Inner, Repo};
use crate::search::diff_manifests;

pub struct Args {
    pub good: String,
    pub bad: Option<String>,
    /// A named command of `[bisect.commands]`; None = the default.
    pub command: Option<String>,
    /// Also test the two ends (good must pass, bad must fail). Default true.
    pub verify: bool,
}

/// One file write of a step: the path and its mode and data, or None to delete it.
type Write = (String, Option<(u32, Vec<u8>)>);

/// A temporary checkout that moves between changesets.
struct Work {
    dir: PathBuf,
    at: Option<String>,
    /// (size, mtime ns) of each file as written, to find files a test changed.
    stamps: HashMap<String, (u64, i128)>,
}

fn stamp(p: &Path) -> Option<(u64, i128)> {
    let m = std::fs::symlink_metadata(p).ok()?;
    Some((m.len(), i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec())))
}

impl Inner {
    /// The writes that move a checkout from `from` (None = empty dir) to `to`, plus the
    /// restore of `dirty` paths.
    fn bisect_writes(&mut self, from: Option<&str>, to: &str, dirty: &[String]) -> Result<Vec<Write>> {
        let target = self.manifest(to)?;
        let mut paths: Vec<String> = match from {
            None => target.keys().cloned().collect(),
            Some(f) => diff_manifests(&self.manifest(f)?, &target).into_iter().map(|(p, _)| p).collect(),
        };
        paths.extend(dirty.iter().cloned());
        paths.sort();
        paths.dedup();
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            match target.get(&p) {
                Some(e) if e.mode != MODE_GITLINK => {
                    let data = self.blob(&e.blob)?;
                    out.push((p, Some((e.mode, data))));
                }
                Some(_) => {}
                None => out.push((p, None)),
            }
        }
        Ok(out)
    }
}

impl Work {
    fn apply(&mut self, writes: Vec<Write>) -> Result<()> {
        for (p, w) in writes {
            let dest = crate::safety::dest(&self.dir, &p)?;
            let old_mtime = std::fs::symlink_metadata(&dest).ok().and_then(|m| m.modified().ok());
            if old_mtime.is_some() {
                std::fs::remove_file(&dest).with_context(|| format!("removing {}", dest.display()))?;
            }
            self.stamps.remove(&p);
            let Some((mode, data)) = w else { continue };
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if mode == MODE_LINK {
                std::os::unix::fs::symlink(String::from_utf8_lossy(&data).as_ref(), &dest)?;
            } else {
                std::fs::write(&dest, &data)?;
                if mode == MODE_EXEC {
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
                }
                // Build tools and Python's .pyc check compare mtime in whole seconds (and
                // size): a rewrite in the same second with the same size would look unchanged.
                if let Some(old) = old_mtime {
                    let f = std::fs::File::options().write(true).open(&dest)?;
                    let now = f.metadata()?.modified()?;
                    let floor = old + Duration::from_secs(1);
                    if now < floor {
                        f.set_modified(floor)?;
                    }
                }
            }
            if let Some(s) = stamp(&dest) {
                self.stamps.insert(p, s);
            }
        }
        Ok(())
    }

    fn dirty(&self) -> Vec<String> {
        self.stamps
            .iter()
            .filter(|(p, s)| stamp(&self.dir.join(p)).as_ref() != Some(s))
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn move_to(&mut self, repo: &Repo, cs: &str) -> Result<()> {
        let dirty = self.dirty();
        let from = self.at.clone();
        let writes = repo.with(|r| r.bisect_writes(from.as_deref(), cs, &dirty))?;
        self.apply(writes)?;
        self.at = Some(cs.to_string());
        Ok(())
    }
}

fn describe(r: &mut Inner, cs: &str) -> Result<Value> {
    let m = r.changeset(cs)?.meta;
    Ok(json!({
        "changeset": cs, "commit": r.store.commit_of(cs)?, "change": m.change, "agent": m.agent, "model": m.model,
        "session": m.session, "intent": m.intent, "message": m.message, "time": m.time,
    }))
}

/// Runs the bisect; returns the first bad changeset and every step.
pub fn run(repo: &Repo, a: &Args) -> Result<Value> {
    let started = Instant::now();
    let (chain, cmd, timeout, scratch) = repo.with(|r| {
        let bad = r.resolve_at(a.bad.as_deref())?;
        let good = r.resolve_at(Some(&a.good))?;
        let mut chain = Vec::new();
        let mut cur = Some(bad.clone());
        while let Some(id) = cur {
            if id == good {
                break;
            }
            cur = r.changeset(&id)?.parents.first().cloned();
            if cur.is_none() {
                bail!("good {} is not a first-parent ancestor of bad {}", &good[..12], &bad[..12]);
            }
            chain.push(id);
        }
        chain.push(good);
        chain.reverse(); // chain[0] = good, last = bad
        let cmd = r.cfg.bisect.command(a.command.as_deref(), &r.cfg.landing)?;
        let timeout = Duration::from_secs(r.cfg.bisect.timeout_s.unwrap_or(r.cfg.landing.timeout_s));
        Ok((chain, cmd, timeout, r.scratch()))
    })?;
    if chain.len() < 2 {
        bail!("good and bad are the same changeset");
    }
    let dir = scratch.join(format!("bisect-{:08x}", rand::random::<u32>()));
    std::fs::create_dir_all(&dir)?;
    let mut work = Work { dir: dir.clone(), at: None, stamps: HashMap::new() };
    let mut steps = Vec::new();
    let mut outputs: HashMap<usize, String> = HashMap::new();
    let result = (|| -> Result<Value> {
        let mut test = |i: usize, work: &mut Work| -> Result<bool> {
            let cs = &chain[i];
            work.move_to(repo, cs)?;
            let out = run_cmd(&cmd, &work.dir, timeout, &[("TREEYARD_CHANGESET", cs)])?;
            let commit = repo.with(|r| r.store.commit_of(cs))?;
            steps.push(json!({"changeset": cs, "commit": commit, "ok": out.ok, "seconds": out.seconds}));
            if !out.ok {
                outputs.insert(i, out.output);
            }
            Ok(out.ok)
        };
        let (mut lo, mut hi) = (0usize, chain.len() - 1);
        if a.verify {
            if test(hi, &mut work)? {
                bail!("the test passes on bad {}: nothing to bisect", &chain[hi][..12]);
            }
            if !test(lo, &mut work)? {
                bail!("the test fails on good {}: pick an older good revision", &chain[lo][..12]);
            }
        }
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if test(mid, &mut work)? {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let first = repo.with(|r| describe(r, &chain[hi]))?;
        Ok(json!({"first_bad": first, "last_good": chain[lo], "candidates": chain.len() - 1}))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let mut v = result?;
    let hi = chain.iter().position(|c| v["first_bad"]["changeset"] == c.as_str()).unwrap_or(0);
    v["output"] = json!(outputs.get(&hi));
    v["command"] = json!(a.command.as_deref().unwrap_or("default"));
    v["tests"] = json!(steps.len());
    v["steps"] = Value::Array(steps);
    v["seconds"] = json!(started.elapsed().as_secs_f64());
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewrite_moves_mtime_on_by_at_least_one_second() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = Work { dir: tmp.path().to_path_buf(), at: None, stamps: HashMap::new() };
        w.apply(vec![("a.py".into(), Some((crate::model::MODE_FILE, b"x = (1, 2)\n".to_vec())))]).unwrap();
        let first = std::fs::metadata(tmp.path().join("a.py")).unwrap().modified().unwrap();
        // Same size, same second: without the bump a .pyc or make would see no change.
        w.apply(vec![("a.py".into(), Some((crate::model::MODE_FILE, b"x = (2, 1)\n".to_vec())))]).unwrap();
        let second = std::fs::metadata(tmp.path().join("a.py")).unwrap().modified().unwrap();
        assert!(second.duration_since(first).unwrap() >= Duration::from_secs(1));
        assert_eq!(std::fs::read(tmp.path().join("a.py")).unwrap(), b"x = (2, 1)\n");
        // A test that edits a tracked file is found and undone on the next move.
        std::fs::write(tmp.path().join("a.py"), "edited by the test\n").unwrap();
        assert_eq!(w.dirty(), vec!["a.py".to_string()]);
    }
}
