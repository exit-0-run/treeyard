//! Attachments of a change: transcripts, summaries and notes an agent stores with its work.
//! They live in SQLite while the change is open (never in git objects); on land they become
//! one git note on the landed commit under `refs/notes/treeyard`, written deterministically
//! (note commit dates come from the landed changeset). Lines that look like secrets are dropped
//! when the text comes in.

use anyhow::{Result, bail};
use regex::RegexSet;
use rusqlite::params;
use serde_json::{Value, json};
use std::sync::OnceLock;

use crate::git::{self, Signature};
use crate::model::{Changes, Entry, MODE_FILE, Manifest};
use crate::repo::{Inner, now};

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS attachments (id INTEGER PRIMARY KEY AUTOINCREMENT, change TEXT NOT NULL,
    kind TEXT NOT NULL, agent TEXT NOT NULL, text TEXT NOT NULL, dropped INTEGER NOT NULL, time INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS attachments_change ON attachments (change, id);
";

/// Largest text of one attachment, and of all attachments of one change.
pub const MAX_ATTACHMENT: usize = 256 * 1024;
pub const MAX_PER_CHANGE: usize = 4 << 20;
pub const KINDS: &[&str] = &["transcript", "summary", "note"];
pub const NOTES_REF: &str = "refs/notes/treeyard";

/// Lines with one of these are dropped: API keys and tokens by their known prefixes, and
/// `key = value` lines whose name says secret. Private key blocks are dropped whole.
const SECRET_LINES: &[&str] = &[
    r"\bsk-[A-Za-z0-9_-]{16,}",
    r"\bgh[pousr]_[A-Za-z0-9]{20,}",
    r"\bgithub_pat_[A-Za-z0-9_]{20,}",
    r"\b(AKIA|ASIA)[0-9A-Z]{16}\b",
    r"\bxox[abprse]-[A-Za-z0-9-]{10,}",
    r"\bglpat-[A-Za-z0-9_-]{20,}",
    r"\bAIza[0-9A-Za-z_-]{35}\b",
    r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{20,}",
    r#"(?i)(api[_-]?key|secret|passw(or)?d|token|credential)s?["']?\s*[:=]\s*["']?[^\s"']{8,}"#,
];

fn secret_lines() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| RegexSet::new(SECRET_LINES).expect("secret patterns compile"))
}

/// The text without lines that look like secrets, and how many lines were dropped.
pub fn redact(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let (mut dropped, mut in_key) = (0, false);
    for line in text.split_inclusive('\n') {
        let begins = line.contains("-----BEGIN") && line.contains("PRIVATE KEY");
        if begins || in_key {
            in_key = !(line.contains("-----END") && line.contains("PRIVATE KEY"));
            dropped += 1;
        } else if secret_lines().is_match(line) {
            dropped += 1;
        } else {
            out.push_str(line);
        }
    }
    (out, dropped)
}

impl Inner {
    /// Stores a transcript, summary or note with an open change (redacted).
    pub fn attach(&mut self, id: &str, kind: &str, text: &str) -> Result<Value> {
        let ch = self.change(id)?;
        if ch.state != crate::model::ChangeState::Open {
            bail!("change {id} is {}: attach before land", ch.state.as_str());
        }
        if !KINDS.contains(&kind) {
            bail!("kind must be one of {KINDS:?}, not {kind:?}");
        }
        if text.len() > MAX_ATTACHMENT {
            bail!("the text is {} bytes; one attachment holds up to {MAX_ATTACHMENT} bytes", text.len());
        }
        let used: i64 = self.store.conn().query_row(
            "SELECT COALESCE(SUM(LENGTH(CAST(text AS BLOB))), 0) FROM attachments WHERE change = ?1",
            [id],
            |r| r.get(0),
        )?;
        if used as usize + text.len() > MAX_PER_CHANGE {
            bail!("change {id} holds {used} bytes of attachments; the limit is {MAX_PER_CHANGE}");
        }
        let (kept, dropped) = redact(text);
        self.store.conn().execute(
            "INSERT INTO attachments (change, kind, agent, text, dropped, time) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, kind, ch.agent, kept, dropped as i64, now()],
        )?;
        let n = self.store.conn().last_insert_rowid();
        Ok(json!({"id": n, "change": id, "kind": kind, "bytes": kept.len(), "dropped_lines": dropped}))
    }

    /// Attachments of a change, oldest first; for a landed change also where its git note is.
    pub fn attachments(&mut self, id: &str) -> Result<Value> {
        let ch = self.change(id)?;
        let mut stmt = self
            .store
            .conn()
            .prepare("SELECT id, kind, agent, text, dropped, time FROM attachments WHERE change = ?1 ORDER BY id")?;
        let rows = stmt
            .query_map([id], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?, "agent": r.get::<_, String>(2)?,
                    "text": r.get::<_, String>(3)?, "dropped_lines": r.get::<_, i64>(4)?, "time": r.get::<_, i64>(5)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let note = match &ch.landed_as {
            Some(cs) if !rows.is_empty() && ch.head != ch.base => {
                let commit = self.commit_for(cs)?;
                json!({"ref": NOTES_REF, "commit": commit, "show": format!("git notes --ref=treeyard show {commit}")})
            }
            _ => Value::Null,
        };
        Ok(json!({"change": id, "attachments": rows, "note": note}))
    }

    /// The note text of a change's attachments (None without any), from stored rows only.
    fn note_text(&self, id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .store
            .conn()
            .prepare("SELECT kind, agent, text, dropped FROM attachments WHERE change = ?1 ORDER BY id")?;
        let rows = stmt
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get::<_, i64>(3)?)))?
            .collect::<rusqlite::Result<Vec<(String, String, String, i64)>>>()?;
        if rows.is_empty() {
            return Ok(None);
        }
        let mut s = format!("TreeYard attachments of change {id} ({})\n", rows.len());
        for (i, (kind, agent, text, dropped)) in rows.iter().enumerate() {
            let redacted = if *dropped > 0 { format!(", secret lines dropped: {dropped}") } else { String::new() };
            s.push_str(&format!("\n## {kind} {} (agent {agent}{redacted})\n\n{text}", i + 1));
            if !text.ends_with('\n') {
                s.push('\n');
            }
        }
        Ok(Some(s))
    }

    /// Writes the attachments of a landed change as a git note on `commit`. The notes commit
    /// takes its dates from the landed changeset, so the same history gives the same notes.
    pub fn export_notes(&mut self, id: &str, changeset: &str, commit: &str) -> Result<Option<String>> {
        let Some(text) = self.note_text(id)? else { return Ok(None) };
        let blob = self.odb.write("blob", text.as_bytes())?;
        let entry = Entry { blob, mode: MODE_FILE };
        let parent = self.store.get_ref("notes")?;
        let tree = match &parent {
            Some(p) => {
                let parent_tree = git::commit_tree(&mut self.odb, p)?;
                git::write_tree_changed(
                    &mut self.odb,
                    &parent_tree,
                    &Changes::from([(commit.to_string(), Some(entry))]),
                )?
            }
            None => git::write_tree(&mut self.odb, &Manifest::unit(commit.to_string(), entry))?,
        };
        let time = self.changeset(changeset)?.meta.time;
        let sig = Signature { name: "TreeYard".into(), email: "treeyard@localhost".into(), time };
        let msg = format!("Notes for {id}\n");
        let bytes = git::commit_bytes(&tree, &parent.into_iter().collect::<Vec<_>>(), &sig, &sig, &msg);
        let notes = self.odb.write("commit", &bytes)?;
        self.store.set_ref("notes", &notes)?;
        self.odb.update_ref(NOTES_REF, &notes)?;
        Ok(Some(notes))
    }

    /// `export_notes` after a land: main has moved already, so a failure is reported, not raised.
    pub(crate) fn notes_after_land(&mut self, id: &str, changeset: &str, commit: &str) -> Value {
        match self.export_notes(id, changeset, commit) {
            Ok(Some(n)) => json!({"ref": NOTES_REF, "commit": n}),
            Ok(None) => Value::Null,
            Err(e) => {
                eprintln!("treeyard: notes for {id} not written: {e:#}");
                json!({"error": format!("{e:#}")})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_drops_secret_lines_and_key_blocks() {
        let fake_gh = format!("ghp_{}", "a1".repeat(18));
        let text = format!(
            "ran the tests\nexport OPENAI_API_KEY=sk-{}\ntoken: {fake_gh}\n-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----\nAKIA{}\nall green\npassword = hunter2hunter2\nthe token count is 12\n",
            "x".repeat(24),
            "ABCDEFGHIJKLMNOP"
        );
        let (kept, dropped) = redact(&text);
        assert_eq!(kept, "ran the tests\nall green\nthe token count is 12\n");
        assert_eq!(dropped, 7);
    }
}
