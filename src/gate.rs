//! The land test on the agent's side (`[coordination] land_test`): the stdio bridge rebases the
//! change onto main, brings the change's directory to exactly that tree, runs the command there
//! with a timeout, and lands only with a proof (`tested`: command, main, head) that the server
//! checks under its lock together with the land. When main moved in between, the server answers
//! `main_moved` and the bridge goes round again, at most `ROUNDS` times.

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

/// Runs `command` with `sh -c` in `dir`, output to `log`, killed (with its process group) after
/// `timeout`.
pub fn run(command: &str, dir: &Path, timeout: Duration, log: &Path) -> Result<Run> {
    let t0 = Instant::now();
    let out = std::fs::File::create(log).with_context(|| format!("create {}", log.display()))?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
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
    let mut f = std::fs::File::open(log)?;
    let bytes = f.metadata()?.len();
    let start = bytes.saturating_sub(OUTPUT_CAP as u64);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let mut tail = String::from_utf8_lossy(&buf).into_owned();
    if start > 0
        && let Some(i) = tail.find('\n')
    {
        tail = tail[i + 1..].to_string();
    }
    let passed = status.is_some_and(|s| s.success());
    Ok(Run { passed, timed_out, code: status.and_then(|s| s.code()), tail, bytes, seconds: t0.elapsed().as_secs_f64() })
}

/// What `land` needs from the caller: `prepare` rebases the change onto main and writes the
/// directory (Ok(Some(reply)) stops with that reply, a conflict for one); `land_args` are the
/// agent's `undo` and `reason`.
pub fn land(
    url: &str,
    change: &str,
    gate: &Gate,
    dir: &Path,
    land_args: &Value,
    prepare: &mut dyn FnMut() -> Result<Option<Value>>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
