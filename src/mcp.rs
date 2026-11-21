//! MCP: one JSON-RPC handler shared by the stdio bridge (`treeyard mcp`, a thin client of one
//! treeyard server) and the server's own Streamable HTTP endpoint (`POST /mcp`, see `server`).

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::{client, rpc, workspace};

const INSTRUCTIONS: &str = "TreeYard serves the repository as an API. Read with read/list/grep/log \
without a checkout; ask blame (who changed a line, with intent), symbols/outline (definitions) and \
bisect (first bad change) on the server. To change code: open_change (agent + one-line intent), apply edits, then land. \
land rebases on the server; on status conflict, read the tasks in change.conflicts, write the final \
content with apply (or resolve), and land again.";

/// The reply to one JSON-RPC message, or `None` for a notification. `call` runs one operation;
/// its error becomes a tool result with `isError`, so the agent reads it.
pub fn reply(msg: &Value, call: &mut dyn FnMut(&str, Value) -> Result<Value>) -> Option<Value> {
    reply_with(msg, &[], call)
}

/// `reply` with `extra` tools after the operation table in `tools/list` (the stdio bridge's own).
pub fn reply_with(msg: &Value, extra: &[Value], call: &mut dyn FnMut(&str, Value) -> Result<Value>) -> Option<Value> {
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
        "tools/list" => {
            let mut tools = rpc::tools();
            tools.extend(extra.iter().cloned());
            Ok(json!({"tools": tools}))
        }
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

/// The bridge's own tool: it writes files on the agent's machine, so only the local stdio bridge
/// has it (the server's `/mcp` endpoint cannot reach the agent's disk).
pub fn checkout_tool() -> Value {
    json!({
        "name": "checkout",
        "description": "Write the files of a change (its base plus your edits) into a local directory and \
    return its path in `dir`; run the repository's tests there. Default directory: one of its own per change, \
    under the system temp directory. After that, every apply, resolve, rebase, restore_change and land of the \
    change through this bridge rewrites the directory (reply field `checkout`). Edits made in the directory do \
    not reach the change: edit with apply. `main_moved` true: rebase, then checkout again, to test what land \
    will put on main.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "change": {"type": "string", "description": "change id, ch_..."},
                "into": {"type": "string", "description": "a new, empty or earlier checkout directory instead of the default"},
            },
            "required": ["change"],
            "additionalProperties": false,
        },
    })
}

/// Operations after which the bridge rewrites the change's checkout directory.
const REFRESH: &[&str] = &["apply", "resolve", "rebase", "restore_change", "land"];

pub fn run(url: &str, agent: Option<String>) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let extra = [checkout_tool()];
    // Checkout directories made through this bridge, per change.
    let mut checkouts: HashMap<String, PathBuf> = HashMap::new();
    let mut call = |name: &str, mut args: Value| -> Result<Value> {
        if name == "checkout" {
            let change = args["change"].as_str().context("checkout needs change (ch_...)")?.to_string();
            let dir = match args["into"].as_str().filter(|s| !s.trim().is_empty()) {
                Some(d) => std::path::absolute(d)?,
                None => workspace::checkout_dir(&change)?,
            };
            let v = workspace::checkout(url, &change, &dir)?;
            checkouts.insert(change, dir);
            return Ok(v);
        }
        // With a token the server names the agent itself (the token's name).
        if matches!(name, "open_change" | "revert")
            && args.get("agent").is_none()
            && std::env::var_os("TREEYARD_TOKEN").is_none()
            && let Some(a) = &agent
        {
            args["agent"] = json!(a);
        }
        let change = args["change"].as_str().map(str::to_string);
        let mut v = client::call(url, name, args)?;
        if REFRESH.contains(&name)
            && v.is_object()
            && let Some(dir) = change.as_ref().and_then(|c| checkouts.get(c))
        {
            let c = change.as_deref().unwrap_or_default();
            // A failed rewrite never fails the call: the edit is on the server.
            v["checkout"] = workspace::checkout(url, c, dir)
                .unwrap_or_else(|e| json!({"dir": dir, "error": format!("checkout not rewritten: {e:#}")}));
        }
        Ok(v)
    };
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => match reply_with(&msg, &extra, &mut call) {
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
