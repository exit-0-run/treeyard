//! MCP over stdio: a thin bridge from an agent's tool calls to one rootstock server.

use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

use crate::{client, rpc};

const INSTRUCTIONS: &str = "Rootstock serves the repository as an API. Read with read/list/grep/log \
without a checkout. To change code: open_change (agent + one-line intent), apply edits, then land. \
land rebases on the server; on status conflict, read the tasks in change.conflicts, write the final \
content with apply (or resolve), and land again.";

pub fn run(url: &str, agent: Option<String>) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}});
                writeln!(out, "{err}")?;
                out.flush()?;
                continue;
            }
        };
        let method = msg["method"].as_str().unwrap_or_default();
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "rootstock", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": rpc::tools()})),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                let mut args = match &msg["params"]["arguments"] {
                    Value::Object(_) => msg["params"]["arguments"].clone(),
                    _ => json!({}),
                };
                if matches!(name, "open_change" | "revert")
                    && args.get("agent").is_none()
                    && let Some(a) = &agent
                {
                    args["agent"] = json!(a);
                }
                Ok(match client::call(url, name, args) {
                    Ok(v) => json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v)?}]}),
                    Err(e) => json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true}),
                })
            }
            m if m.starts_with("notifications/") => continue,
            _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
        };
        let Some(id) = msg.get("id").cloned() else { continue };
        let reply = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}
