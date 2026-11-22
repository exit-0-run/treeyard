//! The land test on the agent's side (`[coordination] land_test`): the stdio bridge rebases the
//! change onto main, brings the change's directory to exactly that tree, runs the command there
//! with a timeout, and lands only with a proof (`tested`: command, main, head) that the server
//! checks under its lock together with the land. When main moved in between, the server answers
//! `main_moved` and the bridge goes round again, at most `ROUNDS` times.
//!
//! The land queue on the agent's side (`[landing] mode = "queue"`): the server tests the change
//! in its own test slots; `land_queued` waits for the verdict and answers like a direct land.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::client;

/// Bytes of the test output a `tests_failed` reply carries (its end, from a line start).
pub const OUTPUT_CAP: usize = 4096;
/// Rebase-test-land rounds before the bridge gives up because main keeps moving.
pub const ROUNDS: usize = 3;

pub const FAILED_ADVICE: &str = "Not landed: the land test failed on your change rebased onto main (dir holds that \
tree). Fix it in dir, run the tests there, and land again.";
const MOVING_ADVICE: &str = "Not landed: main moved during every test round, so no tested result could land. \
Land again.";

/// The land test of a repository, from a `change` reply (`land_test`), if it has one.
pub struct Gate {
    pub command: String,
    pub timeout: Duration,
}

impl Gate {
    pub fn of(change: &Value) -> Option<Gate> {
        let t = change.get("land_test")?;
        Some(Gate {
            command: t["command"].as_str()?.to_string(),
            timeout: Duration::from_secs(t["timeout_s"].as_u64().unwrap_or(crate::config::LAND_TEST_TIMEOUT_S)),
        })
    }
}

/// One run of the command.
pub struct Run {
    pub passed: bool,
    pub timed_out: bool,
    pub code: Option<i32>,
    /// The end of stdout and stderr together, at most `OUTPUT_CAP` bytes.
    pub tail: String,
    pub bytes: u64,
    pub seconds: f64,
}

/// The shell around the land test: the test runs in the background of a group leader that also
/// watches its stdin, a pipe from the bridge that the bridge never writes. When the bridge dies
/// for any reason (SIGKILL included) the pipe closes and the watcher kills the whole group, so no
/// test outlives the bridge that started it. The exit status is the test's.
const GUARD: &str = r#"exec 3<&0 </dev/null
sh -c "$1" 3<&- &
c=$!
{ read -r _ <&3; kill -KILL 0; } &
w=$!
wait "$c"
s=$?
kill "$w" 2>/dev/null
exit "$s""#;

/// Runs `command` with `sh -c` in `dir`, output to `log`, killed (with its process group) after
/// `timeout`, or as soon as this process dies (`GUARD`).
pub fn run(command: &str, dir: &Path, timeout: Duration, log: &Path) -> Result<Run> {
    let t0 = Instant::now();
    let out = std::fs::File::create(log).with_context(|| format!("create {}", log.display()))?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(GUARD)
        .arg("treeyard-land-test")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(out.try_clone()?)
        .stderr(out)
        .env("TREEYARD_LAND_TEST", "1")
        .process_group(0)
        .spawn()
        .with_context(|| format!("run land test {command:?}"))?;
    let mut timed_out = false;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if t0.elapsed() >= timeout {
            timed_out = true;
            // SAFETY: kill has no memory preconditions; the group is the child's own.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // The pipe closes here: a watcher still alive kills what is left of the group.
    drop(child.stdin.take());
    let mut f = std::fs::File::open(log)?;
    let bytes = f.metadata()?.len();
    let start = bytes.saturating_sub(OUTPUT_CAP as u64);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let tail = tail_lines(&buf, start > 0);
    let passed = status.is_some_and(|s| s.success());
    Ok(Run { passed, timed_out, code: status.and_then(|s| s.code()), tail, bytes, seconds: t0.elapsed().as_secs_f64() })
}

/// The text of the end of an output; `cut` drops the first, partial line.
pub fn tail_lines(buf: &[u8], cut: bool) -> String {
    let mut tail = String::from_utf8_lossy(buf).into_owned();
    if cut && let Some(i) = tail.find('\n') {
        tail = tail[i + 1..].to_string();
    }
    tail
}

/// What `land` needs from the caller: `prepare` rebases the change onto main and writes the
/// directory (Ok(Some(reply)) stops with that reply, a conflict for one); `unchanged`, after a
/// passing run, checks that dir still holds exactly the tree it was written with (Ok(Some(reply))
/// stops: what was tested is then not what would land); `land_args` are the agent's `undo` and
/// `reason`.
pub fn land(
    url: &str,
    change: &str,
    gate: &Gate,
    dir: &Path,
    land_args: &Value,
    prepare: &mut dyn FnMut() -> Result<Option<Value>>,
    unchanged: &mut dyn FnMut() -> Result<Option<Value>>,
) -> Result<Value> {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let log = dir.with_file_name(format!("{name}.land-test.log"));
    let mut seconds = 0.0;
    for round in 1..=ROUNDS {
        if let Some(stop) = prepare()? {
            return Ok(stop);
        }
        let ch = client::call(url, "change", json!({"change": change}))?;
        let (main, head) = (ch["base"].clone(), ch["head"].clone());
        let r = run(&gate.command, dir, gate.timeout, &log)?;
        seconds += r.seconds;
        let meta = json!({"command": gate.command, "rounds": round, "seconds": (seconds * 100.0).round() / 100.0});
        if !r.passed {
            return Ok(json!({
                "status": "tests_failed", "change": change, "land_test": meta, "exit": r.code,
                "timed_out": r.timed_out, "timeout_s": gate.timeout.as_secs(), "output": r.tail,
                "output_bytes": r.bytes, "advice": FAILED_ADVICE,
            }));
        }
        if let Some(mut stop) = unchanged()? {
            stop["land_test"] = meta;
            return Ok(stop);
        }
        let mut req = land_args.clone();
        req["change"] = json!(change);
        req["tested"] = json!({"command": gate.command, "main": main, "head": head, "seconds": r.seconds});
        let mut v = client::call(url, "land", req)?;
        if v["status"] == "main_moved" && round < ROUNDS {
            continue;
        }
        if v["status"] == "main_moved" {
            v = json!({"status": "untested", "change": change, "advice": MOVING_ADVICE});
        }
        v["land_test"] = meta;
        return Ok(v);
    }
    unreachable!("the last round returns")
}

/// The reply when files of dir changed while the land test ran: the run tested something else
/// than the change, so nothing lands.
pub fn changed_during_test(change: &str, paths: Vec<String>) -> Value {
    json!({"status": "refused", "change": change, "changed_during_test": crate::workspace::capped(paths),
        "advice": CHANGED_ADVICE})
}

const CHANGED_ADVICE: &str = "Not landed: these files in dir changed while the land test ran, so the test did not \
run on your change. Leave dir alone until land answers, and land again.";

// ---- the land queue (`[landing] mode = "queue"`) ------------------------------------------------

/// The land queue of a repository, from a `change` reply (`land_queue`), if it has one.
pub struct Queue {
    pub timeout: Duration,
    pub batch: u64,
    pub parallel: u64,
}

impl Queue {
    pub fn of(change: &Value) -> Option<Queue> {
        let q = change.get("land_queue").filter(|q| q.is_object())?;
        Some(Queue {
            timeout: Duration::from_secs(q["timeout_s"].as_u64().unwrap_or(600)),
            batch: q["batch"].as_u64().unwrap_or(8).max(1),
            parallel: q["parallel"].as_u64().unwrap_or(1).max(1),
        })
    }

    /// How long a change at `position` may wait for its verdict: the test runs of the batches
    /// before it and of its own (a failing batch costs more runs: the changes alone side by side
    /// and the prefixes, or a bisect), plus a margin. One more batch for a step that ran stale.
    pub fn budget(&self, position: u64) -> Duration {
        let runs = if self.parallel > 1 { 3 } else { 1 + (64 - (self.batch - 1).leading_zeros()) as u64 };
        let steps = position.max(1).div_ceil(self.batch) + 1;
        self.timeout * (runs * steps) as u32 + Duration::from_secs(60)
    }
}

pub const QUEUE_FAILED_ADVICE: &str = "Not landed: the server's tests failed on your change on top of main \
(output). dir now holds your change rebased onto main as it is now. Fix it in dir, run the tests there, and land \
again.";
const QUEUE_TIMEOUT_ADVICE: &str = "Not landed yet: the server is still testing the changes ahead of yours. \
Land again to keep waiting.";

/// Fields of the agent-facing reply that tell how the queue went.
fn queue_meta(t0: Instant, position: &Value, entries: u32) -> Value {
    json!({"seconds": (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0, "position": position, "entries": entries})
}

/// `land` through the land queue, for a bridge: enqueues the change (a change already queued just
/// waits), waits for the server's verdict (events of the change, `land_status`) and answers in the
/// shapes of a direct land: `landed`, `tests_failed` (after `on_reject` brought the change and dir
/// to main as it is now; its reply, if any, is merged in: conflict markers), `conflict`, `undoes`.
/// The agent never sees `queued`. `land_args` are the agent's `undo` and `reason`.
pub fn land_queued(
    url: &str,
    change: &str,
    q: &Queue,
    land_args: &Value,
    on_reject: &mut dyn FnMut() -> Result<Value>,
) -> Result<Value> {
    let t0 = Instant::now();
    let mut req = land_args.clone();
    req["change"] = json!(change);
    let status = |c: &str| client::call(url, "land_status", json!({"change": c}));
    // A rejection before this call is old news: only a new review is this land's verdict.
    let first = status(change)?;
    let old_review = if first["verdict"] == "rejected" { first["review"].clone() } else { Value::Null };
    let mut queued = first["verdict"] == "queued";
    let mut position = Value::Null;
    for entries in 1..=4u32 {
        if !queued {
            let v = client::call(url, "land", req.clone())?;
            if v["status"] != "queued" {
                return Ok(v);
            }
            position = v["position"].clone();
        }
        queued = false;
        let deadline = t0 + q.budget(position.as_u64().unwrap_or(q.batch));
        let mut since = client::call(url, "events", json!({"since": 0, "limit": 0}))?["latest"].as_u64().unwrap_or(0);
        let st = loop {
            let st = status(change)?;
            if !matches!(st["verdict"].as_str(), Some("queued" | "pending")) {
                break st;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(json!({"status": "not_landed_yet", "change": change,
                    "land_queue": queue_meta(t0, &position, entries), "advice": QUEUE_TIMEOUT_ADVICE}));
            }
            // Wakes on any event of the change (landed, rejected, conflict, undoes), else re-checks.
            let wait = left.min(Duration::from_secs(5)).as_millis() as u64;
            let ev = client::call(url, "events", json!({"since": since, "change": change, "wait_ms": wait}))?;
            since = ev["next"].as_u64().unwrap_or(since);
        };
        let meta = queue_meta(t0, &position, entries);
        match st["verdict"].as_str().unwrap_or_default() {
            "passed" => {
                // Landing again answers like the land the queue made (changeset, commit).
                let mut v = client::call(url, "land", req.clone())?;
                if let Some(m) = v.as_object_mut() {
                    m.remove("repeated");
                }
                v["land_queue"] = meta;
                return Ok(v);
            }
            "rejected" if st["review"] != old_review => {
                let output = st["output"].as_str().unwrap_or_default();
                let cut = output.len() > OUTPUT_CAP;
                let tail = tail_lines(&output.as_bytes()[output.len().saturating_sub(OUTPUT_CAP)..], cut);
                let mut v = json!({
                    "status": "tests_failed", "change": change, "exit": st["exit"], "timed_out": st["timed_out"],
                    "output": tail, "land_queue": meta, "advice": QUEUE_FAILED_ADVICE,
                });
                let extra = on_reject()?;
                if let Some(m) = extra.as_object() {
                    for (k, x) in m {
                        v[k.as_str()] = x.clone();
                    }
                }
                return Ok(v);
            }
            "abandoned" => anyhow::bail!("change {change} was abandoned while it waited in the land queue"),
            // Held (undoes), sent back with a conflict, or dropped: landing again gives the reply
            // with the details (undoes entries, conflicts), or enqueues it once more.
            _ => {}
        }
    }
    Ok(json!({"status": "not_landed_yet", "change": change, "land_queue": queue_meta(t0, &position, 4),
        "advice": QUEUE_TIMEOUT_ADVICE}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_wait_grows_with_the_position_and_the_runs_of_a_step() {
        let q = |batch, parallel| Queue { timeout: Duration::from_secs(30), batch, parallel };
        // batch 8, one run at a time: a bisect of 8 is 1 + 3 runs; first in line: its batch and one more.
        assert_eq!(q(8, 1).budget(1), Duration::from_secs(30 * 4 * 2 + 60));
        assert_eq!(q(8, 1).budget(9), Duration::from_secs(30 * 4 * 3 + 60));
        assert_eq!(q(1, 1).budget(1), Duration::from_secs(30 * 2 + 60));
        assert_eq!(q(8, 4).budget(1), Duration::from_secs(30 * 3 * 2 + 60));
    }

    #[test]
    fn a_run_passes_fails_times_out_and_caps_its_output() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("log");
        assert!(run("true", tmp.path(), Duration::from_secs(10), &log).unwrap().passed);
        let r = run("echo nope; exit 3", tmp.path(), Duration::from_secs(10), &log).unwrap();
        assert_eq!((r.passed, r.code, r.tail.as_str()), (false, Some(3), "nope\n"));
        let r = run("sleep 30", tmp.path(), Duration::from_millis(300), &log).unwrap();
        assert!(!r.passed && r.timed_out && r.seconds < 10.0);
        let r = run(
            "i=0; while [ $i -lt 2000 ]; do echo line $i; i=$((i+1)); done; exit 1",
            tmp.path(),
            Duration::from_secs(20),
            &log,
        )
        .unwrap();
        assert!(r.tail.len() <= OUTPUT_CAP && r.bytes > OUTPUT_CAP as u64);
        assert!(r.tail.starts_with("line ") && r.tail.ends_with("line 1999\n"), "{}", &r.tail[..40]);
    }
}
