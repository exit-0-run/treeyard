# Changelog

All notable changes. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) (before 1.0, a minor version may break).

## [Unreleased]

## [0.1.0] - 2026-10-18

The first version that is complete end to end on one node.

### After the first live runs with real agents
- `treeyard workspace update` writes the change as the server holds it; `workspace land` does it on its
  own after a land that hits a conflict.
- `land_status` and `workspace land --wait-s`: the verdict of queue or optimistic tests (passed,
  rejected, reverted with the failing output); a closed change is followed by a new change from the
  workspace base, merged 3-way onto main.
- Collisions trial-land both orders on today's main and give a `reason`; clean overlaps far apart are
  not reported.
- Safe `workspace remove`, repo config `.treeyard/workspace.toml`, workspace states and logs,
  `workspace gc`, polite git probes, `workspace mirror`, sparse `--paths`, `--from-dir`.
- Every git child runs without automatic maintenance (detached repacks once used all memory).

### Shared directories, checkpoints, conflicts that wait, notes, best-of-N
- Shared directories: `assign`, `claim`, `apply_assigned` and `treeyard share join|sync|claim`. Agents
  that edit one checkout get one change each; every hunk has a stable id and one owner (claim, intent,
  earlier owner), and a hunk that builds on another change's lines is held as `depends_on`.
- Checkpoints inside an open change: `checkpoint`, `checkpoints`, `restore_change` (undo that keeps
  history), `treeyard workspace restore NAME --to N`, and a Claude Code `UserPromptSubmit` hook that
  marks one checkpoint per turn (`examples/claude-code/checkpoint.sh`).
- `[coordination] keep_conflicted = true`: a conflicted change keeps rebasing and taking edits;
  `land` waits until its conflicts are resolved. Default off (the old rule).
- Agent transcripts per change: `attach` (transcript, summary or note; lines that look like secrets
  are dropped) and `attachments`; on land they become a git note under `refs/notes/treeyard`.
- Best-of-N: `open_change` takes `group`; `group` compares the members (state, diff size) and
  `land_one_of` lands one and abandons the other open changes of the group.

### Server core
- Changes with stable ids instead of branches; content-addressed changesets; SQLite metadata.
- Server-side 3-way merge, rebase and land without a global git lock; every clean merge checked
  against `git merge-tree` in the tests (`tests/swarm.rs`, `tests/fuzz_history.rs`).
- Conflicts as typed tasks (`content`, `add_add`, `modify_delete`, `mode`, `binary`) with base,
  ours, theirs and the agents that changed the path.
- Stacked changes (`waiting`), `already_on_main`, `revert`, `export_change`.
- One operation table for HTTP (`POST /api/<op>`), MCP (stdio) and the CLI (`treeyard call`).
- `treeyard replay`: real git history replayed by waves of N agents, checked against git.

### Git for people
- Git smart HTTP under `/git`: clone, fetch, and fast-forward push to main (each pushed commit lands
  as a change). `treeyard mirror` pushes main to a read-only remote.

### Workspaces
- `treeyard workspace` and `treeyard daemon`: copy-on-write directories (APFS `clonefile`, reflink,
  overlayfs, hardlink, copy), dependency layers keyed by lockfile hash, ports and env per workspace,
  a warm pool, continuous sync to the change.
- Claude Code `WorktreeCreate` / `WorktreeRemove` hook scripts (`examples/claude-code/`).

### Coordination
- Intents (paths, symbols), live collisions by trial merge, events (poll, long poll, SSE),
  continuous rebase, merge drivers (union, Cargo.lock, regenerate, mergiraf), landing modes
  `queue` and `optimistic`, semantic guard with review tasks.

### Derived data, access
- `blame` equal to `git blame --first-parent`, `log` with a changed-path index, trigram `grep`,
  tree-sitter `symbols` and `outline`, server-side `bisect`.
- API tokens with read and write path globs, admin tokens, change ownership, audit log.

### Hardening
- Crash recovery at 6 failpoints, land races, retried replies (`repeated`), `503` back-pressure,
  data-dir lock, hostile paths and sizes; random histories compared with git. Threat model:
  `SECURITY.md`.

### Packaging and docs
- `scripts/dist.sh` (release archives for macOS arm64 and static Linux x86_64 / arm64, with
  `SHA256SUMS`), `scripts/install.sh` (checksum-checked install, no root).
- `docs/`: quickstart (checked by `scripts/check-quickstart.sh`), concepts, API reference generated
  from the operation table (`treeyard api-doc`), MCP setup for Claude Code, Codex and OpenCode,
  comparison, open RPC spec v0 (`docs/spec/rpc.md`), install.
- `examples/swarm-demo.sh`: N scripted agents against git worktrees and against the server.
- `site/index.html`: a static page with the measured numbers.
