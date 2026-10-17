//! One operation table shared by the HTTP API, MCP and the CLI.

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::repo::{Edit, Repo};

fn params<T: DeserializeOwned>(p: Value) -> Result<T> {
    let p = if p.is_null() { json!({}) } else { p };
    serde_json::from_value(p).map_err(|e| anyhow!("bad params: {e}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct At {
    path: Option<String>,
    at: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grep {
    pattern: String,
    at: Option<String>,
    path: Option<String>,
    max: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Diff {
    change: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Log {
    path: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    agent: String,
    intent: String,
    model: Option<String>,
    session: Option<String>,
    base: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    change: String,
    edits: Vec<Edit>,
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    change: String,
    path: String,
    take: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeId {
    change: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Changes {
    state: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revert {
    changeset: String,
    agent: String,
}

pub fn call(repo: &Repo, op: &str, p: Value) -> Result<Value> {
    repo.with(|r| match op {
        "status" => r.status(),
        "read" => {
            let a: At = params(p)?;
            let path = a.path.ok_or_else(|| anyhow!("read needs path"))?;
            r.read(&path, a.at.as_deref())
        }
        "list" => {
            let a: At = params(p)?;
            r.list(a.path.as_deref().unwrap_or(""), a.at.as_deref())
        }
        "grep" => {
            let a: Grep = params(p)?;
            r.grep(&a.pattern, a.at.as_deref(), a.path.as_deref(), a.max.unwrap_or(100))
        }
        "diff" => {
            let a: Diff = params(p)?;
            let (from, to) = match (a.change, a.from) {
                (Some(c), None) => {
                    let ch = r.change(&c)?;
                    (ch.base, ch.head)
                }
                (None, Some(from)) => (r.resolve_at(Some(&from))?, r.resolve_at(a.to.as_deref())?),
                _ => bail!("diff needs either change, or from (and optionally to)"),
            };
            r.diff(&from, &to)
        }
        "log" => {
            let a: Log = params(p)?;
            r.log(a.path.as_deref(), a.limit.unwrap_or(20))
        }
        "open_change" => {
            let a: Open = params(p)?;
            let ch = r.open_change(&a.agent, &a.intent, a.model, a.session, a.base.as_deref())?;
            r.change_view(&ch)
        }
        "apply" => {
            let a: Apply = params(p)?;
            let ch = r.apply(&a.change, &a.edits, a.message)?;
            let mut v = r.change_view(&ch)?;
            v["overlaps"] = Value::Array(r.overlaps(&ch)?);
            Ok(v)
        }
        "resolve" => {
            let a: Resolve = params(p)?;
            let ch = r.resolve(&a.change, &a.path, &a.take)?;
            r.change_view(&ch)
        }
        "rebase" => {
            let a: ChangeId = params(p)?;
            let ch = r.rebase(&a.change)?;
            r.change_view(&ch)
        }
        "land" => {
            let a: ChangeId = params(p)?;
            r.land(&a.change)
        }
        "abandon" => {
            let a: ChangeId = params(p)?;
            let ch = r.abandon(&a.change)?;
            r.change_view(&ch)
        }
        "change" => {
            let a: ChangeId = params(p)?;
            let ch = r.change(&a.change)?;
            let mut v = r.change_view(&ch)?;
            v["overlaps"] = Value::Array(r.overlaps(&ch)?);
            Ok(v)
        }
        "changes" => {
            let a: Changes = params(p)?;
            let list = r.store.changes(Some(a.state.as_deref().unwrap_or("open")))?;
            let views = list.iter().map(|c| r.change_view(c)).collect::<Result<Vec<_>>>()?;
            Ok(json!({"changes": views}))
        }
        "revert" => {
            let a: Revert = params(p)?;
            r.revert(&a.changeset, &a.agent)
        }
        "export_change" => {
            let a: ChangeId = params(p)?;
            r.export_change(&a.change)
        }
        "tools" => Ok(json!({"tools": tools()})),
        _ => bail!("unknown operation {op:?}; see the tools operation for the list"),
    })
}

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": props, "required": required, "additionalProperties": false},
    })
}

/// MCP tool definitions for every operation.
pub fn tools() -> Vec<Value> {
    let at = json!({"type": "string", "description": "main (default), a change id ch_..., a changeset id or prefix, or a git commit id"});
    let change = json!({"type": "string", "description": "change id, ch_..."});
    vec![
        tool("status", "Main changeset and git commit, file count, open changes.", json!({}), &[]),
        tool("read", "Read one file without a checkout.", json!({"path": {"type": "string"}, "at": at}), &["path"]),
        tool(
            "list",
            "List one directory (files and subdirectories).",
            json!({"path": {"type": "string", "description": "directory, empty for the root"}, "at": at}),
            &[],
        ),
        tool(
            "grep",
            "Search file contents with a regular expression, server side.",
            json!({"pattern": {"type": "string"}, "path": {"type": "string", "description": "limit to this directory"}, "at": at, "max": {"type": "integer", "description": "max matches, default 100"}}),
            &["pattern"],
        ),
        tool(
            "diff",
            "Unified diff of a change (base to head), or between two revisions.",
            json!({"change": change, "from": at, "to": at}),
            &[],
        ),
        tool(
            "log",
            "History of main, newest first, optionally for one path or directory.",
            json!({"path": {"type": "string"}, "limit": {"type": "integer"}}),
            &[],
        ),
        tool(
            "open_change",
            "Start a change on top of main (no branch). Returns its id; edit it with apply, then land.",
            json!({"agent": {"type": "string"}, "intent": {"type": "string", "description": "what you are about to do, in one line"}, "model": {"type": "string"}, "session": {"type": "string"}, "base": {"type": "string", "description": "main (default) or another revision; a change id ch_... stacks on that change, which then lands first"}}),
            &["agent", "intent"],
        ),
        tool(
            "apply",
            "Edit files in a change: per edit give content, content_b64, delete:true, replace:{old,new,all?}, and optionally executable. Returns the change and other open changes touching the same paths.",
            json!({"change": change, "message": {"type": "string", "description": "commit message; defaults to the intent"}, "edits": {"type": "array", "items": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}, "content_b64": {"type": "string"}, "delete": {"type": "boolean"}, "replace": {"type": "object", "properties": {"old": {"type": "string"}, "new": {"type": "string"}, "all": {"type": "boolean"}}, "required": ["old", "new"]}, "executable": {"type": "boolean"}}, "required": ["path"]}}}),
            &["change", "edits"],
        ),
        tool(
            "resolve",
            "Settle a conflicted path by taking one side (or write the path with apply).",
            json!({"change": change, "path": {"type": "string"}, "take": {"type": "string", "enum": ["ours", "theirs", "base", "delete"]}}),
            &["change", "path", "take"],
        ),
        tool(
            "rebase",
            "Rebase a change onto current main on the server. Conflicts come back as typed tasks.",
            json!({"change": change}),
            &["change"],
        ),
        tool(
            "land",
            "Land a change on main: rebases on the server if main moved. Returns status landed (with the git commit), already_on_main (main had every edit), waiting (stacked on a change that has not landed), or conflict (with tasks).",
            json!({"change": change}),
            &["change"],
        ),
        tool("abandon", "Drop a change.", json!({"change": change}), &["change"]),
        tool(
            "change",
            "One change: paths, conflicts as tasks, overlaps with other open changes.",
            json!({"change": change}),
            &["change"],
        ),
        tool(
            "changes",
            "List changes by state (open by default).",
            json!({"state": {"type": "string", "enum": ["open", "landed", "abandoned"]}}),
            &[],
        ),
        tool(
            "revert",
            "Undo a landed changeset: opens a change with the inverse and lands it.",
            json!({"changeset": at, "agent": {"type": "string"}}),
            &["changeset", "agent"],
        ),
        tool(
            "export_change",
            "Write a change's head as a git commit under refs/rootstock/changes/<id>.",
            json!({"change": change}),
            &["change"],
        ),
    ]
}
