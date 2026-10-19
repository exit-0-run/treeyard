//! Landing load test, the shape of gitai's graft-bench: `--agents` agents (threads) each land
//! `--changes` changes on one branch. Every change adds its own file; a share of them
//! (`--conflict-rate`) also appends a line at the end of one shared file, so two such changes
//! made from the same base conflict. No conflict resolver: a conflicting change fails.
//!
//!   landbench git --remote BARE.git --work DIR [opts]   plain git: clone, commit, push; a rejected
//!                                                        push runs `pull --rebase` and retries
//!   landbench treeyard --url URL [opts]                 open_change, apply, land (direct mode)
//!
//! Prints one JSON object: changes, landed, failed, conflicts, attempts_per_land, lands_per_minute,
//! p50/p95/p99 latency of landed changes (from the start of the change to the end of its land).
//! `bench/landbench.sh` runs both against the same seed repository.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::time::Instant;

struct Opts {
    agents: usize,
    changes: usize,
    conflict_rate: f64,
    seed: u64,
    max_attempts: usize,
}

/// Deterministic per change: does change `j` of agent `i` touch the shared file?
fn shared(o: &Opts, i: usize, j: usize) -> bool {
    let mut x = o.seed ^ ((i as u64) << 32) ^ j as u64 ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x % 10_000) as f64 / 10_000.0 < o.conflict_rate
}

#[derive(Default)]
struct Sample {
    landed: bool,
    conflict: bool,
    attempts: usize,
    ms: f64,
    error: Option<String>,
}

fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "gc.auto")
        .env("GIT_CONFIG_VALUE_1", "0")
        .output()?)
}

fn ok(out: std::process::Output, what: &str) -> Result<()> {
    if !out.status.success() {
        bail!("{what}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Runs a fetch, retrying failures: with many agents pushing to one local bare repository, git's
/// upload-pack fails now and then ("not our ref", "bad object", "bad pack header") while a ref or
/// pack is being written. An agent would simply fetch again.
fn git_retry(dir: &Path, args: &[&str], what: &str) -> Result<()> {
    let mut last = String::new();
    for k in 0..30u64 {
        let out = git(dir, args)?;
        if out.status.success() {
            return Ok(());
        }
        last = String::from_utf8_lossy(&out.stderr).trim().to_string();
        std::thread::sleep(std::time::Duration::from_millis(20 * (k + 1)));
    }
    bail!("{what}: failed 30 times: {last}")
}

fn git_agent(o: &Opts, i: usize, remote: &Path, work: &Path) -> Result<Vec<Sample>> {
    let dir = work.join(format!("a{i:03}"));
    ok(git(work, &["clone", "-q", &remote.to_string_lossy(), &dir.to_string_lossy()])?, "clone")?;
    ok(git(&dir, &["config", "user.email", format!("a{i}@bench").as_str()])?, "config")?;
    ok(git(&dir, &["config", "user.name", format!("agent {i}").as_str()])?, "config")?;
    let mut out = Vec::new();
    for j in 0..o.changes {
        let t0 = Instant::now();
        let mut s = Sample::default();
        // Start from the current tip, like an agent that begins a task.
        git_retry(&dir, &["fetch", "-q", "origin"], "fetch")?;
        ok(git(&dir, &["reset", "-q", "--hard", "origin/main"])?, "reset")?;
        let f = dir.join(format!("agents/a{i:03}/c{j:02}.txt"));
        std::fs::create_dir_all(f.parent().unwrap())?;
        std::fs::write(&f, format!("agent {i} change {j}\n"))?;
        if shared(o, i, j) {
            let p = dir.join("shared.txt");
            let mut text = std::fs::read_to_string(&p)?;
            text.push_str(&format!("agent {i} change {j}\n"));
            std::fs::write(&p, text)?;
        }
        ok(git(&dir, &["add", "-A"])?, "add")?;
        ok(git(&dir, &["commit", "-qm", &format!("agent {i} change {j}")])?, "commit")?;
        loop {
            s.attempts += 1;
            if git(&dir, &["push", "-q", "origin", "HEAD:main"])?.status.success() {
                s.landed = true;
                break;
            }
            if s.attempts >= o.max_attempts {
                s.error = Some(format!("gave up after {} attempts", s.attempts));
                break;
            }
            git_retry(&dir, &["fetch", "-q", "origin"], "fetch")?;
            if !git(&dir, &["rebase", "-q", "origin/main"])?.status.success() {
                let _ = git(&dir, &["rebase", "--abort"]);
                s.conflict = true;
                s.error = Some("merge conflict".into());
                break;
            }
        }
        s.ms = t0.elapsed().as_secs_f64() * 1000.0;
        out.push(s);
    }
    Ok(out)
}

fn ty_agent(o: &Opts, i: usize, url: &str) -> Result<Vec<Sample>> {
    let call = |op: &str, p: Value| treeyard::client::call(url, op, p);
    let agent = format!("a{i:03}");
    let mut out = Vec::new();
    for j in 0..o.changes {
        let t0 = Instant::now();
        let mut s = Sample { attempts: 1, ..Default::default() };
        let ch = call("open_change", json!({"agent": agent, "intent": format!("change {j}")}))?;
        let id = ch["id"].as_str().context("open_change: no id")?.to_string();
        let base = ch["base"].as_str().context("open_change: no base")?.to_string();
        let mut edits =
            vec![json!({"path": format!("agents/{agent}/c{j:02}.txt"), "content": format!("agent {i} change {j}\n")})];
        if shared(o, i, j) {
            let text =
                call("read", json!({"path": "shared.txt", "at": base}))?["content"].as_str().unwrap_or("").to_string();
            edits.push(json!({"path": "shared.txt", "content": format!("{text}agent {i} change {j}\n")}));
        }
        call("apply", json!({"change": id, "seen": base, "edits": edits}))?;
        let r = call("land", json!({"change": id}))?;
        match r["status"].as_str() {
            Some("landed") | Some("already_on_main") => s.landed = true,
            Some("conflict") => {
                s.conflict = true;
                s.error = Some("merge conflict".into());
                let _ = call("abandon", json!({"change": id}));
            }
            other => s.error = Some(format!("land: {other:?} {r}")),
        }
        s.ms = t0.elapsed().as_secs_f64() * 1000.0;
        out.push(s);
    }
    Ok(out)
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    (sorted[k.clamp(1, sorted.len()) - 1] * 10.0).round() / 10.0
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_default();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|p| args.get(p + 1)).cloned();
    let o = Arc::new(Opts {
        agents: get("--agents").map_or(Ok(50), |v| v.parse())?,
        changes: get("--changes").map_or(Ok(5), |v| v.parse())?,
        conflict_rate: get("--conflict-rate").map_or(Ok(0.2), |v| v.parse())?,
        seed: get("--seed").map_or(Ok(1), |v| v.parse())?,
        max_attempts: get("--max-attempts").map_or(Ok(20), |v| v.parse())?,
    });
    let (remote, work, url) = (get("--remote").map(PathBuf::from), get("--work").map(PathBuf::from), get("--url"));
    if !matches!(mode.as_str(), "git" | "treeyard") {
        bail!(
            "usage: landbench git --remote BARE --work DIR | treeyard --url URL [--agents N --changes N --conflict-rate F --seed N]"
        );
    }
    if mode == "git" {
        std::fs::create_dir_all(work.as_ref().context("--work")?)?;
    }
    let start = Arc::new(Barrier::new(o.agents + 1));
    let mut hs = Vec::new();
    for i in 0..o.agents {
        let (o, start, mode, remote, work, url) =
            (o.clone(), start.clone(), mode.clone(), remote.clone(), work.clone(), url.clone());
        hs.push(std::thread::spawn(move || -> Result<Vec<Sample>> {
            start.wait();
            if mode == "git" {
                git_agent(&o, i, remote.as_deref().context("--remote")?, work.as_deref().context("--work")?)
            } else {
                ty_agent(&o, i, url.as_deref().context("--url")?)
            }
        }));
    }
    start.wait();
    let t0 = Instant::now();
    let mut samples = Vec::new();
    let mut errors = Vec::new();
    for h in hs {
        match h.join().map_err(|_| anyhow::anyhow!("agent thread panicked"))? {
            Ok(s) => samples.extend(s),
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    let landed: Vec<&Sample> = samples.iter().filter(|s| s.landed).collect();
    let mut ms: Vec<f64> = landed.iter().map(|s| s.ms).collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let attempts: usize = samples.iter().map(|s| s.attempts).sum();
    let mut kinds: Vec<String> = samples.iter().filter_map(|s| s.error.clone()).collect();
    kinds.sort();
    kinds.dedup();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": mode, "agents": o.agents, "changes_per_agent": o.changes, "conflict_rate": o.conflict_rate,
            "changes": samples.len(), "landed": landed.len(), "failed": samples.len() - landed.len(),
            "conflicts": samples.iter().filter(|s| s.conflict).count(),
            "attempts_per_land": if landed.is_empty() { 0.0 } else { (attempts as f64 / landed.len() as f64 * 100.0).round() / 100.0 },
            "max_attempts": samples.iter().map(|s| s.attempts).max().unwrap_or(0),
            "duration_seconds": (secs * 100.0).round() / 100.0,
            "lands_per_minute": (landed.len() as f64 / secs * 60.0).round(),
            "p50_ms": pct(&ms, 50.0), "p95_ms": pct(&ms, 95.0), "p99_ms": pct(&ms, 99.0),
            "failure_kinds": kinds, "agent_errors": errors,
        }))?
    );
    Ok(())
}
