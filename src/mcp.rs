//! MCP: one JSON-RPC handler shared by the stdio bridge (`treeyard mcp`, a thin client of one
//! treeyard server) and the server's own Streamable HTTP endpoint (`POST /mcp`, see `server`).

use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

use crate::{client, rpc};

const INSTRUCTIONS: &str = "TreeYard serves the repository as an API. Read with read/list/grep/log \
without a checkout; ask blame (who changed a line, with intent), symbols/outline (definitions) and \
bisect (first bad change) on the server. To change code: open_change (agent + one-line intent), apply edits, then land. \
land rebases on the server; on status conflict, read the tasks in change.conflicts, write the final \
content with apply (or resolve), and land again.";

/// The reply to one JSON-RPC message, or `None` for a notification. `call` runs one operation;
/// its error becomes a tool result with `isError`, so the agent reads it.
pub fn reply(msg: &Value, call: &mut dyn FnMut(&str, Value) -> Result<Value>) -> Option<Value> {
    let method = msg["method"].as_str().unwrap_or_default();
    if method.starts_with("notifications/") {
        return None;
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "treeyard", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": rpc::tools()})),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or_default();
            let args = match &msg["params"]["arguments"] {
                Value::Object(_) => msg["params"]["arguments"].clone(),
                _ => json!({}),
            };
            Ok(match call(name, args) {
                Ok(v) => {
                    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}]})
                }
                Err(e) => json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true}),
            })
        }
        _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
    };
    let id = msg.get("id").cloned()?;
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

pub fn run(url: &str, agent: Option<String>) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut call = |name: &str, mut args: Value| {
        // With a token the server names the agent itself (the token's name).
        if matches!(name, "open_change" | "revert")
            && args.get("agent").is_none()
            && std::env::var_os("TREEYARD_TOKEN").is_none()
            && let Some(a) = &agent
        {
            args["agent"] = json!(a);
        }
        client::call(url, name, args)
    };
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => match reply(&msg, &mut call) {
                Some(r) => r,
                None => continue,
            },
            Err(e) => json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}}),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}
