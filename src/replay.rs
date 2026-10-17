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
//!
//! Coordination (phase 3): `config` installs a treeyard.toml (merge drivers, continuous
//! rebase). With `intents`, each agent declares the paths of its commit at `open_change`; an
//! agent whose intent overlaps an open change of the wave is postponed to the next round (the
//! orchestrator policy "defer"), and then applies its commit's diff onto the fresh main. Every
//! run also counts the semantic guard's review tasks and the precision of early warnings.

use anyhow::{Context, Result, bail};
use rand::seq::SliceRandom;
use rand::{SeedableRng, rngs::StdRng};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::git;
use crate::merge::{Merged, merge3};
use crate::model::{Entry, Intent};
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
    /// A treeyard.toml copied into the data directory before the repo opens.
    pub config: Option<PathBuf>,
    /// Declare each commit's paths and postpone overlapping agents to a later round.
    pub intents: bool,
    /// `wave`: every agent starts on main as it is when its wave (or redo) starts and makes its
    /// commit's edit there (3-way). Default (false): the commit's true parent, as in phase 0.
    pub wave_base: bool,
}

struct MergeRecord {
    commit: String,
    /// Changeset ids; turned into git commits only for the git check.
    base: String,
    ours: String,
    theirs: String,
    /// The merge result changeset, if clean.
    landed: Option<String>,
    /// A merge driver resolved at least one path (git has no such driver).
    driver: bool,
    /// base/ours/theirs/landed are git commits (a land merge), else changeset ids (a rebase).
    is_commit: bool,
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
    /// Changes that conflicted on their first land attempt (the late conflicts).
    first_conflicts: usize,
    /// Of those, the ones `overlaps` reported before any change of the wave landed.
    flagged_early: usize,
    /// Changes `overlaps` flagged (shared path) and trial merges predicted to conflict.
    flagged: usize,
    predicted: usize,
    predicted_hit: usize,
    /// Pairs of open changes whose trial merge conflicted, and those where the one that landed
    /// second really conflicted on a predicted path. Only the second of a pair can conflict, so
    /// the per-change precision above counts the first one as a false alarm.
    pairs_predicted: usize,
    pairs_hit: usize,
    /// Conflicts continuous rebase put on a change before its owner called land.
    surfaced_by_rebase: usize,
    /// Intents: agents postponed to a later round, and rounds beyond the first.
    postponed: usize,
    extra_rounds: usize,
    apply_conflicts: usize,
    /// Commits whose edit changed nothing on the agent's base (wave base only).
    noop: usize,
}

fn rev_parse(dir: &Path, rev: &str) -> Result<String> {
    let out =
        Command::new("git").current_dir(dir).args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")]).output()?;
    if !out.status.success() {
        bail!("unknown revision {rev} in {}", dir.display());
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn ratio(a: usize, b: usize) -> Value {
    if b == 0 { Value::Null } else { json!((a as f64 / b as f64 * 1000.0).round() / 1000.0) }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// What happened when an agent took commit `i`.
enum Opened {
    /// An open change with the commit's edit.
    Change(String),
    /// The commit changes nothing.
    Empty,
    /// The declared intent overlaps an open change: the agent starts later.
    Overlap,
    /// The commit's edit does not apply to the agent's base (a prerequisite is missing).
    Blocked,
    /// The edit is already true on the base (e.g. a fix for code main does not have yet).
    Noop,
}

/// Opens a change for commit `i` and makes the commit's edit: on `base` when given (a revision),
/// else on the commit's true parent. On another base the edit (parent -> commit) is merged in
/// per path; if that conflicts the agent is `Blocked`, unless `force` (then the commit's
/// content wins). With `declare`, the commit's paths are the declared intent first.
#[allow(clippy::too_many_arguments)]
fn open_commit(
    r: &mut Inner,
    commits: &[String],
    from: &str,
    i: usize,
    agents: usize,
    base: Option<&str>,
    declare: bool,
    force: bool,
) -> Result<Opened> {
    let parent = if i == 0 { from } else { commits[i - 1].as_str() };
    let parent_cs = r.store.changeset_of_commit(parent)?.context("parent not imported")?;
    let base = match base {
        Some(b) => r.resolve_at(Some(b))?,
        None => parent_cs.clone(),
    };
    let original = r.store.changeset_of_commit(&commits[i])?.context("commit not imported")?;
    let cs = r.changeset(&original)?;
    if cs.changes.is_empty() {
        return Ok(Opened::Empty);
    }
    let agent = format!("agent-{:02}", i % agents);
    let declared = Intent {
        // A leading "/" anchors the glob at the root, so Cargo.toml is not crates/x/Cargo.toml.
        paths: if declare { cs.changes.keys().map(|p| format!("/{}", globset::escape(p))).collect() } else { vec![] },
        symbols: vec![],
    };
    let ch = r.open_change_with(&agent, &format!("replay {}", &commits[i][..12]), None, None, Some(&base), declared)?;
    if declare && !force && !r.intent_overlaps(&ch)?.is_empty() {
        r.abandon(&ch.id)?;
        return Ok(Opened::Overlap);
    }
    let entries: Vec<(String, Option<Entry>)> = if base == parent_cs {
        cs.changes.into_iter().collect()
    } else {
        let (pm, bm) = (r.manifest(&parent_cs)?, r.manifest(&base)?);
        let mut out = Vec::new();
        for (path, new) in cs.changes {
            match merge3(r, pm.get(&path), new.as_ref(), bm.get(&path))? {
                Merged::Clean(e) => out.push((path, e)),
                Merged::Conflict { .. } if force => out.push((path, new)),
                Merged::Conflict { .. } => {
                    r.abandon(&ch.id)?;
                    return Ok(Opened::Blocked);
                }
            }
        }
        out
    };
    let ch = r.apply_entries(&ch.id, entries, Some(cs.meta.intent))?;
    if ch.head == ch.base {
        r.abandon(&ch.id)?;
        return Ok(Opened::Noop);
    }
    Ok(Opened::Change(ch.id))
}

/// The paths commit `i` changes.
fn commit_paths(r: &Inner, commits: &[String], i: usize) -> Result<BTreeSet<String>> {
    let cs = r.store.changeset_of_commit(&commits[i])?.context("commit not imported")?;
    Ok(r.changeset(&cs)?.changes.into_keys().collect())
}

pub fn run(o: &Options) -> Result<Value> {
    let started = Instant::now();
    let from = rev_parse(&o.src, &o.from)?;
    let to = rev_parse(&o.src, &o.to)?;
    Repo::init(&o.data, &o.src, "main", Some(&from))?;
    let _ = std::fs::remove_file(o.data.join(crate::config::FILE));
    if let Some(c) = &o.config {
        std::fs::copy(c, o.data.join(crate::config::FILE)).with_context(|| format!("copying {}", c.display()))?;
    }
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

    let mut reviews_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut drivers_used: BTreeMap<String, usize> = BTreeMap::new();
    let mut forced_by_path: BTreeMap<String, usize> = BTreeMap::new();
    let mut absorbed: Vec<Value> = Vec::new();
    let mut absorbed_on_diverged = 0usize;
    let mut last_review = 0i64;
    let mut commit_of_change: HashMap<String, usize> = HashMap::new();
    let mut diverged_paths: BTreeMap<String, usize> = BTreeMap::new();
    let mut noop_examples: Vec<String> = Vec::new();
    let mut conflict_examples: Vec<Value> = Vec::new();

    let starts: Vec<usize> = (0..commits.len()).step_by(o.agents.max(1)).collect();
    for start in starts {
        let end = (start + o.agents).min(commits.len());
        waves += 1;
        repo.with(|r| {
            let mut forced_paths = BTreeSet::new();
            let mut candidates: Vec<usize> = (start..end).collect();
            let mut round = 0;
            let mut moved = true;
            while !candidates.is_empty() {
                round += 1;
                if round > 1 {
                    n.extra_rounds += 1;
                }
                let mut opened: Vec<(usize, String)> = Vec::new();
                let mut next = Vec::new();
                // Intents keep history order per path: a commit waits for an earlier waiting one.
                let mut waiting_paths: BTreeSet<String> = BTreeSet::new();
                let stuck = !moved;
                let count = candidates.len();
                for (k, i) in candidates.into_iter().enumerate() {
                    let base = match (o.stacked, opened.last(), round) {
                        (true, Some((_, prev)), _) => Some(prev.clone()),
                        (true, None, _) => Some("main".to_string()),
                        // Parent model: always the commit's true parent (by a later round every
                        // earlier commit on the same paths has landed, so the merge is clean).
                        (false, _, _) if !o.wave_base => None,
                        (false, _, _) => Some("main".to_string()),
                    };
                    let paths = commit_paths(r, &commits, i)?;
                    if o.intents && !paths.is_disjoint(&waiting_paths) {
                        n.postponed += 1;
                        waiting_paths.extend(paths);
                        next.push(i);
                        continue;
                    }
                    // A round that cannot move forces its first commit through.
                    let force = stuck && k == 0;
                    match open_commit(r, &commits, &from, i, o.agents, base.as_deref(), o.intents, force)? {
                        Opened::Change(id) => {
                            commit_of_change.insert(id.clone(), i);
                            opened.push((i, id));
                        }
                        Opened::Noop => {
                            n.noop += 1;
                            if noop_examples.len() < 20 {
                                noop_examples.push(commits[i].clone());
                            }
                        }
                        Opened::Overlap => {
                            n.postponed += 1;
                            waiting_paths.extend(paths);
                            next.push(i);
                        }
                        Opened::Blocked => {
                            n.apply_conflicts += 1;
                            if o.intents {
                                waiting_paths.extend(paths);
                            }
                            next.push(i);
                        }
                        Opened::Empty => n.empty += 1,
                    }
                }
                // What the server can tell before landing: open changes that share a path, and
                // the ones whose trial merge with another open change conflicts.
                let mut flagged = BTreeSet::new();
                let mut predicted = BTreeSet::new();
                let mut pairs: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
                for (_, id) in &opened {
                    let ch = r.change(id)?;
                    if !r.overlaps(&ch)?.is_empty() {
                        flagged.insert(id.clone());
                    }
                    for c in r.collisions(&ch)? {
                        if c["kind"] != "conflict" {
                            continue;
                        }
                        predicted.insert(id.clone());
                        let other = c["change"].as_str().unwrap_or_default().to_string();
                        let key = if *id < other { (id.clone(), other) } else { (other, id.clone()) };
                        let paths = c["files"].as_array().into_iter().flatten();
                        pairs.entry(key).or_default().extend(
                            paths
                                .filter(|f| f["kind"] == "conflict")
                                .filter_map(|f| f["path"].as_str().map(String::from)),
                        );
                    }
                }
                n.flagged += flagged.len();
                n.predicted += predicted.len();
                n.pairs_predicted += pairs.len();
                let mut conflicted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                opened.shuffle(&mut rng);
                let mut land = |r: &mut Inner, i: usize, id: &str, n: &mut Counts| -> Result<bool> {
                    let t = Instant::now();
                    let res = r.land(id).with_context(|| format!("landing the change for {}", commits[i]))?;
                    land_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                    if res["status"] == "waiting" {
                        n.waits += 1;
                        return Ok(false);
                    }
                    let count_drivers = |d: &Value, used: &mut BTreeMap<String, usize>| -> bool {
                        let list = d.as_array().cloned().unwrap_or_default();
                        for x in &list {
                            if x["error"].is_null() {
                                *used.entry(x["driver"].as_str().unwrap_or("?").to_string()).or_default() += 1;
                            }
                        }
                        list.iter().any(|x| x["error"].is_null())
                    };
                    let driver = count_drivers(&res["drivers"], &mut drivers_used);
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
                            driver,
                            is_commit: true,
                        });
                        if landed {
                            n.clean_merges += 1;
                        }
                    } else if landed {
                        n.no_merge += 1;
                    }
                    // Continuous rebase merged other open changes onto the new main.
                    for x in res["rebased"].as_array().cloned().unwrap_or_default() {
                        let driver = count_drivers(&x["drivers"], &mut drivers_used);
                        let clean = x["conflicts"].as_array().is_none_or(|c| c.is_empty());
                        // The intent names the replayed commit: "replay <12 hex>".
                        let label = r.change(x["change"].as_str().unwrap_or_default())?.intent;
                        merges.push(MergeRecord {
                            commit: label.trim_start_matches("replay ").to_string(),
                            base: x["base"].as_str().unwrap_or_default().into(),
                            ours: x["ours"].as_str().unwrap_or_default().into(),
                            theirs: x["theirs"].as_str().unwrap_or_default().into(),
                            landed: clean.then(|| x["head"].as_str().unwrap_or_default().to_string()),
                            driver,
                            is_commit: false,
                        });
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
                    if !r.change(&id)?.conflicts.is_empty() {
                        n.surfaced_by_rebase += 1;
                    }
                    if !land(r, i, &id, &mut n)? {
                        n.first_conflicts += 1;
                        if conflict_examples.len() < 20 {
                            let paths: Vec<String> = r.change(&id)?.conflicts.iter().map(|c| c.path.clone()).collect();
                            conflict_examples.push(json!({"commit": commits[i], "round": round, "paths": paths}));
                        }
                        if flagged.contains(&id) {
                            n.flagged_early += 1;
                        }
                        if predicted.contains(&id) {
                            n.predicted_hit += 1;
                        }
                        conflicted
                            .insert(id.clone(), r.change(&id)?.conflicts.iter().map(|c| c.path.clone()).collect());
                        r.abandon(&id)?;
                        deferred.push(i);
                    }
                }
                // A predicted pair is right when one of the two hit a conflict on a predicted path.
                n.pairs_hit += pairs
                    .iter()
                    .filter(|((x, y), paths)| {
                        [x, y].iter().any(|id| conflicted.get(*id).is_some_and(|c| !c.is_disjoint(paths)))
                    })
                    .count();
                // Redo thrown-away commits in history order until nothing moves, then force one.
                let redo_base = o.wave_base.then_some("main");
                deferred.sort_unstable();
                while !deferred.is_empty() {
                    let mut still = Vec::new();
                    let mut progress = false;
                    for i in deferred {
                        let id = match open_commit(r, &commits, &from, i, o.agents, redo_base, false, false)? {
                            Opened::Change(id) => id,
                            Opened::Blocked => {
                                still.push(i);
                                continue;
                            }
                            _ => {
                                n.noop += 1;
                                continue;
                            }
                        };
                        commit_of_change.insert(id.clone(), i);
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
                        let Opened::Change(id) = open_commit(r, &commits, &from, i, o.agents, redo_base, false, true)?
                        else {
                            n.noop += 1;
                            deferred = still;
                            continue;
                        };
                        commit_of_change.insert(id.clone(), i);
                        if !land(r, i, &id, &mut n)? {
                            let paths: Vec<String> = r.change(&id)?.conflicts.iter().map(|c| c.path.clone()).collect();
                            for p in paths {
                                r.resolve(&id, &p, "ours")?;
                                *forced_by_path.entry(p.clone()).or_default() += 1;
                                forced_paths.insert(p);
                            }
                            if land(r, i, &id, &mut n)? {
                                n.forced += 1;
                            }
                        }
                    }
                    deferred = still;
                }
                moved = next.len() < count;
                candidates = next;
            }
            let main = r.main()?;
            let expected = r.store.changeset_of_commit(&commits[end - 1])?.context("wave end not imported")?;
            let (got, want) = (r.manifest(&main)?, r.manifest(&expected)?);
            let mut paths: Vec<&String> =
                got.keys().chain(want.keys()).filter(|p| got.get(*p) != want.get(*p)).collect();
            paths.sort();
            paths.dedup();
            for p in &paths {
                *diverged_paths.entry((*p).clone()).or_default() += 1;
            }
            if paths.is_empty() {
                faithful_waves += 1;
            } else {
                // A forced "take ours" drops their side, so divergence on those paths is expected.
                let explained = paths.iter().all(|p| forced_paths.contains(*p));
                diverged.push(json!({
                    "wave_end": commits[end - 1], "paths": paths.iter().take(10).collect::<Vec<_>>(),
                    "path_count": paths.len(), "explained_by_forced": explained,
                }));
            }
            // Review tasks of the semantic guard opened during this wave.
            let wrong: BTreeSet<&String> = paths.iter().copied().collect();
            for (id, change, _, body) in r.store.reviews(None)? {
                if id <= last_review {
                    continue;
                }
                last_review = id;
                let kind = body["kind"].as_str().unwrap_or("?").to_string();
                *reviews_by_kind.entry(kind.clone()).or_default() += 1;
                if kind != "absorbed_edit" {
                    continue;
                }
                let path = body["detail"]["path"].as_str().unwrap_or_default().to_string();
                let on_wrong = wrong.contains(&path);
                if on_wrong {
                    absorbed_on_diverged += 1;
                }
                if absorbed.len() < 40 {
                    let i = commit_of_change.get(&change).copied();
                    let subject = match i {
                        Some(i) => {
                            let cs = r.store.changeset_of_commit(&commits[i])?.unwrap_or_default();
                            r.changeset(&cs).map(|c| c.meta.intent).unwrap_or_default()
                        }
                        None => String::new(),
                    };
                    absorbed.push(json!({
                        "commit": i.map(|i| commits[i].clone()), "subject": subject, "path": path,
                        "whole_file": body["detail"]["whole_file"], "path_wrong_at_wave_end": on_wrong,
                    }));
                }
            }
            Ok(())
        })?;
    }

    let mut git_check = json!(null);
    if o.check_git {
        let (mut agree, mut differs, mut server_clean_git_conflict, mut server_conflict_git_clean, mut both_conflict) =
            (0, Vec::new(), Vec::new(), 0, 0);
        let mut driver_resolved = 0;
        let gd_s = gd.to_string_lossy().to_string();
        for m in &merges {
            let (base, ours, theirs, landed) = if m.is_commit {
                (m.base.clone(), m.ours.clone(), m.theirs.clone(), m.landed.clone())
            } else {
                repo.with(|r| {
                    let landed = m.landed.as_deref().map(|l| r.commit_for(l)).transpose()?;
                    Ok((r.commit_for(&m.base)?, r.commit_for(&m.ours)?, r.commit_for(&m.theirs)?, landed))
                })?
            };
            let out = Command::new("git")
                .args(["--git-dir", &gd_s, "-c", "merge.renames=false", "-c", "diff.renames=false", "merge-tree"])
                .args(["--write-tree", &format!("--merge-base={base}"), &theirs, &ours])
                .output()?;
            let git_clean = out.status.success();
            let git_tree = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string();
            match (&landed, git_clean) {
                (Some(landed), true) => {
                    let tree = git::run(Some(&gd), &["rev-parse", &format!("{landed}^{{tree}}")])?;
                    if tree.trim() == git_tree {
                        agree += 1;
                    } else {
                        differs.push(json!({"commit": m.commit, "landed": landed, "driver": m.driver}));
                    }
                }
                (Some(_), false) if m.driver => driver_resolved += 1,
                (Some(landed), false) => server_clean_git_conflict.push(json!({"commit": m.commit, "landed": landed})),
                (None, true) => server_conflict_git_clean += 1,
                (None, false) => both_conflict += 1,
            }
        }
        git_check = json!({
            "merges_checked": merges.len(), "same_tree": agree, "tree_differs": differs,
            "server_clean_git_conflict": server_clean_git_conflict, "driver_resolved_git_conflict": driver_resolved,
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
        "first_conflicts": n.first_conflicts, "flagged_early": n.flagged_early,
        "warnings": {
            "flagged_by_overlaps": n.flagged, "flagged_and_conflicted": n.flagged_early,
            "overlaps_precision": ratio(n.flagged_early, n.flagged),
            "predicted_by_trial_merge": n.predicted, "predicted_and_conflicted": n.predicted_hit,
            "trial_merge_precision": ratio(n.predicted_hit, n.predicted), "trial_merge_recall": ratio(n.predicted_hit, n.first_conflicts),
            "pairs_predicted": n.pairs_predicted, "pairs_hit": n.pairs_hit,
            "trial_merge_pair_precision": ratio(n.pairs_hit, n.pairs_predicted),
        },
        "coordination": {
            "config": o.config, "intents": o.intents, "postponed": n.postponed, "extra_rounds": n.extra_rounds,
            "apply_conflicts": n.apply_conflicts, "noop_on_base": n.noop, "noop_examples": noop_examples, "surfaced_by_rebase": n.surfaced_by_rebase,
            "drivers": drivers_used, "reviews": reviews_by_kind,
            "absorbed_on_path_wrong_at_wave_end": absorbed_on_diverged, "absorbed_examples": absorbed,
        },
        "first_conflict_examples": conflict_examples, "thrown_away": n.conflicts, "forced_paths": forced_by_path, "wave_base": o.wave_base,
        "diverged_paths": diverged_paths,
        "land_ms": {"p50": percentile(&land_ms, 0.5), "p95": percentile(&land_ms, 0.95), "max": land_ms.last().copied().unwrap_or(0.0)},
        "git_check": git_check, "fsck_ok": new_fsck_errors.is_empty(), "fsck_new_errors": new_fsck_errors, "diverged_unexplained": diverged.iter().filter(|d| d["explained_by_forced"] == false).count(), "diverged": diverged,
        "seconds": started.elapsed().as_secs_f64(),
    }))
}
