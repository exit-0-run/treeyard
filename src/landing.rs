//! Landing modes with a test command (`[landing]` in treeyard.toml):
//!
//! - `queue`: `land` puts the change in a queue. A step takes up to `batch` changes, rebases
//!   them one on top of the other (main does not move), tests the top of the stack, and lands
//!   the whole batch when it passes. A failing batch is bisected: the longest passing prefix
//!   lands, the first failing change goes back to its owner as a review task.
//! - `optimistic`: `land` moves main at once. A step tests main; when it fails, the changesets
//!   since the last green main are bisected and the first failing one is reverted through the
//!   normal revert operation.
//!
//! Tests run in a temporary copy of the tree, outside the repo lock, so agents keep working.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Mode;
use crate::model::{Change, ChangeState, MODE_EXEC, MODE_GITLINK, MODE_LINK, Manifest};
use crate::repo::{Inner, RebaseReport, Repo};

/// Writes every file of `m` under `dir` (submodules are skipped).
pub fn materialize(r: &mut Inner, m: &Manifest, dir: &Path) -> Result<()> {
    crate::safety::check_tree(m.keys().map(String::as_str))?;
    for (path, e) in m.iter() {
        if e.mode == MODE_GITLINK {
            continue;
        }
        let dest = crate::safety::dest(dir, path)?;
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        let data = r.blob(&e.blob)?;
        if e.mode == MODE_LINK {
            std::os::unix::fs::symlink(String::from_utf8_lossy(&data).as_ref(), &dest)?;
            continue;
        }
        std::fs::write(&dest, &data)?;
        if e.mode == MODE_EXEC {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

pub struct Outcome {
    pub ok: bool,
    pub output: String,
    pub seconds: f64,
}

/// Runs `sh -c cmd` in `dir` with a timeout; keeps the last 4000 bytes of the output.
pub fn run_cmd(cmd: &str, dir: &Path, timeout: Duration, env: &[(&str, &str)]) -> Result<Outcome> {
    let started = Instant::now();
    let log = dir.with_extension("log");
    let file = std::fs::File::create(&log)?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file)
        .spawn()
        .with_context(|| format!("starting {cmd:?}"))?;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = std::fs::read(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    let tail = &text[text.len().saturating_sub(4000)..];
    let mut output = String::from_utf8_lossy(tail).into_owned();
    if status.is_none() {
        output.push_str(&format!("\n(treeyard: killed after {} s)", timeout.as_secs()));
    }
    Ok(Outcome { ok: status.is_some_and(|s| s.success()), output, seconds: started.elapsed().as_secs_f64() })
}

/// Materializes changeset `cs` and runs the repo's test command on it.
fn test(repo: &Repo, cs: &str) -> Result<Outcome> {
    let (dir, cmd, timeout) = repo.with(|r| {
        let cmd = r.cfg.landing.test.clone().context("landing.test is not set in treeyard.toml")?;
        let dir = r.scratch().join(format!("test-{}-{:08x}", &cs[..12], rand::random::<u32>()));
        std::fs::create_dir_all(&dir)?;
        let m = r.manifest(cs)?;
        materialize(r, &m, &dir)?;
        Ok((dir, cmd, Duration::from_secs(r.cfg.landing.timeout_s)))
    })?;
    let started = std::time::Instant::now();
    let out = run_cmd(&cmd, &dir, timeout, &[("TREEYARD_CHANGESET", cs)]);
    let _ = std::fs::remove_dir_all(&dir);
    // One event per test run, so a slow landing loop shows where its time goes.
    let (ok, seconds) = match &out {
        Ok(o) => (Some(o.ok), o.seconds),
        Err(_) => (None, started.elapsed().as_secs_f64()),
    };
    let _ = repo.with(|r| {
        r.emit("test_run", &[], &[], json!({"changeset": cs, "ok": ok, "seconds": (seconds * 10.0).round() / 10.0}));
        Ok(())
    });
    out
}

struct Item {
    before: Change,
    after: Change,
    report: RebaseReport,
}

struct Speculation {
    main: String,
    items: Vec<Item>,
    already: Vec<(Change, Change, RebaseReport)>,
    notes: Vec<Value>,
}

impl Inner {
    /// What happened to a change after `land`, for an agent that waits on it. `verdict` is
    /// `passed` (the tests of the landing mode covered it), `pending` (not tested yet), `queued`,
    /// `rejected` (the queue's tests failed; it is still open), `reverted` (optimistic mode: bisect
    /// found it broke main and reverted it), `broke_main` (the same, but the revert conflicted:
    /// it stays on main until its agent fixes forward), `main_red` (tested, but main is red from
    /// `red_change`, which could not be reverted), `not_tested` (direct mode), `open` or `abandoned`.
    /// `output` holds the failing test output for `rejected`, `reverted` and `broke_main`.
    pub fn land_status(&mut self, id: &str) -> Result<Value> {
        let ch = self.change(id)?;
        let reviews = self.reviews_view(None, Some(id))?["reviews"].as_array().cloned().unwrap_or_default();
        let last_of = |kinds: &[&str]| {
            reviews.iter().rev().find(|r| kinds.contains(&r["kind"].as_str().unwrap_or_default())).cloned()
        };
        let mode = self.cfg.landing.mode;
        let (verdict, review) = match ch.state {
            ChangeState::Abandoned => ("abandoned", None),
            ChangeState::Open if self.store.queue()?.iter().any(|q| q == id) => ("queued", None),
            ChangeState::Open => match last_of(&["tests_failed"]) {
                Some(r) => ("rejected", Some(r)),
                None => ("open", None),
            },
            ChangeState::Landed => {
                let landed = ch.landed_as.clone().context("landed change without landed_as")?;
                let covered = |r: &mut Self, at: Option<String>| -> Result<bool> {
                    Ok(match at {
                        Some(g) => g == landed || r.is_ancestor(&landed, &g)?,
                        None => false,
                    })
                };
                match last_of(&["reverted_by_bisect", "revert_failed"]) {
                    Some(r) if r["kind"] == "reverted_by_bisect" => ("reverted", Some(r)),
                    // Not revertable: it stays on main until a fix lands and main is green again.
                    Some(r) => {
                        let green = self.store.get_ref("green")?;
                        if covered(self, green)? { ("passed", None) } else { ("broke_main", Some(r)) }
                    }
                    None => match mode {
                        Mode::Direct => ("not_tested", None),
                        Mode::Queue => ("passed", None),
                        Mode::Optimistic => {
                            let green = self.store.get_ref("green")?;
                            let tested = self.store.get_ref("tested")?;
                            if covered(self, green)? {
                                ("passed", None)
                            } else if self.store.get_ref("red")?.is_some() && covered(self, tested)? {
                                // Tested, but main is red from another change that could not be
                                // reverted: this change is not to blame and nothing waits on it.
                                ("main_red", None)
                            } else {
                                ("pending", None)
                            }
                        }
                    },
                }
            }
        };
        let output = review.as_ref().map(|r| r["detail"]["output"].clone()).unwrap_or(Value::Null);
        let red = if verdict == "main_red" { self.store.get_ref("red")? } else { None };
        let red_change = match &red {
            Some(cs) => self.changeset(cs)?.meta.change.clone(),
            None => None,
        };
        Ok(json!({
            "change": id, "state": ch.state.as_str(), "mode": mode, "verdict": verdict,
            "landed_as": ch.landed_as, "output": output, "review": review.map(|r| r["id"].clone()),
            "red_since": red, "red_change": red_change,
        }))
    }

    /// Queue mode: validates the change and appends it to the landing queue.
    pub fn enqueue(&mut self, id: &str) -> Result<Value> {
        let ch = self.change(id)?;
        if ch.state != ChangeState::Open {
            bail!("change {id} is {}", ch.state.as_str());
        }
        if !ch.conflicts.is_empty() {
            return Ok(json!({"status": "conflict", "change": self.change_view(&ch)?}));
        }
        if ch.head == ch.base && ch.predecessors.is_empty() {
            bail!("change {id} is empty: apply edits before landing");
        }
        let position = self.store.enqueue(id)?;
        self.emit("queued", &[id], &[&ch.agent], json!({"change": id, "position": position}));
        Ok(json!({"status": "queued", "change": id, "position": position}))
    }

    pub fn queue_view(&mut self) -> Result<Value> {
        let ids = self.store.queue()?;
        let mut out = Vec::new();
        for id in ids {
            let ch = self.change(&id)?;
            out.push(json!({"change": id, "agent": ch.agent, "intent": ch.intent}));
        }
        Ok(json!({"mode": self.cfg.landing.mode, "batch": self.cfg.landing.batch, "queue": out}))
    }

    /// Rebases up to `batch` queued changes one on top of the other, without moving main.
    fn speculate(&mut self) -> Result<Speculation> {
        let main = self.main()?;
        let mut spec = Speculation { main: main.clone(), items: vec![], already: vec![], notes: vec![] };
        let mut top = main.clone();
        let mut in_batch: BTreeSet<String> = BTreeSet::new();
        for id in self.store.queue()? {
            if spec.items.len() >= self.cfg.landing.batch.max(1) {
                break;
            }
            let before = self.change(&id)?;
            if before.state != ChangeState::Open || !before.conflicts.is_empty() {
                self.store.dequeue(&id)?;
                spec.notes.push(json!({"change": id, "status": "dropped", "why": "not open or has conflicts"}));
                continue;
            }
            if let Some(dep) = self.changeset(&before.base)?.meta.change.filter(|d| *d != id)
                && self.change(&dep)?.state == ChangeState::Open
                && !in_batch.contains(&dep)
            {
                spec.notes.push(json!({"change": id, "status": "waiting", "waiting_for": dep}));
                continue;
            }
            let mut after = before.clone();
            let report = self.rebase_onto(&mut after, &top)?;
            if !after.conflicts.is_empty() {
                // A conflict with a batch-mate waits for the next batch; one with main is real.
                let mut real = before.clone();
                let report = self.rebase_onto(&mut real, &main)?;
                if real.conflicts.is_empty() {
                    spec.notes.push(json!({"change": id, "status": "deferred", "why": "conflicts with an earlier change in the batch"}));
                    continue;
                }
                self.store.dequeue(&id)?;
                self.store.put_change(&real)?;
                self.guard(&real, &report)?;
                let paths: Vec<&String> = real.conflicts.iter().map(|c| &c.path).collect();
                self.emit("conflict", &[&id], &[&real.agent], json!({"change": id, "paths": paths, "cause": "queue"}));
                spec.notes.push(json!({"change": id, "status": "conflict", "paths": paths}));
                continue;
            }
            if after.head == after.base {
                spec.already.push((before, after, report));
                continue;
            }
            top = after.head.clone();
            in_batch.insert(id.clone());
            spec.items.push(Item { before, after, report });
        }
        Ok(spec)
    }

    /// Queue mode with `parallel` > 1, after a batch failed: each queued change (up to `batch`)
    /// rebased alone onto main, to be tested side by side. Returns (change, head alone, head now).
    fn preflight(&mut self) -> Result<Vec<(String, String, String)>> {
        let main = self.main()?;
        let mut out = Vec::new();
        for id in self.store.queue()?.into_iter().take(self.cfg.landing.batch.max(1)) {
            let ch = self.change(&id)?;
            if ch.state != ChangeState::Open || !ch.conflicts.is_empty() {
                continue;
            }
            let mut alone = ch.clone();
            self.rebase_onto(&mut alone, &main)?;
            if alone.conflicts.is_empty() && alone.head != alone.base {
                out.push((id, alone.head, ch.head));
            }
        }
        Ok(out)
    }

    /// Sends a change whose own preflight test failed back to its owner, unless it moved meanwhile.
    fn reject_preflight(&mut self, id: &str, head: &str, out: &Outcome) -> Result<Option<Value>> {
        let ch = self.change(id)?;
        if ch.state != ChangeState::Open || ch.head != head || !self.store.queue()?.iter().any(|q| q == id) {
            return Ok(None);
        }
        self.store.dequeue(id)?;
        let review = self.add_review(&ch, "tests_failed", json!({"output": out.output, "seconds": out.seconds}))?;
        self.emit("rejected", &[id], &[&ch.agent], json!({"change": id, "review": review, "preflight": true}));
        Ok(Some(json!({"change": id, "review": review})))
    }

    /// Lands the first `passing` items of a tested speculation, if nothing moved meanwhile.
    fn commit_speculation(
        &mut self,
        spec: Speculation,
        passing: usize,
        culprit: Option<&Outcome>,
        tests: Vec<Value>,
    ) -> Result<Value> {
        let unchanged = |r: &Inner, c: &Change| -> Result<bool> {
            let now = r.change(&c.id)?;
            Ok(now.state == ChangeState::Open && now.head == c.head && now.base == c.base)
        };
        if self.main()? != spec.main
            || !spec.items.iter().map(|i| unchanged(self, &i.before)).collect::<Result<Vec<_>>>()?.iter().all(|x| *x)
        {
            return Ok(
                json!({"status": "stale", "why": "main or a queued change moved during the test; the step runs again", "tests": tests}),
            );
        }
        let mut landed = Vec::new();
        for (before, mut after, report) in spec.already {
            if !unchanged(self, &before)? {
                continue;
            }
            after.state = ChangeState::Landed;
            after.landed_as = Some(spec.main.clone());
            self.store.put_change(&after)?;
            self.store.dequeue(&after.id)?;
            self.guard(&after, &report)?;
            let review = self.add_review(&after, "already_on_main", json!({"absorbed": report.absorbed}))?;
            self.forget(&after.id);
            self.emit(
                "landed",
                &[&after.id],
                &[&after.agent],
                json!({"change": after.id, "status": "already_on_main", "review": review}),
            );
            landed.push(json!({"change": after.id, "status": "already_on_main"}));
        }
        let mut items = spec.items.into_iter();
        let mut new_main = None;
        for item in items.by_ref().take(passing) {
            let mut ch = item.after;
            ch.state = ChangeState::Landed;
            ch.landed_as = Some(ch.head.clone());
            self.store.put_change(&ch)?;
            self.store.dequeue(&ch.id)?;
            self.guard(&ch, &item.report)?;
            self.forget(&ch.id);
            self.emit("landed", &[&ch.id], &[&ch.agent], json!({"change": ch.id, "changeset": ch.head, "queue": true}));
            landed.push(json!({"change": ch.id, "status": "landed", "changeset": ch.head}));
            new_main = Some(ch.head.clone());
        }
        if let Some(m) = &new_main {
            let commit = self.commit_for(m)?;
            self.store.set_ref("main", m)?;
            self.odb.update_ref(&format!("refs/heads/{}", self.branch), &commit)?;
            for l in &landed {
                if let (Some(id), Some(cs)) = (l["change"].as_str(), l["changeset"].as_str()) {
                    let c = self.commit_for(cs)?;
                    self.notes_after_land(id, cs, &c);
                }
            }
        }
        let mut rejected = Value::Null;
        if let (Some(out), Some(item)) = (culprit, items.next()) {
            let ch = item.before;
            self.store.dequeue(&ch.id)?;
            let review = self.add_review(&ch, "tests_failed", json!({"output": out.output, "seconds": out.seconds}))?;
            self.emit("rejected", &[&ch.id], &[&ch.agent], json!({"change": ch.id, "review": review}));
            rejected = json!({"change": ch.id, "review": review});
        }
        let rebased = if new_main.is_some() { self.continuous_rebase()? } else { vec![] };
        Ok(
            json!({"status": "done", "landed": landed, "rejected": rejected, "main": self.main()?, "tests": tests, "rebased": rebased}),
        )
    }

    /// First-parent main changesets after `green` (exclusive) up to main, oldest first.
    fn untested(&mut self, green: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut cur = self.main()?;
        while cur != green {
            out.push(cur.clone());
            match self.changeset(&cur)?.parents.first() {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        out.reverse();
        Ok(out)
    }

    /// The import changeset (the root of main).
    fn root(&mut self) -> Result<String> {
        let mut cur = self.main()?;
        while let Some(p) = self.changeset(&cur)?.parents.first() {
            cur = p.clone();
        }
        Ok(cur)
    }
}

/// Runs `test` on each changeset, `parallel` at a time.
fn test_all(repo: &Repo, heads: &[String], parallel: usize) -> Result<Vec<Outcome>> {
    let mut outs = Vec::new();
    for chunk in heads.chunks(parallel.max(1)) {
        let res: Vec<Result<Outcome>> = std::thread::scope(|sc| {
            let hs: Vec<_> = chunk.iter().map(|h| sc.spawn(move || test(repo, h))).collect();
            hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("test thread panicked")))).collect()
        });
        for r in res {
            outs.push(r?);
        }
    }
    Ok(outs)
}

/// One queue step: speculate, test the batch, land it when it passes. A failing batch with
/// `parallel` > 1 tests its changes alone on main, side by side, and sends the failing ones back,
/// so one broken change costs one more round and no longer holds up its batch-mates; with
/// `parallel` = 1 (or when every change passes alone) the first failing prefix is searched.
pub fn queue_step(repo: &Repo) -> Result<Value> {
    let parallel = repo.with(|r| Ok(r.cfg.landing.parallel.max(1)))?;
    let spec = repo.with(|r| r.speculate())?;
    if spec.items.is_empty() {
        let notes = spec.notes.clone();
        let out = repo.with(|r| r.commit_speculation(spec, 0, None, vec![]))?;
        return Ok(json!({"status": "idle", "notes": notes, "landed": out["landed"]}));
    }
    let heads: Vec<String> = spec.items.iter().map(|i| i.after.head.clone()).collect();
    let mut tests = Vec::new();
    let mut run = |k: usize| -> Result<Outcome> {
        let out = test(repo, &heads[k - 1])?;
        tests.push(json!({"changes": k, "changeset": heads[k - 1], "ok": out.ok, "seconds": out.seconds}));
        Ok(out)
    };
    let n = heads.len();
    let top = run(n)?;
    if !top.ok && parallel > 1 && n > 1 {
        // The batch failed: test each of its changes alone on main, side by side, and send the
        // failing ones back. The rest are batched again by the next step. (Testing every change
        // alone first, before the batch, cost one run per change: 44 runs for 50 changes in the
        // wide50e cloud run, as many as the git queue.)
        let pre = repo.with(|r| r.preflight())?;
        let alone_heads: Vec<String> = pre.iter().map(|p| p.1.clone()).collect();
        let outs = test_all(repo, &alone_heads, parallel)?;
        let mut alone = Vec::new();
        let mut rejected = Vec::new();
        for ((id, cs, head), out) in pre.iter().zip(&outs) {
            alone.push(json!({"change": id, "changeset": cs, "ok": out.ok, "seconds": out.seconds}));
            if !out.ok
                && let Some(r) = repo.with(|r| r.reject_preflight(id, head, out))?
            {
                rejected.push(r);
            }
        }
        if !rejected.is_empty() {
            return Ok(json!({"status": "done", "landed": [], "rejected": rejected, "tests": tests, "alone": alone}));
        }
        // Every change passes alone: they break each other. Find the first failing prefix below.
    }
    let (passing, culprit) = if top.ok {
        (n, None)
    } else if parallel > 1 && n > 2 {
        // Test the other prefixes `parallel` at a time instead of bisecting one run after another.
        let outs = test_all(repo, &heads[..n - 1], parallel)?;
        for (k, out) in outs.iter().enumerate() {
            tests.push(json!({"changes": k + 1, "changeset": heads[k], "ok": out.ok, "seconds": out.seconds}));
        }
        let mut outs: Vec<Outcome> = outs;
        outs.push(top);
        let first_bad = outs.iter().position(|o| !o.ok).unwrap_or(n - 1);
        (first_bad, Some(outs.swap_remove(first_bad)))
    } else {
        // Prefix 0 (main) passes, prefix n fails: find the first failing prefix.
        let (mut lo, mut hi, mut bad) = (0, n, top);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            let out = run(mid)?;
            if out.ok {
                lo = mid;
            } else {
                hi = mid;
                bad = out;
            }
        }
        (lo, Some(bad))
    };
    let notes = spec.notes.clone();
    let mut out = repo.with(|r| r.commit_speculation(spec, passing, culprit.as_ref(), tests))?;
    out["notes"] = json!(notes);
    Ok(out)
}

/// One optimistic step: test main if it is new; on failure bisect since the last green main
/// and revert the first failing changeset.
pub fn optimistic_step(repo: &Repo) -> Result<Value> {
    let (main, green, chain) = repo.with(|r| {
        let main = r.main()?;
        if r.store.get_ref("tested")?.as_deref() == Some(main.as_str()) {
            return Ok((main, String::new(), vec![]));
        }
        let green = match r.store.get_ref("green")? {
            Some(g) => g,
            None => r.root()?,
        };
        let chain = r.untested(&green)?;
        Ok((main, green, chain))
    })?;
    if chain.is_empty() {
        return Ok(json!({"status": "idle", "main": main}));
    }
    let mut tests = Vec::new();
    let mut run = |cs: &str| -> Result<Outcome> {
        let out = test(repo, cs)?;
        tests.push(json!({"changeset": cs, "ok": out.ok, "seconds": out.seconds}));
        Ok(out)
    };
    let top = run(&main)?;
    if top.ok {
        repo.with(|r| {
            r.store.set_ref("green", &main)?;
            r.store.set_ref("tested", &main)?;
            r.store.delete_ref("red")?;
            r.emit("test_passed", &[], &[], json!({"changeset": main}));
            Ok(())
        })?;
        return Ok(json!({"status": "green", "main": main, "tests": tests}));
    }
    // Main is still red from a culprit that could not be reverted: bisecting again would find
    // it again, so wait for a fix (one test per new main) instead.
    if let Some(red) = repo.with(|r| r.store.get_ref("red"))? {
        // The culprit's agent fixes forward: when its newest changeset since the culprit passes,
        // main was green there, so the next step bisects only what landed after it.
        let fix = repo.with(|r| {
            let agent = r.changeset(&red)?.meta.agent;
            let pos = chain.iter().position(|c| *c == red);
            let mut last = None;
            for cs in chain.iter().skip(pos.map_or(0, |p| p + 1)) {
                if r.changeset(cs)?.meta.agent == agent {
                    last = Some(cs.clone());
                }
            }
            Ok(last.filter(|f| *f != main && r.store.get_ref("red_checked").ok().flatten().as_ref() != Some(f)))
        })?;
        if let Some(f) = fix {
            let out = run(&f)?;
            let ok = out.ok;
            repo.with(|r| {
                r.store.set_ref("red_checked", &f)?;
                if ok {
                    r.store.set_ref("green", &f)?;
                    r.store.delete_ref("red")?;
                    r.emit("test_passed", &[], &[], json!({"changeset": f, "fixed": red}));
                }
                Ok(())
            })?;
            if ok {
                return Ok(json!({"status": "fixed", "main": main, "fixed_at": f, "tests": tests}));
            }
        }
        repo.with(|r| {
            r.store.set_ref("tested", &main)?;
            r.emit("test_failed", &[], &[], json!({"main": main, "culprit": red, "known": true, "output": top.output}));
            Ok(())
        })?;
        return Ok(json!({"status": "red", "main": main, "culprit": red, "known": true, "tests": tests}));
    }
    // chain[i] for i < lo pass (lo = 0: the green base), chain[hi] fails.
    let (mut lo, mut hi, mut bad) = (0usize, chain.len() - 1, top);
    while hi > lo {
        let mid = (lo + hi) / 2;
        let out = run(&chain[mid])?;
        if out.ok {
            lo = mid + 1;
        } else {
            hi = mid;
            bad = out;
        }
    }
    let culprit = chain[hi].clone();
    let good = if hi == 0 { green.clone() } else { chain[hi - 1].clone() };
    repo.with(|r| {
        r.store.set_ref("tested", &main)?;
        r.store.set_ref("green", &good)?;
        let cs = r.changeset(&culprit)?;
        let owner = match cs.meta.change.as_deref() {
            Some(c) => Some(r.change(c)?),
            None => None,
        };
        r.emit(
            "test_failed",
            &owner.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            &[&cs.meta.agent],
            json!({"main": main, "culprit": culprit, "output": bad.output}),
        );
        let reverted = r.revert(&culprit, "treeyard-bisect")?;
        let status = reverted["status"].as_str().unwrap_or_default().to_string();
        if let Some(ch) = &owner {
            let kind = if status == "landed" { "reverted_by_bisect" } else { "revert_failed" };
            r.add_review(ch, kind, json!({"culprit": culprit, "output": bad.output, "revert": reverted}))?;
        }
        if status == "landed" {
            r.emit("reverted", &[], &[&cs.meta.agent], json!({"culprit": culprit, "revert": reverted["changeset"]}));
        } else {
            r.store.set_ref("red", &culprit)?;
        }
        Ok(json!({"status": "red", "main": main, "culprit": culprit, "revert": reverted, "tests": tests}))
    })
}

/// Background worker for `serve`: runs queue or optimistic steps while there is work.
pub fn spawn_worker(repo: Arc<Repo>) -> Option<std::thread::JoinHandle<()>> {
    let mode = repo.with(|r| Ok(r.cfg.landing.mode)).ok()?;
    if mode == Mode::Direct {
        return None;
    }
    Some(std::thread::spawn(move || {
        loop {
            let step = match mode {
                Mode::Queue => match repo.with(|r| Ok(r.store.queue()?.is_empty())) {
                    Ok(true) => Ok(json!({"status": "idle"})),
                    _ => queue_step(&repo),
                },
                _ => optimistic_step(&repo),
            };
            match step {
                Ok(v) if v["status"] == "idle" => std::thread::sleep(Duration::from_millis(200)),
                Ok(v) => eprintln!("treeyard landing: {}", v["status"]),
                Err(e) => {
                    eprintln!("treeyard landing step failed: {e:#}");
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }))
}

/// Path helper for tests and drivers: a fresh dir under the repo's scratch area.
pub fn scratch_dir(base: &Path, tag: &str) -> Result<PathBuf> {
    let dir = base.join(format!("{tag}-{:08x}", rand::random::<u32>()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
