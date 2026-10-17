//! Core data model: entries, manifests, changesets, changes and conflicts.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const MODE_FILE: u32 = 0o100644;
pub const MODE_EXEC: u32 = 0o100755;
pub const MODE_LINK: u32 = 0o120000;
pub const MODE_GITLINK: u32 = 0o160000;

/// One file in a tree: its git blob id and its git mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub blob: String,
    pub mode: u32,
}

/// The full tree of a changeset, path -> entry. Persistent map, so clones share structure.
pub type Manifest = im::OrdMap<String, Entry>;

/// Changed paths relative to the first parent; `None` is a deletion.
pub type Changes = BTreeMap<String, Option<Entry>>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub change: Option<String>,
    pub agent: String,
    pub model: Option<String>,
    pub session: Option<String>,
    pub intent: String,
    pub message: Option<String>,
    /// Unix seconds; the git commit takes its dates from here, so export is deterministic.
    pub time: i64,
}

/// Immutable, content-addressed set of changes (Bonsai style). The root changeset of an
/// imported repo has no parents and lists the whole tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changeset {
    pub parents: Vec<String>,
    pub changes: Changes,
    pub meta: Meta,
}

impl Changeset {
    pub fn id(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("changeset serializes");
        hex::encode(Sha256::digest(&bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    Open,
    Landed,
    Abandoned,
}

impl ChangeState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeState::Open => "open",
            ChangeState::Landed => "landed",
            ChangeState::Abandoned => "abandoned",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Both sides edited the same lines.
    Content,
    /// Both sides added the file with different content.
    AddAdd,
    /// One side deleted the file, the other edited it.
    ModifyDelete,
    /// Both sides changed the mode differently.
    Mode,
    /// Both sides changed a binary file, symlink or submodule.
    Binary,
}

/// A main changeset that touched a path, for the conflict task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Touch {
    pub changeset: String,
    pub change: Option<String>,
    pub agent: String,
    pub intent: String,
}

/// A first-class conflict on one path. The change keeps living with it; landing waits until
/// the owner (or a resolver agent) writes the path again or calls `resolve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub kind: ConflictKind,
    pub base: Option<Entry>,
    pub ours: Option<Entry>,
    pub theirs: Option<Entry>,
    pub theirs_by: Vec<Touch>,
}

/// A mutable pointer with a stable id, owned by one agent. Replaces branches: it points at
/// the latest changeset (`head`) on top of `base`, and survives amend and rebase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub state: ChangeState,
    pub agent: String,
    pub model: Option<String>,
    pub session: Option<String>,
    pub intent: String,
    pub base: String,
    /// Equal to `base` while the change is empty; otherwise a changeset whose only parent is `base`.
    pub head: String,
    pub predecessors: Vec<String>,
    pub conflicts: Vec<Conflict>,
    pub landed_as: Option<String>,
    pub created: i64,
    /// Paths (globs) and symbols the owner said it will touch (`open_change` or `declare`).
    #[serde(default, skip_serializing_if = "Intent::is_empty")]
    pub declared: Intent,
    /// The head the owner's writes are relative to, set when the server rebased the change on
    /// its own (continuous rebase). Whole-file writes are then merged onto the new head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
}

/// What an agent declares before it edits: path globs and symbol names (`name` or `path#name`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
}

impl Intent {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.symbols.is_empty()
    }
}
