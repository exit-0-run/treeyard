//! MCP: one JSON-RPC handler shared by the stdio bridge (`treeyard mcp`, a thin client of one
//! treeyard server) and the server's own Streamable HTTP endpoint (`POST /mcp`, see `server`).

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::lean::Lean;
use crate::{client, gate, native, rpc, workspace};

const INSTRUCTIONS: &str = "TreeYard serves the repository as an API. Read with read/list/grep/log \
without a checkout; ask blame (who changed a line, with intent), symbols/outline (definitions) and \
bisect (first bad change) on the server. To change code: open_change (agent + one-line intent), apply edits, then land. \
land rebases on the server; on status conflict, read the tasks in change.conflicts, write the final \
content with apply (or resolve), and land again.";

const AGENT_INSTRUCTIONS: &str = "TreeYard serves the repository as an API. Read with read/grep/symbols/outline \
without a checkout. To change code: apply with a one-line intent and your edits (it opens your change and, \
through the stdio bridge, writes it to the directory in checkout.dir), run the tests in checkout.dir, then land. \
Later applies give the change id. On land status conflict, resolve the regions in change.conflicts with apply \
(or resolve), and land again. This is the agent tool set; every operation is listed with --tools all.";

const NATIVE_INSTRUCTIONS: &str = "TreeYard lets several agents work on this repository at once. Call start \
first: it opens your change and writes a full copy of the repository to dir. Read, edit and test only in dir, \
with your own tools. sync sends your edits and reports collisions and undoes (another agent's landed lines that \
you removed: put its part back; land with undo only when your own task names those lines for removal). land syncs \
and lands; on status conflict, fix the marked files in dir and land again.";

/// Which tools `tools/list` offers. `Agent`: the operations a coding agent needs
/// (`rpc::AGENT_TOOLS`), with short descriptions; `All`: every operation; `Native` (stdio bridge
/// only): `start`, `sync`, `land` on a directory the agent edits with its own tools, plus
/// `change` and `diff` (`native`). A `tools/call` of any operation works with each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToolSet {
    Agent,
    #[default]
    All,
    Native,
}

impl std::str::FromStr for ToolSet {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "agent" => Ok(ToolSet::Agent),
            "all" => Ok(ToolSet::All),
            "native" => Ok(ToolSet::Native),
            _ => anyhow::bail!("tools must be agent, all or native, not {s:?}"),
        }
    }
}

/// A whole-file `read` (no range, no `max_bytes`) through MCP returns at most this many bytes of
/// whole lines, with `truncated` and a note (a 60 000-line file once returned 938 KB to an agent).
pub const READ_CAP: usize = 50 * 1024;

/// The agent view's memory for the stateless HTTP endpoint (one per server process).
static HTTP_LEAN: std::sync::LazyLock<std::sync::Mutex<Lean>> = std::sync::LazyLock::new(Default::default);

/// The reply to one JSON-RPC message, or `None` for a notification. `call` runs one operation;
/// its error becomes a tool result with `isError`, so the agent reads it. Used by `POST /mcp`.
pub fn reply(msg: &Value, set: ToolSet, call: &mut dyn FnMut(&str, Value) -> Result<Value>) -> Option<Value> {
    let mut lean = HTTP_LEAN.lock().unwrap_or_else(|e| e.into_inner());
    reply_with(msg, set, &[], &mut lean, call)
}

/// `reply` with `extra` tools after the operation table in `tools/list` (the stdio bridge's own),
/// and its own agent-view memory.
pub fn reply_with(
    msg: &Value,
    set: ToolSet,
    extra: &[Value],
    lean: &mut Lean,
    call: &mut dyn FnMut(&str, Value) -> Result<Value>,
) -> Option<Value> {
    let method = msg["method"].as_str().unwrap_or_default();
    if method.starts_with("notifications/") {
        return None;
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "treeyard", "version": env!("CARGO_PKG_VERSION")},
            "instructions": match set {
                ToolSet::Agent => AGENT_INSTRUCTIONS,
                ToolSet::All => INSTRUCTIONS,
                ToolSet::Native => NATIVE_INSTRUCTIONS,
            },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut tools = match set {
                ToolSet::Agent => rpc::agent_tools(),
                ToolSet::All => rpc::tools(),
                ToolSet::Native => native::tools(),
            };
            if extra.iter().any(|t| t["name"] == "checkout") {
                add_checkout_param(&mut tools, set);
            }
            tools.extend(extra.iter().cloned());
            Ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or_default();
            let mut args = match &msg["params"]["arguments"] {
                Value::Object(_) => msg["params"]["arguments"].clone(),
                _ => json!({}),
            };
            if name == "read"
                && ["start", "end", "ranges", "max_bytes"].iter().all(|k| args.get(*k).is_none_or(Value::is_null))
            {
                args["max_bytes"] = json!(READ_CAP);
            }
            Ok(match call(name, args) {
                Ok(mut v) => {
                    // The native tools shape their own replies.
                    if set == ToolSet::Agent || (set == ToolSet::Native && !native::OWN.contains(&name)) {
                        lean.shape(name, &mut v);
                    }
                    json!({"content": [{"type": "text", "text": serde_json::to_string(&v).unwrap_or_default()}]})
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
    not reach the change: edit with apply. `main_moved` true (work landed in files you edit): rebase. An apply \
    reply with checkout.dir has written it already: no call needed.",
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

/// The stdio bridge's own `apply` parameter `checkout` (the server's `/mcp` cannot write the
/// agent's disk). Default: true in the agent tool set, false with `--tools all`.
fn add_checkout_param(tools: &mut [Value], set: ToolSet) {
    let default = if set == ToolSet::Agent { "default true" } else { "default false" };
    for t in tools.iter_mut().filter(|t| t["name"] == "apply") {
        t["inputSchema"]["properties"]["checkout"] = json!({"type": "boolean", "description": format!("write the change to a directory for tests (checkout.dir); {default}")});
    }
}

/// Operations after which the bridge rewrites the change's checkout directory.
const REFRESH: &[&str] = &["apply", "resolve", "rebase", "restore_change", "land"];

pub fn run(url: &str, agent: Option<String>, set: ToolSet) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let extra = if set == ToolSet::Native { vec![] } else { vec![checkout_tool()] };
    let mut lean = Lean::default();
    // The native tools keep their own agent view (`reply_with` borrows `lean`).
    let mut native_lean = Lean::default();
    // Checkout directories made through this bridge, per change.
    let mut checkouts: HashMap<String, PathBuf> = HashMap::new();
    let mut call = |name: &str, mut args: Value| -> Result<Value> {
        if set == ToolSet::Native && native::OWN.contains(&name) {
            return native::call(url, agent.as_deref(), &mut native_lean, name, args);
        }
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
        // `checkout` on `apply` is the bridge's: the first apply of a change can write its directory.
        let want_checkout = match args.as_object_mut().and_then(|m| m.remove("checkout")) {
            Some(Value::Bool(b)) => b,
            Some(Value::Null) | None => set == ToolSet::Agent,
            Some(other) => anyhow::bail!("checkout must be true or false, not {other}"),
        } && name == "apply";
        let opens = name == "apply" && args.get("change").is_none_or(Value::is_null);
        // With a token the server names the agent itself (the token's name).
        if (matches!(name, "open_change" | "revert") || opens)
            && args.get("agent").is_none()
            && std::env::var_os("TREEYARD_TOKEN").is_none()
            && let Some(a) = &agent
        {
            args["agent"] = json!(a);
        }
        let given = args["change"].as_str().map(str::to_string);
        // The land test (`[coordination] land_test`): run it in the change's checkout directory.
        if name == "land"
            && let Some(c) = given.clone()
            && let Some(g) = gate::Gate::of(&client::call(url, "change", json!({"change": c}))?)
        {
            let dir = match checkouts.get(&c) {
                Some(d) => d.clone(),
                None => workspace::checkout_dir(&c)?,
            };
            let mut prepare = || -> Result<Option<Value>> {
                let rb = client::call(url, "rebase", json!({"change": c}))?;
                if rb["conflicted"] == true {
                    return Ok(Some(json!({"status": "conflict", "change": rb})));
                }
                workspace::checkout(url, &c, &dir)?;
                Ok(None)
            };
            let mut req = args.clone();
            if let Some(m) = req.as_object_mut() {
                m.remove("change");
            }
            let mut v = gate::land(url, &c, &g, &dir, &req, &mut prepare)?;
            checkouts.insert(c.clone(), dir.clone());
            if v.is_object() {
                v["checkout"] = workspace::checkout(url, &c, &dir)
                    .unwrap_or_else(|e| json!({"dir": dir, "error": format!("checkout not written: {e:#}")}));
            }
            return Ok(v);
        }
        let mut v = client::call(url, name, args)?;
        // An apply that opened the change names it in the reply.
        let change = given.or_else(|| if opens { v["id"].as_str().map(str::to_string) } else { None });
        if REFRESH.contains(&name)
            && v.is_object()
            && let Some(c) = change
        {
            let dir = match checkouts.get(&c) {
                Some(d) => Some(d.clone()),
                None if want_checkout => workspace::checkout_dir(&c).ok(),
                None => None,
            };
            if let Some(dir) = dir {
                // A failed write never fails the call: the edit is on the server.
                v["checkout"] = match workspace::checkout(url, &c, &dir) {
                    Ok(co) => {
                        checkouts.insert(c, dir);
                        co
                    }
                    Err(e) => json!({"dir": dir, "error": format!("checkout not written: {e:#}")}),
                };
            }
        }
        Ok(v)
    };
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => match reply_with(&msg, set, &extra, &mut lean, &mut call) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_tool_set_is_short_and_every_name_is_an_operation() {
        let list = |set| {
            let r = reply_with(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
                set,
                &[checkout_tool()],
                &mut Lean::default(),
                &mut |_, _| Ok(json!({})),
            );
            r.unwrap()["result"].clone()
        };
        let agent = list(ToolSet::Agent);
        let names: Vec<&str> = agent["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        let mut want = rpc::AGENT_TOOLS.to_vec();
        want.push("checkout");
        assert_eq!(names, want);
        let all: Vec<String> = rpc::tools().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        for n in rpc::AGENT_TOOLS {
            assert!(all.iter().any(|a| a == n), "{n} is not an operation");
        }
        let bytes = |v: &Value| serde_json::to_string(v).unwrap().len();
        assert!(bytes(&agent) * 3 < bytes(&list(ToolSet::All)), "{} bytes", bytes(&agent));
    }
}
