//! SQLite metadata store: agent blobs, changesets, changes, refs and the git mapping.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

use crate::model::{Change, Changeset};

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS changesets (id TEXT PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS changes (id TEXT PRIMARY KEY, state TEXT NOT NULL, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS refs (name TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS git_map (changeset TEXT PRIMARY KEY, git_commit TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS git_map_commit ON git_map (git_commit);",
        )?;
        Ok(Self { conn })
    }

    /// Runs `f` in one transaction; rolls back if it fails.
    pub fn tx<R>(&self, f: impl FnOnce(&Self) -> Result<R>) -> Result<R> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f(self) {
            Ok(r) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(r)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn put_blob(&self, id: &str, data: &[u8]) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO blobs (id, data) VALUES (?1, ?2)", params![id, data])?;
        Ok(())
    }

    pub fn blob(&self, id: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.conn.query_row("SELECT data FROM blobs WHERE id = ?1", [id], |r| r.get(0)).optional()?)
    }

    pub fn put_changeset(&self, cs: &Changeset) -> Result<String> {
        let id = cs.id();
        self.conn.execute(
            "INSERT OR IGNORE INTO changesets (id, body) VALUES (?1, ?2)",
            params![id, serde_json::to_string(cs)?],
        )?;
        Ok(id)
    }

    pub fn changeset(&self, id: &str) -> Result<Option<Changeset>> {
        let body: Option<String> =
            self.conn.query_row("SELECT body FROM changesets WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into)).transpose()
    }

    /// Up to two changeset ids starting with `prefix` (two means ambiguous).
    pub fn changesets_by_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM changesets WHERE id >= ?1 AND id < ?2 ORDER BY id LIMIT 2")?;
        let upper = format!("{prefix}\u{7f}");
        let ids = stmt.query_map(params![prefix, upper], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }

    pub fn put_change(&self, ch: &Change) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO changes (id, state, body) VALUES (?1, ?2, ?3)",
            params![ch.id, ch.state.as_str(), serde_json::to_string(ch)?],
        )?;
        Ok(())
    }

    pub fn change(&self, id: &str) -> Result<Option<Change>> {
        let body: Option<String> =
            self.conn.query_row("SELECT body FROM changes WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        body.map(|b| serde_json::from_str(&b).map_err(Into::into)).transpose()
    }

    pub fn changes(&self, state: Option<&str>) -> Result<Vec<Change>> {
        let mut stmt = self.conn.prepare("SELECT body FROM changes WHERE ?1 IS NULL OR state = ?1 ORDER BY rowid")?;
        let bodies = stmt.query_map([state], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        bodies.iter().map(|b| serde_json::from_str(b).map_err(Into::into)).collect()
    }

    pub fn get_ref(&self, name: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM refs WHERE name = ?1", [name], |r| r.get(0)).optional()?)
    }

    pub fn set_ref(&self, name: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO refs (name, value) VALUES (?1, ?2)", params![name, value])?;
        Ok(())
    }

    pub fn commit_of(&self, changeset: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT git_commit FROM git_map WHERE changeset = ?1", [changeset], |r| r.get(0))
            .optional()?)
    }

    pub fn changeset_of_commit(&self, commit: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT changeset FROM git_map WHERE git_commit = ?1", [commit], |r| r.get(0))
            .optional()?)
    }

    pub fn map_commit(&self, changeset: &str, commit: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO git_map (changeset, git_commit) VALUES (?1, ?2)",
            params![changeset, commit],
        )?;
        Ok(())
    }
}
