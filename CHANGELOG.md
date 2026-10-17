# Changelog

All notable changes. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) (before 1.0, a minor version may break).

## [Unreleased]

Product name: TreeYard. The binary, the crate and the commands are still `rootstock` in this
version; the rename comes in a later version.

## [0.1.0] - not released yet

The first version that is complete end to end on one node.

### Server core
- Changes with stable ids instead of branches; content-addressed changesets; SQLite metadata.
- Server-side 3-way merge, rebase and land without a global git lock; every clean merge checked
  against `git merge-tree` in the tests (`tests/swarm.rs`, `tests/fuzz_history.rs`).
- Conflicts as typed tasks (`content`, `add_add`, `modify_delete`, `mode`, `binary`) with base,
  ours, theirs and the agents that changed the path.
- Stacked changes (`waiting`), `already_on_main`, `revert`, `export_change`.
- One operation table for HTTP (`POST /api/<op>`), MCP (stdio) and the CLI (`rootstock call`).
- `rootstock replay`: real git history replayed by waves of N agents, checked against git.

### Git for people
- Git smart HTTP under `/git`: clone, fetch, and fast-forward push to main (each pushed commit lands
  as a change). `rootstock mirror` pushes main to a read-only remote.

### Workspaces
- `rootstock workspace` and `rootstock daemon`: copy-on-write directories (APFS `clonefile`, reflink,
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
  from the operation table (`rootstock api-doc`), MCP setup for Claude Code, Codex and OpenCode,
  comparison, open RPC spec v0 (`docs/spec/rpc.md`), install.
- `examples/swarm-demo.sh`: N scripted agents against git worktrees and against the server.
- `site/index.html`: a static page with the measured numbers.
