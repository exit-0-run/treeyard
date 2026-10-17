//! Replay of real history: consecutive first-parent commits are split into waves of N agents.
//! Each agent starts on the commit's true parent (a stale base for all but one) and applies
//! that commit; the wave lands in random order, so the server has to merge. A conflicted
//! change is thrown away and redone later. Ground truth: after every wave, main must hold the
//! tree of the wave's last original commit, and every server merge is replayed with
//! `git merge-tree` on the same three commits.
//!
//! With `stacked`, each agent instead builds on the previous agent's change in the wave (as a
//! stack), so the server makes it wait until that change lands: no semantic reordering, at the
//! price of serial landing.

use anyhow::{Context, Result, bail};
use rand::seq::SliceRandom;
use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::git;
use crate::repo::{Inner, Repo};

pub struct Options {
    pub src: PathBuf,
    pub from: String,
    pub to: String,
    pub agents: usize,
    pub limit: Option<usize>,
    pub seed: u64,
    pub data: PathBuf,
    pub check_git: bool,
    pub stacked: bool,
}

struct MergeRecord {
    commit: String,
    base: String,
    ours: String,
    theirs: String,
    landed: Option<String>,
}

#[derive(Default)]
struct Counts {
    landed: usize,
    no_merge: usize,
    clean_merges: usize,
    conflicts: usize,
    redone: usize,
    forced: usize,
    empty: usize,
    already_on_main: usize,
    waits: usize,
}

fn rev_parse(dir: &Path, rev: &str) -> Result<String> {
    let out =
        Command::new("git").current_dir(dir).args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")]).output()?;
    if !out.status.success() {
        bail!("unknown revision {rev} in {}", dir.display());
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Opens a change for commit `i` and applies the commit: on `base` when given (a revision), else
/// on the commit's true parent. `None` if the commit is empty.
fn open(
    r: &mut Inner,
    commits: &[String],
    from: &str,
    i: usize,
    agents: usize,
    base: Option<&str>,
) -> Result<Option<String>> {
    let base = match base {
        Some(b) => r.resolve_at(Some(b))?,
        None => {
            let parent = if i == 0 { from } else { commits[i - 1].as_str() };
            r.store.changeset_of_commit(parent)?.context("parent not imported")?
        }
    };
    let original = r.store.changeset_of_commit(&commits[i])?.context("commit not imported")?;
    let cs = r.changeset(&original)?;
    if cs.changes.is_empty() {
        return Ok(None);
    }
    let agent = format!("agent-{:02}", i % agents);
    let ch = r.open_change(&agent, &format!("replay {}", &commits[i][..12]), None, None, Some(&base))?;
    r.apply_entries(&ch.id, cs.changes.into_iter().collect(), Some(cs.meta.intent))?;
    Ok(Some(ch.id))
}

pub fn run(o: &Options) -> Result<Value> {
    let started = Instant::now();
    let from = rev_parse(&o.src, &o.from)?;
    let to = rev_parse(&o.src, &o.to)?;
    Repo::init(&o.data, &o.src, "main", Some(&from))?;
    let repo = Repo::open(&o.data)?;
    let gd = o.data.join("git");
    let range = format!("{from}..{to}");
    let mut commits: Vec<String> = git::run(Some(&gd), &["rev-list", "--first-parent", "--reverse", &range])?
        .lines()
        .map(str::to_string)
        .collect();
    if let Some(l) = o.limit {
        commits.truncate(l);
    }
    if commits.is_empty() {
        bail!("no commits in {range}");
    }
    repo.with(|r| commits.iter().try_for_each(|c| r.import_commit(c).map(|_| ())))?;

    let mut rng = StdRng::seed_from_u64(o.seed);
    let mut n = Counts::default();
    let mut merges: Vec<MergeRecord> = Vec::new();
    let mut land_ms: Vec<f64> = Vec::new();
    let (mut waves, mut faithful_waves) = (0, 0);
    let mut diverged: Vec<Value> = Vec::new();

    let starts: Vec<usize> = (0..commits.len()).step_by(o.agents.max(1)).collect();
    for start in starts {
        let end = (start + o.agents).min(commits.len());
        waves += 1;
        repo.with(|r| {
            let mut forced_paths = std::collections::BTreeSet::new();
            let mut opened: Vec<(usize, String)> = Vec::new();
            for i in start..end {
                let base = match (o.stacked, opened.last()) {
                    (true, Some((_, prev))) => Some(prev.clone()),
                    (true, None) => Some("main".to_string()),
                    (false, _) => None,
                };
                match open(r, &commits, &from, i, o.agents, base.as_deref())? {
                    Some(id) => opened.push((i, id)),
                    None => n.empty += 1,
                }
            }
            opened.shuffle(&mut rng);
            let mut land = |r: &mut Inner, i: usize, id: &str, n: &mut Counts| -> Result<bool> {
                let t = Instant::now();
                let res = r.land(id)?;
                land_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                if res["status"] == "waiting" {
                    n.waits += 1;
                    return Ok(false);
                }
                let already = res["status"] == "already_on_main";
                let landed = res["status"] == "landed" || already;
                if already {
                    n.already_on_main += 1;
                }
                if let Value::Object(m) = &res["merged_from"] {
                    merges.push(MergeRecord {
                        commit: commits[i].clone(),
                        base: m["base"].as_str().unwrap_or_default().into(),
                        ours: m["ours"].as_str().unwrap_or_default().into(),
                        theirs: m["theirs"].as_str().unwrap_or_default().into(),
                        landed: landed.then(|| res["commit"].as_str().unwrap_or_default().to_string()),
                    });
                    if landed {
                        n.clean_merges += 1;
                    }
                } else if landed {
                    n.no_merge += 1;
                }
                if landed {
                    n.landed += 1;
                } else {
                    n.conflicts += 1;
                }
                Ok(landed)
            };
            if o.stacked {
                // Land in random order; waiting changes are retried, never thrown away.
                let mut pending = std::mem::take(&mut opened);
                while !pending.is_empty() {
                    let before = pending.len();
                    let mut still = Vec::new();
                    for (i, id) in pending {
                        if !land(r, i, &id, &mut n)? {
                            if !r.change(&id)?.conflicts.is_empty() {
                                bail!("stacked change for {} conflicted", commits[i]);
                            }
                            still.push((i, id));
                        }
                    }
                    if still.len() == before {
                        bail!("stacked wave made no progress");
                    }
                    pending = still;
                }
            }
            let mut deferred = Vec::new();
            for (i, id) in opened {
                if !land(r, i, &id, &mut n)? {
                    r.abandon(&id)?;
                    deferred.push(i);
                }
            }
            // Redo thrown-away commits in history order until nothing moves, then force one.
            deferred.sort_unstable();
            while !deferred.is_empty() {
                let mut still = Vec::new();
                let mut progress = false;
                for i in deferred {
                    let Some(id) = open(r, &commits, &from, i, o.agents, None)? else { continue };
                    n.redone += 1;
                    if land(r, i, &id, &mut n)? {
                        progress = true;
                    } else {
                        r.abandon(&id)?;
                        still.push(i);
                    }
                }
                if !progress && !still.is_empty() {
                    let i = still.remove(0);
                    let id = open(r, &commits, &from, i, o.agents, None)?.context("forced commit is empty")?;
                    land(r, i, &id, &mut n)?;
                    let paths: Vec<String> = r.change(&id)?.conflicts.iter().map(|c| c.path.clone()).collect();
                    for p in paths {
                        r.resolve(&id, &p, "ours")?;
                        forced_paths.insert(p);
                    }
                    if land(r, i, &id, &mut n)? {
                        n.forced += 1;
                    }
                }
                deferred = still;
            }
            let main = r.main()?;
            let expected = r.store.changeset_of_commit(&commits[end - 1])?.context("wave end not imported")?;
            let (got, want) = (r.manifest(&main)?, r.manifest(&expected)?);
            if got == want {
                faithful_waves += 1;
            } else {
                let mut paths: Vec<&String> =
                    got.keys().chain(want.keys()).filter(|p| got.get(*p) != want.get(*p)).collect();
                paths.sort();
                paths.dedup();
                // A forced "take ours" drops their side, so divergence on those paths is expected.
                let explained = paths.iter().all(|p| forced_paths.contains(*p));
                diverged.push(json!({
                    "wave_end": commits[end - 1], "paths": paths.iter().take(10).collect::<Vec<_>>(),
                    "path_count": paths.len(), "explained_by_forced": explained,
                }));
            }
            Ok(())
        })?;
    }

    let mut git_check = json!(null);
    if o.check_git {
        let (mut agree, mut differs, mut server_clean_git_conflict, mut server_conflict_git_clean, mut both_conflict) =
            (0, Vec::new(), Vec::new(), 0, 0);
        let gd_s = gd.to_string_lossy().to_string();
        for m in &merges {
            let out = Command::new("git")
                .args(["--git-dir", &gd_s, "-c", "merge.renames=false", "-c", "diff.renames=false", "merge-tree"])
                .args(["--write-tree", &format!("--merge-base={}", m.base), &m.theirs, &m.ours])
                .output()?;
            let git_clean = out.status.success();
            let git_tree = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string();
            match (&m.landed, git_clean) {
                (Some(landed), true) => {
                    let tree = git::run(Some(&gd), &["rev-parse", &format!("{landed}^{{tree}}")])?;
                    if tree.trim() == git_tree {
                        agree += 1;
                    } else {
                        differs.push(json!({"commit": m.commit, "landed": landed}));
                    }
                }
                (Some(landed), false) => server_clean_git_conflict.push(json!({"commit": m.commit, "landed": landed})),
                (None, true) => server_conflict_git_clean += 1,
                (None, false) => both_conflict += 1,
            }
        }
        git_check = json!({
            "merges_checked": merges.len(), "same_tree": agree, "tree_differs": differs,
            "server_clean_git_conflict": server_clean_git_conflict,
            "server_conflict_git_clean": server_conflict_git_clean, "both_conflict": both_conflict,
        });
    }

    // Old histories carry their own fsck errors; only errors the source does not have count.
    let fsck_errors = |dir: &Path, bare: bool| -> Result<std::collections::BTreeSet<String>> {
        let mut cmd = Command::new("git");
        if bare {
            cmd.arg("--git-dir").arg(dir);
        } else {
            cmd.current_dir(dir);
        }
        let out = cmd.args(["fsck", "--strict", "--no-progress", "--no-dangling"]).output()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        Ok(text.lines().filter(|l| l.starts_with("error")).map(str::to_string).collect())
    };
    let source_errors = fsck_errors(&o.src, false)?;
    let new_fsck_errors: Vec<String> = fsck_errors(&gd, true)?.difference(&source_errors).cloned().collect();
    let final_equal = repo.with(|r| {
        let main = r.main()?;
        let last = r.store.changeset_of_commit(commits.last().unwrap())?.context("last not imported")?;
        Ok(r.manifest(&main)? == r.manifest(&last)?)
    })?;
    let files = repo.with(|r| {
        let main = r.main()?;
        Ok(r.manifest(&main)?.len())
    })?;
    land_ms.sort_by(f64::total_cmp);
    Ok(json!({
        "source": o.src, "from": from, "to": to, "commits": commits.len(), "agents": o.agents, "seed": o.seed,
        "files_at_end": files, "waves": waves, "faithful_waves": faithful_waves, "final_tree_equal": final_equal,
        "landed": n.landed, "landed_without_merge": n.no_merge, "clean_merges": n.clean_merges,
        "conflicts": n.conflicts, "redone": n.redone, "forced": n.forced, "empty_commits": n.empty, "already_on_main": n.already_on_main, "waits": n.waits, "stacked": o.stacked,
        "land_ms": {"p50": percentile(&land_ms, 0.5), "p95": percentile(&land_ms, 0.95), "max": land_ms.last().copied().unwrap_or(0.0)},
        "git_check": git_check, "fsck_ok": new_fsck_errors.is_empty(), "fsck_new_errors": new_fsck_errors, "diverged_unexplained": diverged.iter().filter(|d| d["explained_by_forced"] == false).count(), "diverged": diverged,
        "seconds": started.elapsed().as_secs_f64(),
    }))
}
