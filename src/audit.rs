//! Audit log: one row per write operation and per denied call: who (token and agent), with
//! which model, session and intent, changed which paths, when, and the outcome. Rows are
//! written in the same lock hold as the operation they describe.

use anyhow::Result;
use rusqlite::params;
use serde_json::{Value, json};

use crate::repo::Inner;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS audit (seq INTEGER PRIMARY KEY AUTOINCREMENT, time_ms INTEGER NOT NULL,
    actor TEXT NOT NULL, token TEXT, agent TEXT, model TEXT, session TEXT, intent TEXT, op TEXT NOT NULL,
    change TEXT, changeset TEXT, git_commit TEXT, status TEXT NOT NULL, detail TEXT, paths TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit_paths (path TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (path, seq)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS audit_agent ON audit (agent, seq);
CREATE INDEX IF NOT EXISTS audit_change ON audit (change, seq);
";

/// At most this many paths are kept per row (the count is kept in `detail`).
const MAX_PATHS: usize = 500;

#[derive(Default, Debug, Clone)]
pub struct Record {
    pub actor: String,
    pub token: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub session: Option<String>,
    pub intent: Option<String>,
    pub op: String,
    pub change: Option<String>,
    pub changeset: Option<String>,
    pub commit: Option<String>,
    pub status: String,
    pub detail: Option<String>,
    pub paths: Vec<String>,
}

/// Filters of the `audit` operation.
#[derive(Default)]
pub struct Query {
    pub agent: Option<String>,
    pub actor: Option<String>,
    pub change: Option<String>,
    pub op: Option<String>,
    pub status: Option<String>,
    /// A file or a directory.
    pub path: Option<String>,
    /// Unix seconds, inclusive.
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub before: Option<i64>,
    pub limit: usize,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Inner {
    pub fn audit_add(&self, mut rec: Record) -> Result<i64> {
        rec.paths.sort();
        rec.paths.dedup();
        if rec.paths.len() > MAX_PATHS {
            let n = rec.paths.len();
            rec.paths.truncate(MAX_PATHS);
            rec.detail = Some(format!("{} ({n} paths, first {MAX_PATHS} kept)", rec.detail.unwrap_or_default()));
        }
        let c = self.store.conn();
        c.execute(
            "INSERT INTO audit (time_ms, actor, token, agent, model, session, intent, op, change, changeset, git_commit,
                 status, detail, paths) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                now_ms(),
                rec.actor,
                rec.token,
                rec.agent,
                rec.model,
                rec.session,
                rec.intent,
                rec.op,
                rec.change,
                rec.changeset,
                rec.commit,
                rec.status,
                rec.detail,
                serde_json::to_string(&rec.paths)?
            ],
        )?;
        let seq = c.last_insert_rowid();
        let mut st = c.prepare_cached("INSERT OR IGNORE INTO audit_paths (path, seq) VALUES (?1, ?2)")?;
        for p in &rec.paths {
            st.execute(params![p, seq])?;
        }
        Ok(seq)
    }

    pub fn audit_query(&self, q: &Query) -> Result<Value> {
        let mut sql = String::from(
            "SELECT seq, time_ms, actor, token, agent, model, session, intent, op, change, changeset, git_commit,
                    status, detail, paths FROM audit a WHERE 1 = 1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut add = |cond: &str, v: rusqlite::types::Value| {
            args.push(v);
            sql.push_str(&format!(" AND {cond} ?{}", args.len()));
        };
        if let Some(x) = &q.agent {
            add("agent =", x.clone().into());
        }
        if let Some(x) = &q.actor {
            add("actor =", x.clone().into());
        }
        if let Some(x) = &q.change {
            add("change =", x.clone().into());
        }
        if let Some(x) = &q.op {
            add("op =", x.clone().into());
        }
        if let Some(x) = &q.status {
            add("status =", x.clone().into());
        }
        if let Some(x) = q.since {
            add("time_ms >=", (x * 1000).into());
        }
        if let Some(x) = q.until {
            add("time_ms <", ((x + 1) * 1000).into());
        }
        if let Some(x) = q.before {
            add("seq <", x.into());
        }
        if let Some(p) = q.path.as_deref().map(|p| p.trim_matches('/')).filter(|p| !p.is_empty()) {
            let n = args.len();
            args.push(p.to_string().into());
            args.push(format!("{p}/").into());
            args.push(format!("{p}0").into());
            sql.push_str(&format!(
                " AND seq IN (SELECT seq FROM audit_paths WHERE path = ?{} OR (path >= ?{} AND path < ?{}))",
                n + 1,
                n + 2,
                n + 3
            ));
        }
        sql.push_str(&format!(" ORDER BY seq DESC LIMIT {}", q.limit.clamp(1, 10_000)));
        let mut stmt = self.store.conn().prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(json!({
                    "seq": r.get::<_, i64>(0)?, "time_ms": r.get::<_, i64>(1)?, "actor": r.get::<_, String>(2)?,
                    "token": r.get::<_, Option<String>>(3)?, "agent": r.get::<_, Option<String>>(4)?,
                    "model": r.get::<_, Option<String>>(5)?, "session": r.get::<_, Option<String>>(6)?,
                    "intent": r.get::<_, Option<String>>(7)?, "op": r.get::<_, String>(8)?,
                    "change": r.get::<_, Option<String>>(9)?, "changeset": r.get::<_, Option<String>>(10)?,
                    "commit": r.get::<_, Option<String>>(11)?, "status": r.get::<_, String>(12)?,
                    "detail": r.get::<_, Option<String>>(13)?,
                    "paths": serde_json::from_str::<Value>(&r.get::<_, String>(14)?).unwrap_or(Value::Null),
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next = rows.last().and_then(|r| r["seq"].as_i64());
        Ok(json!({"entries": rows, "before": next}))
    }
}
