//! One operation table shared by the HTTP API, MCP and the CLI.

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::audit::{self, Record};
use crate::auth::{Caller, Denied, deny};
use crate::config::IndexMode;
use crate::coord::Filter;
use crate::model::{Change, Intent};
use crate::repo::{Edit, Inner, Repo};
use crate::{bisect, history, landing, search, symbols};

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
    /// false = scan every file (no trigram index).
    index: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Blame {
    path: String,
    at: Option<String>,
    start: Option<u32>,
    end: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolsP {
    name: Option<String>,
    prefix: Option<String>,
    kind: Option<String>,
    path: Option<String>,
    at: Option<String>,
    max: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BisectP {
    good: String,
    bad: Option<String>,
    command: Option<String>,
    verify: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditP {
    agent: Option<String>,
    actor: Option<String>,
    change: Option<String>,
    op: Option<String>,
    status: Option<String>,
    path: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
    before: Option<i64>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenCreate {
    name: String,
    #[serde(default)]
    admin: bool,
    read: Option<Vec<String>>,
    #[serde(default)]
    write: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenId {
    id: String,
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
    at: Option<String>,
    /// Changed-path filter: only changesets touching a path that matches one of these globs.
    #[serde(default)]
    paths: Vec<String>,
    agent: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
    /// true = walk the changesets (no index).
    scan: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    agent: Option<String>,
    intent: String,
    model: Option<String>,
    session: Option<String>,
    base: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    symbols: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declare {
    change: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    symbols: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsP {
    since: Option<u64>,
    change: Option<String>,
    agent: Option<String>,
    #[serde(default)]
    kinds: Vec<String>,
    limit: Option<usize>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reviews {
    state: Option<String>,
    change: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewClose {
    id: i64,
    state: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    change: String,
    edits: Vec<Edit>,
    message: Option<String>,
    seen: Option<String>,
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
struct Blobs {
    ids: Vec<String>,
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revert {
    changeset: String,
    agent: Option<String>,
}

/// Advice for an agent whose declared intent overlaps open changes.
fn advice(overlaps: &[Value]) -> Value {
    if overlaps.is_empty() {
        return Value::Null;
    }
    let first = overlaps[0]["change"].as_str().unwrap_or_default();
    Value::String(format!(
        "{} open change(s) plan to touch the same code. To avoid a late conflict: wait for them to land \
         (watch events for kind landed), or stack on one with open_change base = {first}, or narrow your paths.",
        overlaps.len()
    ))
}

/// Operations that change state (or run commands); each call writes one audit row.
const AUDITED: &[&str] = &[
    "open_change",
    "declare",
    "apply",
    "resolve",
    "rebase",
    "land",
    "abandon",
    "revert",
    "export_change",
    "review_close",
    "queue_run",
    "test_main",
    "bisect",
    "token_create",
    "token_revoke",
];

/// Runs one operation with full rights (in-process callers: tests, replay, CLI tools).
pub fn call(repo: &Repo, op: &str, p: Value) -> Result<Value> {
    call_as(repo, &Caller::internal(), op, p)
}

/// Runs one operation for `c`: checks its rights, filters what it may not read, and writes
/// the audit row for writes and for every denied call.
pub fn call_as(repo: &Repo, c: &Caller, op: &str, p: Value) -> Result<Value> {
    // These run (in part) outside the repo lock: a long poll, test runs and index builds must
    // not block other agents.
    match op {
        "events" => {
            let a: EventsP = params(p)?;
            let f = Filter { change: a.change, agent: a.agent, kinds: a.kinds };
            let wait = std::time::Duration::from_millis(a.wait_ms.unwrap_or(0).min(30_000));
            return Ok(repo.events.poll(a.since.unwrap_or(0), &f, a.limit.unwrap_or(200).min(5000), wait));
        }
        "queue_run" | "test_main" | "bisect" => {
            let out = outside(repo, c, op, p.clone());
            let rec = repo.with(|r| Ok(record(r, c, op, &p, &out)));
            if let Ok(rec) = rec {
                let _ = repo.with(|r| r.audit_add(rec));
            }
            return out;
        }
        "token_create" | "token_list" | "token_revoke" => return token_op(repo, c, op, p),
        "grep" | "symbols" => prepare(repo, op, &p)?,
        _ => {}
    }
    repo.with(|r| {
        let out = dispatch(r, c, op, p.clone());
        if AUDITED.contains(&op) || out.as_ref().is_err_and(|e| e.downcast_ref::<Denied>().is_some()) {
            let rec = record(r, c, op, &p, &out);
            if let Err(e) = r.audit_add(rec) {
                eprintln!("treeyard: audit write failed for {op}: {e:#}");
            }
        }
        out
    })
}

fn outside(repo: &Repo, c: &Caller, op: &str, p: Value) -> Result<Value> {
    match op {
        "queue_run" => {
            c.require_admin("queue_run")?;
            landing::queue_step(repo)
        }
        "test_main" => {
            c.require_admin("test_main")?;
            landing::optimistic_step(repo)
        }
        _ => {
            c.require_read_all("bisect")?;
            if !c.writes_any() {
                return Err(deny("bisect runs commands on the server: it needs a token that may write"));
            }
            let a: BisectP = params(p)?;
            let args = bisect::Args { good: a.good, bad: a.bad, command: a.command, verify: a.verify.unwrap_or(true) };
            bisect::run(repo, &args)
        }
    }
}

/// Builds missing derived data (trigrams, symbols) for the query in chunks, outside the lock.
fn prepare(repo: &Repo, op: &str, p: &Value) -> Result<()> {
    let at = p.get("at").and_then(Value::as_str).map(str::to_string);
    let (mode, main, cs) = repo.with(|r| {
        let mode = if op == "grep" { r.cfg.index.search } else { r.cfg.index.symbols };
        Ok((mode, r.main()?, r.resolve_at(at.as_deref()).ok()))
    })?;
    if op == "grep" {
        if mode != IndexMode::Off && p.get("index").and_then(Value::as_bool) != Some(false) {
            search::prepare(repo, &main)?;
        }
    } else if let Some(cs) = cs {
        symbols::prepare(repo, &cs)?;
    }
    Ok(())
}

/// The change `id`, which a restricted token may only touch when it owns it.
fn owned(r: &Inner, c: &Caller, id: &str) -> Result<Change> {
    let ch = r.change(id)?;
    if c.restricted() && ch.agent != c.name {
        return Err(deny(format!("change {id} belongs to {}, not to token {}", ch.agent, c.name)));
    }
    Ok(ch)
}

/// The agent name of a new change: a restricted token always acts as itself.
fn agent_for(c: &Caller, given: Option<String>) -> Result<String> {
    if c.restricted() {
        return match given {
            Some(a) if a != c.name => Err(deny(format!("token {} may not act as agent {a}", c.name))),
            _ => Ok(c.name.clone()),
        };
    }
    match given {
        Some(a) => Ok(a),
        None if c.token.is_some() => Ok(c.name.clone()),
        None => bail!("agent is required"),
    }
}

fn check_writes(r: &Inner, c: &Caller, ch: &Change) -> Result<()> {
    if c.writes_all() {
        return Ok(());
    }
    if !c.writes_any() {
        return Err(deny(format!("token {} is read-only", c.name)));
    }
    for p in r.own_changes(ch)?.keys() {
        c.require_write(p)?;
    }
    Ok(())
}

/// Removes array items whose `key` names a path `c` may not read.
fn keep_readable(v: &mut Value, key: &str, c: &Caller, field: &str) {
    if c.reads_all() {
        return;
    }
    if let Some(a) = v.get_mut(key).and_then(Value::as_array_mut) {
        a.retain(|x| x.get(field).or(Some(x)).and_then(Value::as_str).is_some_and(|p| c.can_read(p)));
    }
}

fn filter_change(v: &mut Value, c: &Caller) {
    keep_readable(v, "paths", c, "");
    keep_readable(v, "conflicts", c, "path");
}

fn dispatch(r: &mut Inner, c: &Caller, op: &str, p: Value) -> Result<Value> {
    match op {
        "status" => r.status(),
        "whoami" => Ok(json!({
            "name": c.name, "token": c.token, "admin": c.admin, "read": c.read.patterns, "write": c.write.patterns,
        })),
        "read" => {
            let a: At = params(p)?;
            let path = a.path.ok_or_else(|| anyhow!("read needs path"))?;
            c.require_read(&path)?;
            r.read(&path, a.at.as_deref())
        }
        "list" => {
            let a: At = params(p)?;
            let dir = a.path.as_deref().unwrap_or("");
            let mut v = r.list(dir, a.at.as_deref())?;
            if !c.reads_all() {
                let m = r.manifest(v["at"].as_str().unwrap_or_default())?;
                let d = dir.trim_matches('/');
                let full = |n: &str| if d.is_empty() { n.to_string() } else { format!("{d}/{n}") };
                if let Some(a) = v["entries"].as_array_mut() {
                    a.retain(|e| {
                        let p = full(e["name"].as_str().unwrap_or_default());
                        if e["kind"] == "dir" {
                            let pre = format!("{p}/");
                            m.range(pre.clone()..).take_while(|(k, _)| k.starts_with(&pre)).any(|(k, _)| c.can_read(k))
                        } else {
                            c.can_read(&p)
                        }
                    });
                }
            }
            Ok(v)
        }
        "tree" => {
            let a: At = params(p)?;
            let mut v = r.tree(a.at.as_deref())?;
            if !c.reads_all()
                && let Some(rows) = v["entries"].as_array_mut()
            {
                rows.retain(|e| e[0].as_str().is_some_and(|p| c.can_read(p)));
                v["files"] = json!(rows.len());
            }
            Ok(v)
        }
        "blobs" => {
            let a: Blobs = params(p)?;
            if !c.reads_all() {
                let main = r.main()?;
                let m = r.manifest(&main)?;
                let ok: std::collections::HashSet<&String> =
                    m.iter().filter(|(p, _)| c.can_read(p)).map(|(_, e)| &e.blob).collect();
                if let Some(id) = a.ids.iter().find(|i| !ok.contains(i)) {
                    return Err(deny(format!("blob {id} is not at a path token {} may read", c.name)));
                }
            }
            r.blobs(&a.ids, a.max_bytes.unwrap_or(16 << 20).min(64 << 20))
        }
        "grep" => {
            let a: Grep = params(p)?;
            let allow = |p: &str| c.can_read(p);
            let max = a.max.unwrap_or(100);
            if a.index == Some(false) || r.cfg.index.search == IndexMode::Off {
                r.grep_scan(&a.pattern, a.at.as_deref(), a.path.as_deref(), max, &allow)
            } else {
                r.grep_indexed(&a.pattern, a.at.as_deref(), a.path.as_deref(), max, &allow)
            }
        }
        "symbols" => {
            let a: SymbolsP = params(p)?;
            let q = symbols::SymbolQuery {
                name: a.name,
                prefix: a.prefix,
                kind: a.kind,
                path: a.path,
                max: a.max.unwrap_or(200).min(10_000),
            };
            r.symbols(a.at.as_deref(), &q, &|p: &str| c.can_read(p))
        }
        "outline" => {
            let a: At = params(p)?;
            let path = a.path.ok_or_else(|| anyhow!("outline needs path"))?;
            c.require_read(&path)?;
            r.outline(&path, a.at.as_deref())
        }
        "blame" => {
            let a: Blame = params(p)?;
            c.require_read(&a.path)?;
            let range = match (a.start, a.end) {
                (None, None) => None,
                (s, e) => Some((s.unwrap_or(1), e.unwrap_or(u32::MAX))),
            };
            r.blame(&a.path, a.at.as_deref(), range)
        }
        "diff" => {
            let a: Diff = params(p)?;
            let (from, to) = match (a.change, a.from) {
                (Some(ch), None) => {
                    let ch = r.change(&ch)?;
                    (ch.base, ch.head)
                }
                (None, Some(from)) => (r.resolve_at(Some(&from))?, r.resolve_at(a.to.as_deref())?),
                _ => bail!("diff needs either change, or from (and optionally to)"),
            };
            let mut v = r.diff(&from, &to)?;
            keep_readable(&mut v, "files", c, "path");
            Ok(v)
        }
        "log" => {
            let a: Log = params(p)?;
            let q = history::Query {
                path: a.path,
                globs: a.paths,
                agent: a.agent,
                since: a.since,
                until: a.until,
                limit: a.limit.unwrap_or(20).min(10_000),
                scan: a.scan.unwrap_or(false),
            };
            let mut v = r.history(a.at.as_deref(), &q)?;
            if !c.reads_all()
                && let Some(es) = v["entries"].as_array_mut()
            {
                for e in es.iter_mut() {
                    keep_readable(e, "paths", c, "");
                }
                es.retain(|e| e["paths"].as_array().is_some_and(|p| !p.is_empty()));
            }
            Ok(v)
        }
        "audit" => {
            if !c.admin {
                c.require_read_all("audit")?;
            }
            let a: AuditP = params(p)?;
            r.audit_query(&audit::Query {
                agent: a.agent,
                actor: a.actor,
                change: a.change,
                op: a.op,
                status: a.status,
                path: a.path,
                since: a.since,
                until: a.until,
                before: a.before,
                limit: a.limit.unwrap_or(100),
            })
        }
        "open_change" => {
            let a: Open = params(p)?;
            if !c.writes_any() {
                return Err(deny(format!("token {} is read-only", c.name)));
            }
            let agent = agent_for(c, a.agent)?;
            let declared = Intent { paths: a.paths, symbols: a.symbols };
            let ch = r.open_change_with(&agent, &a.intent, a.model, a.session, a.base.as_deref(), declared)?;
            let overlaps = r.intent_overlaps(&ch)?;
            let mut v = r.change_view(&ch)?;
            v["advice"] = advice(&overlaps);
            v["overlaps"] = Value::Array(overlaps);
            Ok(v)
        }
        "declare" => {
            let a: Declare = params(p)?;
            owned(r, c, &a.change)?;
            let ch = r.declare(&a.change, Intent { paths: a.paths, symbols: a.symbols })?;
            let overlaps = r.intent_overlaps(&ch)?;
            let mut v = r.change_view(&ch)?;
            v["advice"] = advice(&overlaps);
            v["overlaps"] = Value::Array(overlaps);
            Ok(v)
        }
        "apply" => {
            let a: Apply = params(p)?;
            owned(r, c, &a.change)?;
            for e in &a.edits {
                c.require_write(&e.path)?;
            }
            let ch = r.apply_seen(&a.change, &a.edits, a.message, a.seen.as_deref())?;
            let mut v = r.change_view(&ch)?;
            v["overlaps"] = Value::Array(r.overlaps(&ch)?);
            v["collisions"] = Value::Array(r.collisions(&ch)?);
            filter_change(&mut v, c);
            Ok(v)
        }
        "resolve" => {
            let a: Resolve = params(p)?;
            owned(r, c, &a.change)?;
            c.require_write(&a.path)?;
            let ch = r.resolve(&a.change, &a.path, &a.take)?;
            r.change_view(&ch)
        }
        "rebase" => {
            let a: ChangeId = params(p)?;
            owned(r, c, &a.change)?;
            let ch = r.rebase(&a.change)?;
            let mut v = r.change_view(&ch)?;
            filter_change(&mut v, c);
            Ok(v)
        }
        "land" => {
            let a: ChangeId = params(p)?;
            let ch = owned(r, c, &a.change)?;
            check_writes(r, c, &ch)?;
            r.land(&a.change)
        }
        "abandon" => {
            let a: ChangeId = params(p)?;
            owned(r, c, &a.change)?;
            let ch = r.abandon(&a.change)?;
            r.change_view(&ch)
        }
        "change" => {
            let a: ChangeId = params(p)?;
            let ch = r.change(&a.change)?;
            let mut v = r.change_view(&ch)?;
            v["overlaps"] = Value::Array(r.overlaps(&ch)?);
            v["collisions"] = Value::Array(r.collisions(&ch)?);
            v["reviews"] = r.reviews_view(Some("open"), Some(&ch.id))?["reviews"].take();
            filter_change(&mut v, c);
            Ok(v)
        }
        "reviews" => {
            let a: Reviews = params(p)?;
            r.reviews_view(Some(a.state.as_deref().unwrap_or("open")).filter(|s| *s != "all"), a.change.as_deref())
        }
        "review_close" => {
            let a: ReviewClose = params(p)?;
            if c.restricted() {
                let owner = r.store.reviews(None)?.into_iter().find(|x| x.0 == a.id).map(|x| x.1);
                match owner {
                    Some(ch) => drop(owned(r, c, &ch)?),
                    None => bail!("no review {}", a.id),
                }
            }
            let state = a.state.unwrap_or_else(|| "done".into());
            if !["done", "dismissed"].contains(&state.as_str()) {
                bail!("state must be done or dismissed");
            }
            if !r.store.close_review(a.id, &state)? {
                bail!("no review {}", a.id);
            }
            Ok(json!({"id": a.id, "state": state}))
        }
        "queue" => r.queue_view(),
        "config" => {
            c.require_admin("config")?;
            Ok(json!({"config": r.cfg}))
        }
        "changes" => {
            let a: Changes = params(p)?;
            let list = r.store.changes(Some(a.state.as_deref().unwrap_or("open")))?;
            let mut views = list.iter().map(|ch| r.change_view(ch)).collect::<Result<Vec<_>>>()?;
            for v in &mut views {
                filter_change(v, c);
            }
            Ok(json!({"changes": views}))
        }
        "revert" => {
            let a: Revert = params(p)?;
            let agent = agent_for(c, a.agent)?;
            let target = r.resolve_at(Some(&a.changeset))?;
            if !c.writes_all() {
                for path in r.changeset(&target)?.changes.keys() {
                    c.require_write(path)?;
                }
            }
            r.revert(&target, &agent)
        }
        "export_change" => {
            let a: ChangeId = params(p)?;
            r.export_change(&a.change)
        }
        "tools" => Ok(json!({"tools": tools()})),
        "token_create" | "token_list" | "token_revoke" => {
            c.require_admin(op)?;
            Err(anyhow!("internal: {op} is handled by the auth store"))
        }
        _ => bail!("unknown operation {op:?}; see the tools operation for the list"),
    }
}

/// Token operations: admin only, on the auth store (`Repo::auth`), audited like writes.
pub fn token_op(repo: &Repo, c: &Caller, op: &str, p: Value) -> Result<Value> {
    let out = (|| -> Result<Value> {
        c.require_admin(op)?;
        match op {
            "token_create" => {
                let a: TokenCreate = params(p.clone())?;
                let read = a.read.unwrap_or_else(|| vec!["**".into()]);
                let (id, token) = repo.auth.create(&a.name, a.admin, &read, &a.write)?;
                Ok(json!({"id": id, "name": a.name, "admin": a.admin, "read": read, "write": a.write, "token": token,
                    "note": "shown once: the server keeps only a hash"}))
            }
            "token_list" => repo.auth.list(),
            _ => {
                let a: TokenId = params(p.clone())?;
                if !repo.auth.revoke(&a.id)? {
                    bail!("no active token {}", a.id);
                }
                Ok(json!({"id": a.id, "revoked": true}))
            }
        }
    })();
    if op != "token_list" || out.as_ref().is_err_and(|e| e.downcast_ref::<Denied>().is_some()) {
        let _ = repo.with(|r| {
            let mut rec = record(r, c, op, &p, &out);
            if let Ok(v) = &out {
                // Never the secret: only id and name.
                rec.detail =
                    Some(format!("token {} ({})", v["id"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or("")));
            }
            r.audit_add(rec)
        });
    }
    out
}

/// The audit row of one call.
pub fn record(r: &mut Inner, c: &Caller, op: &str, p: &Value, out: &Result<Value>) -> Record {
    let mut rec = Record { actor: c.name.clone(), token: c.token.clone(), op: op.to_string(), ..Record::default() };
    match out {
        Ok(v) => rec.status = v["status"].as_str().unwrap_or("ok").to_string(),
        Err(e) => {
            rec.status = if e.downcast_ref::<Denied>().is_some() { "denied" } else { "error" }.to_string();
            let msg = format!("{e:#}");
            rec.detail = Some(msg.chars().take(500).collect());
        }
    }
    let v = out.as_ref().ok();
    let id = p["change"]
        .as_str()
        .or_else(|| v.and_then(|v| if op == "open_change" { v["id"].as_str() } else { v["change"].as_str() }))
        .map(str::to_string);
    let ch = id.as_deref().and_then(|i| r.change(i).ok());
    match &ch {
        Some(ch) => {
            rec.change = Some(ch.id.clone());
            rec.agent = Some(ch.agent.clone());
            rec.model = ch.model.clone();
            rec.session = ch.session.clone();
            rec.intent = Some(ch.intent.clone());
        }
        None => {
            rec.change = id;
            rec.agent = p["agent"].as_str().map(str::to_string).or_else(|| c.token.as_ref().map(|_| c.name.clone()));
            rec.model = p["model"].as_str().map(str::to_string);
            rec.session = p["session"].as_str().map(str::to_string);
            rec.intent = p["intent"].as_str().map(str::to_string);
        }
    }
    let strs = |x: &Value| -> Vec<String> {
        x.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    rec.paths = match op {
        "apply" => p["edits"]
            .as_array()
            .map(|a| a.iter().filter_map(|e| e["path"].as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        "resolve" | "read" | "blame" | "outline" => p["path"].as_str().map(|s| vec![s.to_string()]).unwrap_or_default(),
        "open_change" | "declare" => strs(&p["paths"]),
        "revert" => v
            .and_then(|v| v["changeset"].as_str())
            .and_then(|cs| r.changeset(cs).ok())
            .map(|cs| cs.changes.into_keys().collect())
            .unwrap_or_default(),
        _ => ch.as_ref().and_then(|ch| r.own_changes(ch).ok()).map(|m| m.into_keys().collect()).unwrap_or_default(),
    };
    if let Some(v) = v {
        rec.changeset = v["changeset"].as_str().map(str::to_string);
        rec.commit = v["commit"].as_str().map(str::to_string);
        if op == "bisect" {
            rec.changeset = v["first_bad"]["changeset"].as_str().map(str::to_string);
            rec.detail = Some(format!("first bad of {} candidates after {} tests", v["candidates"], v["tests"]));
        }
    }
    rec
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
    let strings = json!({"type": "array", "items": {"type": "string"}, "description": "path globs you will edit, e.g. src/parser/** or Cargo.toml"});
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
            "tree",
            "Every file of a revision as [path, mode, blob] rows; a node daemon builds workspaces from it.",
            json!({"at": at}),
            &[],
        ),
        tool(
            "blobs",
            "Blob contents as base64 by id, up to max_bytes (default 16 MiB) per call; ids that did not fit come back in rest.",
            json!({"ids": {"type": "array", "items": {"type": "string"}}, "max_bytes": {"type": "integer"}}),
            &["ids"],
        ),
        tool(
            "grep",
            "Search file contents with a regular expression (per line), server side, through a trigram index.",
            json!({"pattern": {"type": "string"}, "path": {"type": "string", "description": "limit to this directory"}, "at": at, "max": {"type": "integer", "description": "max matches, default 100"}, "index": {"type": "boolean", "description": "false = scan every file"}}),
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
            "History of main (or of at), newest first, from the changed-path index. Filters: path (file or directory), paths (globs: only changesets touching a match), agent, since/until (unix seconds).",
            json!({"path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "agent": {"type": "string"}, "since": {"type": "integer"}, "until": {"type": "integer"}, "at": at, "limit": {"type": "integer"}, "scan": {"type": "boolean", "description": "walk changesets instead of the index (for comparison)"}}),
            &[],
        ),
        tool(
            "blame",
            "Who last changed each line of a file: hunks of lines with their origin changeset (agent, model, session, intent) or, before the import, the git commit. Same answer as git blame --first-parent.",
            json!({"path": {"type": "string"}, "at": at, "start": {"type": "integer"}, "end": {"type": "integer"}}),
            &["path"],
        ),
        tool(
            "symbols",
            "Find definitions (functions, methods, types, classes, traits, interfaces, modules, constants) by exact name or prefix in Rust, Python, Go, JavaScript and TypeScript files (tree-sitter index).",
            json!({"name": {"type": "string"}, "prefix": {"type": "string"}, "kind": {"type": "string", "description": "function, method, class, interface, module, type, macro, constant"}, "path": {"type": "string", "description": "limit to this directory or file"}, "at": at, "max": {"type": "integer"}}),
            &[],
        ),
        tool(
            "outline",
            "Every definition in one file, in line order (tree-sitter).",
            json!({"path": {"type": "string"}, "at": at}),
            &["path"],
        ),
        tool(
            "bisect",
            "Find the first bad changeset between good and bad (default main) on the first-parent history: the server runs a test command from treeyard.toml on temporary checkouts. Then call revert on it if you want it gone.",
            json!({"good": at, "bad": at, "command": {"type": "string", "description": "name of a command in [bisect.commands]; default [bisect] test or landing.test"}, "verify": {"type": "boolean", "description": "test good and bad first (default true)"}}),
            &["good"],
        ),
        tool(
            "audit",
            "Audit log, newest first: who (token, agent, model, session, intent) changed what (paths, changeset, commit), when, and denied calls. Filters: agent, actor, change, op, status, path, since, until; page with before.",
            json!({"agent": {"type": "string"}, "actor": {"type": "string"}, "change": change, "op": {"type": "string"}, "status": {"type": "string"}, "path": {"type": "string"}, "since": {"type": "integer"}, "until": {"type": "integer"}, "before": {"type": "integer"}, "limit": {"type": "integer"}}),
            &[],
        ),
        tool("whoami", "The caller's token name, admin flag and read/write globs.", json!({}), &[]),
        tool(
            "token_create",
            "Admin: create an API token (shown once). A non-admin token acts as agent `name` and may write only paths matching `write`.",
            json!({"name": {"type": "string"}, "admin": {"type": "boolean"}, "read": {"type": "array", "items": {"type": "string"}}, "write": {"type": "array", "items": {"type": "string"}}}),
            &["name"],
        ),
        tool("token_list", "Admin: list tokens (no secrets).", json!({}), &[]),
        tool("token_revoke", "Admin: revoke a token by id.", json!({"id": {"type": "string"}}), &["id"]),
        tool(
            "open_change",
            "Start a change on top of main (no branch). Declare the paths and symbols you plan to touch: the reply lists open changes that overlap them, with advice. Edit with apply, then land.",
            json!({"agent": {"type": "string"}, "intent": {"type": "string", "description": "what you are about to do, in one line"}, "model": {"type": "string"}, "session": {"type": "string"}, "base": {"type": "string", "description": "main (default) or another revision; a change id ch_... stacks on that change, which then lands first"}, "paths": strings.clone(), "symbols": {"type": "array", "items": {"type": "string"}, "description": "function, type or class names you will change; name or path#name"}}),
            &["intent"],
        ),
        tool(
            "declare",
            "Replace the paths and symbols a change plans to touch; returns overlaps with other open changes.",
            json!({"change": change, "paths": strings.clone(), "symbols": {"type": "array", "items": {"type": "string"}}}),
            &["change"],
        ),
        tool(
            "events",
            "Coordination events after seq `since`: opened, intent_overlap, collision, conflict, rebased, landed, review, queued, rejected, test_failed, reverted. Long-poll with wait_ms (max 30000). Pass the reply's next as since. Also GET /api/events (server-sent events).",
            json!({"since": {"type": "integer"}, "change": change, "agent": {"type": "string"}, "kinds": {"type": "array", "items": {"type": "string"}}, "limit": {"type": "integer"}, "wait_ms": {"type": "integer"}}),
            &[],
        ),
        tool(
            "reviews",
            "Review tasks: edits a rebase made vanish (absorbed_edit), changes already on main, failed tests, bisect reverts.",
            json!({"state": {"type": "string", "enum": ["open", "done", "dismissed", "all"]}, "change": change}),
            &[],
        ),
        tool(
            "review_close",
            "Close a review task.",
            json!({"id": {"type": "integer"}, "state": {"type": "string", "enum": ["done", "dismissed"]}}),
            &["id"],
        ),
        tool("queue", "The landing queue (landing mode queue).", json!({}), &[]),
        tool(
            "queue_run",
            "Run one queue step now: test the next batch together, bisect a failing batch, land what passed.",
            json!({}),
            &[],
        ),
        tool(
            "test_main",
            "Optimistic mode: test main now; on failure bisect since the last green main and revert the culprit.",
            json!({}),
            &[],
        ),
        tool("config", "The repo settings in effect (treeyard.toml).", json!({}), &[]),
        tool(
            "apply",
            "Edit files in a change: per edit give content, content_b64, delete:true, replace:{old,new,all?}, and optionally executable. Returns the change, other open changes touching the same paths, and live collisions (trial merge: conflict or overlap, with hunk ranges).",
            json!({"change": change, "seen": {"type": "string", "description": "the revision your whole-file contents are based on (e.g. the head you read); the server merges if the change moved since"}, "message": {"type": "string", "description": "commit message; defaults to the intent"}, "edits": {"type": "array", "items": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}, "content_b64": {"type": "string"}, "delete": {"type": "boolean"}, "replace": {"type": "object", "properties": {"old": {"type": "string"}, "new": {"type": "string"}, "all": {"type": "boolean"}}, "required": ["old", "new"]}, "executable": {"type": "boolean"}, "symlink": {"type": "boolean", "description": "content is a symlink target"}}, "required": ["path"]}}}),
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
            "Land a change on main: rebases on the server if main moved. Returns status landed (with the git commit), queued (landing mode queue), already_on_main (main had every edit; a review task is opened), waiting (stacked on a change that has not landed), or conflict (with tasks). Also lists merge drivers used and review tasks.",
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
            &["changeset"],
        ),
        tool(
            "export_change",
            "Write a change's head as a git commit under refs/treeyard/changes/<id>.",
            json!({"change": change}),
            &["change"],
        ),
    ]
}
