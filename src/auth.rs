//! API tokens: one per agent (or person), with read and write path globs, and admin tokens.
//!
//! The server keeps only a SHA-256 of each token. Auth turns on with the first token and
//! stays on (revoking every token does not open the server again). With auth off the server
//! is open, as before: every caller acts as `open` with full rights.

use anyhow::{Context, Result, bail};
use globset::GlobSet;
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;

use crate::config::globs;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tokens (id TEXT PRIMARY KEY, hash TEXT NOT NULL, name TEXT NOT NULL,
    admin INTEGER NOT NULL, read TEXT NOT NULL, write TEXT NOT NULL, created INTEGER NOT NULL, revoked INTEGER);
";

/// A request without a valid token while auth is on (HTTP 401).
#[derive(Debug)]
pub struct Unauthorized(pub String);

/// A valid token without the right for this operation or path (HTTP 403).
#[derive(Debug)]
pub struct Denied(pub String);

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unauthorized: {}", self.0)
    }
}
impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "permission denied: {}", self.0)
    }
}
impl std::error::Error for Unauthorized {}
impl std::error::Error for Denied {}

pub fn deny(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Denied(msg.into()))
}

/// Path globs of a token (gitignore style, as in rootstock.toml); `**` means every path.
#[derive(Clone, Debug)]
pub struct Rules {
    pub patterns: Vec<String>,
    set: GlobSet,
    all: bool,
}

impl Rules {
    pub fn new(patterns: &[String]) -> Result<Self> {
        let all = patterns.iter().any(|p| matches!(p.trim(), "**" | "*" | "/**" | "**/*"));
        Ok(Self { patterns: patterns.to_vec(), set: globs(patterns)?, all })
    }
    pub fn everything() -> Self {
        Self::new(&["**".to_string()]).expect("the ** glob compiles")
    }
    pub fn matches(&self, path: &str) -> bool {
        self.all || self.set.is_match(path)
    }
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Who makes a call: the token's name, admin flag and path rules.
#[derive(Clone, Debug)]
pub struct Caller {
    pub name: String,
    /// The token id (`None` for in-process calls and an open server).
    pub token: Option<String>,
    pub admin: bool,
    pub read: Rules,
    pub write: Rules,
}

impl Caller {
    /// In-process calls (tests, replay, the landing worker): full rights.
    pub fn internal() -> Self {
        Self {
            name: "internal".into(),
            token: None,
            admin: true,
            read: Rules::everything(),
            write: Rules::everything(),
        }
    }
    /// Any caller while auth is off.
    pub fn open() -> Self {
        Self { name: "open".into(), ..Self::internal() }
    }
    pub fn can_read(&self, path: &str) -> bool {
        self.admin || self.read.matches(path)
    }
    pub fn can_write(&self, path: &str) -> bool {
        self.admin || self.write.matches(path)
    }
    pub fn reads_all(&self) -> bool {
        self.admin || self.read.all
    }
    pub fn writes_all(&self) -> bool {
        self.admin || self.write.all
    }
    pub fn writes_any(&self) -> bool {
        self.admin || !self.write.is_empty()
    }
    /// A token caller that is not admin: identity and ownership are enforced.
    pub fn restricted(&self) -> bool {
        self.token.is_some() && !self.admin
    }
    pub fn require_admin(&self, what: &str) -> Result<()> {
        if !self.admin {
            return Err(deny(format!("{what} needs an admin token")));
        }
        Ok(())
    }
    pub fn require_read(&self, path: &str) -> Result<()> {
        if !self.can_read(path) {
            return Err(deny(format!("token {} may not read {path}", self.name)));
        }
        Ok(())
    }
    pub fn require_write(&self, path: &str) -> Result<()> {
        if !self.can_write(path) {
            return Err(deny(format!(
                "token {} may not write {path} (write globs: {:?})",
                self.name, self.write.patterns
            )));
        }
        Ok(())
    }
    pub fn require_read_all(&self, what: &str) -> Result<()> {
        if !self.reads_all() {
            return Err(deny(format!("{what} needs a token that reads every path (read = [\"**\"])")));
        }
        Ok(())
    }
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Compares two equal-length strings without an early exit.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token table, on its own SQLite connection so a check never waits for the repo lock.
pub struct Auth {
    conn: Mutex<Connection>,
}

impl Auth {
    pub fn open(db: &Path) -> Result<Self> {
        let conn = Connection::open(db).with_context(|| format!("opening {}", db.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Auth is on once any token was ever created.
    pub fn enabled(&self) -> Result<bool> {
        Ok(self.conn().query_row("SELECT EXISTS (SELECT 1 FROM tokens)", [], |r| r.get(0))?)
    }

    /// The caller for a bearer token (or none). Errors are `Unauthorized`.
    pub fn authenticate(&self, token: Option<&str>) -> Result<Caller> {
        if !self.enabled()? {
            return Ok(Caller::open());
        }
        let unauth = |m: &str| anyhow::Error::new(Unauthorized(m.to_string()));
        let token = token.map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| {
            unauth("this server needs a token: send Authorization: Bearer <token> (set ROOTSTOCK_TOKEN for the CLI and MCP)")
        })?;
        let id =
            token.strip_prefix("rst_").and_then(|t| t.split('_').next()).ok_or_else(|| unauth("malformed token"))?;
        let row: Option<(String, String, bool, String, String, Option<i64>)> = self
            .conn()
            .query_row("SELECT hash, name, admin, read, write, revoked FROM tokens WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })
            .optional()?;
        let Some((h, name, admin, read, write, revoked)) = row else { return Err(unauth("unknown token")) };
        if !same(&h, &hash(token)) {
            return Err(unauth("unknown token"));
        }
        if revoked.is_some() {
            return Err(unauth(&format!("token {id} is revoked")));
        }
        let read: Vec<String> = serde_json::from_str(&read)?;
        let write: Vec<String> = serde_json::from_str(&write)?;
        Ok(Caller { name, token: Some(id.to_string()), admin, read: Rules::new(&read)?, write: Rules::new(&write)? })
    }

    /// Creates a token and returns (id, secret). The secret is shown once and never stored.
    pub fn create(&self, name: &str, admin: bool, read: &[String], write: &[String]) -> Result<(String, String)> {
        let name = name.trim();
        if name.is_empty() || name.len() > 100 || name.contains(char::is_whitespace) {
            bail!("token name must be one word of up to 100 characters (it becomes the agent name)");
        }
        Rules::new(read)?;
        Rules::new(write)?;
        let mut rng = rand::thread_rng();
        let id = format!("{:012x}", rng.r#gen::<u64>() & 0xffff_ffff_ffff);
        let secret: String = (0..2).map(|_| format!("{:016x}", rng.r#gen::<u64>())).collect();
        let token = format!("rst_{id}_{secret}");
        self.conn().execute(
            "INSERT INTO tokens (id, hash, name, admin, read, write, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                hash(&token),
                name,
                admin,
                serde_json::to_string(read)?,
                serde_json::to_string(write)?,
                crate::repo::now()
            ],
        )?;
        Ok((id, token))
    }

    pub fn list(&self) -> Result<Value> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, name, admin, read, write, created, revoked FROM tokens ORDER BY created, id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "admin": r.get::<_, bool>(2)?,
                    "read": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                    "write": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                    "created": r.get::<_, i64>(5)?, "revoked": r.get::<_, Option<i64>>(6)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"tokens": rows}))
    }

    pub fn revoke(&self, id: &str) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE tokens SET revoked = ?2 WHERE id = ?1 AND revoked IS NULL",
            params![id, crate::repo::now()],
        )?;
        Ok(n == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_check_hash_name_and_revocation() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Auth::open(&tmp.path().join("db")).unwrap();
        assert_eq!(a.authenticate(None).unwrap().name, "open");
        let (id, tok) = a.create("agent-1", false, &["**".into()], &["src/**".into()]).unwrap();
        let c = a.authenticate(Some(&tok)).unwrap();
        assert!(c.reads_all() && c.can_write("src/a.rs") && !c.can_write("Cargo.toml") && !c.admin);
        assert!(a.authenticate(None).unwrap_err().downcast_ref::<Unauthorized>().is_some());
        let forged = format!("rst_{id}_{}", "0".repeat(32));
        assert!(a.authenticate(Some(&forged)).is_err());
        assert!(a.revoke(&id).unwrap());
        assert!(a.authenticate(Some(&tok)).is_err());
        assert!(a.enabled().unwrap(), "auth stays on after the last token is revoked");
    }
}
