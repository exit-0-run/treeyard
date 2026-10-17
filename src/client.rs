//! Blocking HTTP client for the API, used by the CLI and the MCP bridge.
//!
//! Network errors: a request that never reached the server (connection refused, DNS) and a
//! `503 busy` reply are retried for every operation. A request that may have reached the
//! server (the connection broke while waiting for the reply) is retried only for reads and
//! `land`, because landing twice answers like the first land. Retries back off from 100 ms to
//! 2 s, `TREEYARD_RETRIES` times (default 6, about 6 s in total).

use anyhow::{Result, bail};
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

/// Operations that change nothing on the server, plus `land` (idempotent by design).
const SAFE_TO_REPEAT: &[&str] = &[
    "status", "whoami", "read", "list", "tree", "blobs", "grep", "symbols", "outline", "blame", "diff", "log", "audit",
    "change", "changes", "reviews", "queue", "config", "tools", "events", "land",
];

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(5)).build())
}

fn retries() -> u32 {
    std::env::var("TREEYARD_RETRIES").ok().and_then(|v| v.parse().ok()).unwrap_or(6)
}

/// Calls `op` with the token from `TREEYARD_TOKEN`, if set.
pub fn call(url: &str, op: &str, params: Value) -> Result<Value> {
    let token = std::env::var("TREEYARD_TOKEN").ok().filter(|t| !t.is_empty());
    call_with(url, token.as_deref(), op, params)
}

enum Try {
    Done(Result<Value>),
    /// Retry; the text says why, for the final error.
    Again(String),
}

fn once(endpoint: &str, url: &str, token: Option<&str>, op: &str, params: &Value) -> Try {
    let mut req = agent().post(endpoint);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    match req.send_json(params) {
        Ok(resp) => Try::Done(resp.into_json().map_err(Into::into)),
        Err(ureq::Error::Status(503, resp)) => {
            let body: Value = resp.into_json().unwrap_or(Value::Null);
            Try::Again(body["error"].as_str().unwrap_or("HTTP 503").to_string())
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body: Value = resp.into_json().unwrap_or(Value::Null);
            Try::Done(match body["error"].as_str() {
                Some(msg) if code == 401 || code == 403 => Err(anyhow::anyhow!("HTTP {code}: {msg}")),
                Some(msg) => Err(anyhow::anyhow!("{msg}")),
                None => Err(anyhow::anyhow!("HTTP {code} from {endpoint}")),
            })
        }
        Err(ureq::Error::Transport(t)) => {
            let unsent = matches!(t.kind(), ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Dns);
            if unsent || SAFE_TO_REPEAT.contains(&op) {
                Try::Again(format!("cannot reach treeyard at {url}: {t}"))
            } else {
                Try::Done(Err(anyhow::anyhow!(
                    "connection to treeyard at {url} broke during {op}; the server may have run it: \
                     check with `change` or `status` before you call it again ({t})"
                )))
            }
        }
    }
}

/// Calls `op` with an explicit token (None = no Authorization header).
pub fn call_with(url: &str, token: Option<&str>, op: &str, params: Value) -> Result<Value> {
    let endpoint = format!("{}/api/{op}", url.trim_end_matches('/'));
    let mut wait = Duration::from_millis(100);
    let n = retries();
    for attempt in 0..=n {
        match once(&endpoint, url, token, op, &params) {
            Try::Done(r) => return r,
            Try::Again(why) if attempt == n => bail!("{why} (gave up after {} retries)", n),
            Try::Again(_) => {
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_secs(2));
            }
        }
    }
    unreachable!("the loop returns on its last attempt")
}
