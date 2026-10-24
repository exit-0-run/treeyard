# Changelog

All notable changes. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) (before 1.0, a minor version may break).

## [Unreleased]

### Changed
- Line merges run in process (`src/xmerge.rs`, a port of git's histogram diff and `xdl_merge`) instead of
  one `git merge-file` process per merge. Same text and the same clean or conflict answer as
  `git merge-file --diff-algorithm histogram` (`tests/xmerge_git.rs` compares them); `git merge-file` stays
  as the fallback. On git before 2.44 merges now match git 2.44+ too.
- Export: a land reads its parent's commit and trees from memory, and new objects skip the `cat-file`
  existence check (a loose copy of a packed object is valid git). Loose objects use zlib level 1.

### Added
- `bench/scion/`: the landing load test against CarterT27/scion (`sh bench/scion/run.sh`), results in
  docs/benchmarks.md.

## [0.1.3] - 2026-10-21

### Added
- `POST /mcp`: MCP over Streamable HTTP on the server itself, stateless, same tools and token rules as
  `/api`. No or unknown token is `401` with a Bearer challenge, `initialize` included (docs/mcp.md).
- `Authorization` also accepts a bare token, as an MCP gateway sends a member's key verbatim.

### Fixed
- `apply` refuses a resolution of a conflicted path that still holds the server's own conflict
  markers (`<<<<<<< main`, `>>>>>>> change`). In `direct` mode, `land` used to put them on main.
  Behaviour change: an agent that writes the markers back now gets an error from apply, not a land.

## [0.1.2] - 2026-10-19

Found by runs with 50 real Claude Code agents (docs/benchmarks.md, "50 agents, wide set").

### Fixed
- Optimistic landing: a culprit whose revert conflicts no longer keeps every later verdict `pending`. It
  stays on main as `red`, its owner gets `broke_main` and fixes forward, later changes get `main_red` at
  once, and a red main costs one test per step.

### Changed
- Queue mode tests a batch first. When the batch fails, it tests each change alone on main, side by side
  (`landing.parallel`, default half the cores, at most 4), and sends the failing ones back, so one broken
  change no longer holds up its batch.
- Landing tests run in one kept directory per parallel slot (`landing.warm`, default on). Only changed files
  are written, so `target/` and `node_modules` stay warm between test runs.

### Added
- A `test_run` event for every landing test, with its seconds.
- `bench/landbench.sh` and `src/bin/landbench.rs`: a landing load test against plain git (50 agents x 5
  changes, 20 % conflicting: 197 of 250 landed in 1.1 s; plain git landed 5 to 93).
- Live-agent runner: land and go (`DEMO_LAND_AND_GO=1`), queue mode, a 50-task set with 60 hidden checks,
  ripgrep support, a Claude Code cloud-session runner.

## [0.1.1] - 2026-10-18

### Fixed
- Lands that need a line merge work with git older than 2.44 (Ubuntu 24.04 ships 2.43). Before, they failed
  with `unknown option 'diff-algorithm'`; now `git merge-file` falls back to its default diff and the
  server says so once.

### Added
- Stacking: when a landing-order conflict is warned, `advice.stack_on` names the change to build on;
  `rebase {change, onto}` and `treeyard workspace stack NAME --on CHANGE` move the conflict from land time
  to now, and a stacked change waits to land until the one below it lands.
- Collisions: a conflict with main is reported once as `"change": "main"`; workspaces keep the server's
  collisions in `.treeyard/collisions.json`, and `treeyard workspace status` asks again.
- Live-agent demo: `DEMO_N`, `DEMO_POOL` and `examples/live-agents/cloud.sh` for a Claude Code cloud session.

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

[Unreleased]: https://github.com/exit-0-run/treeyard/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/exit-0-run/treeyard/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/exit-0-run/treeyard/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/exit-0-run/treeyard/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/exit-0-run/treeyard/releases/tag/v0.1.0
