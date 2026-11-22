# Changelog

All notable changes. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/) (before 1.0, a minor version may break).

## [Unreleased]

### Fixed
- A bridge killed in the middle of a land (also by `SIGKILL`) no longer leaves its land test running:
  the test runs under a shell guard that kills its process group when the bridge's pipe closes.

## [0.2.0] - 2026-11-22

Everything since 0.1.3: `treeyard connect` and the agent tool set, the bridge's `checkout`, the
`undoes` hold, native mode (now the default of `connect`), the land test, and the T2 kit with the
development sets. Breaking: `treeyard connect` without a mode flag now sets up native mode; pass
`--api` for the previous setup.

### Changed
- `treeyard connect` sets up native mode by default (`treeyard mcp --tools native` and the native
  block). `--api` gives the previous API mode (`--tools agent`, the API block, byte for byte as
  before); `--native` is accepted and changes nothing; `--all-tools` means API mode and is refused
  with `--native`. `connect status` prints `mode: native` or `mode: api` and the land test.

### Added
- The land test. `[coordination] land_test` (and `land_test_timeout_s`, default 600) in the server's
  `treeyard.toml`, or `treeyard connect --land-test CMD` (`--no-land-test` removes it). The stdio
  bridge's `land` (native and API sets) rebases the change onto main, writes its directory, runs the
  command there, and lands with a proof (`tested`: command, main, head) that the server checks under
  its lock with the land: status `untested` without a proof, `main_moved` when main moved (the bridge
  tests again, at most 3 rounds). A failed test answers `tests_failed` with the end of the output
  (4 KB), not landed. The landed change records `tested`. No agent override; the owner turns it off in
  config. bench/t2: `run --land-test` for arm BN (run name `-gate`), with `land_tests_failed`,
  `land_test_rounds`, `land_test_seconds` in the run file. Tests: `tests/native.rs`, `src/gate.rs`,
  `bench/t2/test_t2.py`.
- Native mode. `treeyard mcp --tools native` is a third tool set for agents that read, edit and test
  with their own tools: `start {intent, paths?}` opens the change and writes a full checkout to a
  directory of its own (`dir`); `sync {change, rebase?}` sends what changed there (modified, added,
  deleted files; `.gitignore` rules and common caches honoured) through `apply`, so the reply carries
  the same `collisions` and `undoes`; `land` syncs, lands with the usual hold on `undoes`, and on a
  conflict writes the rebased files with conflict markers into `dir` (refused while a conflicted file
  still holds markers); `change` and `diff`. The manifest of the checkout lives beside it, not in it.
  `treeyard connect --native` writes these entries and a short native instruction block (the normal
  block is unchanged). Only the stdio bridge has the set (`POST /mcp?tools=native` is refused).
  Tests: `tests/native.rs`, `tests/connect.rs`.
- Native mode, after an audit of the first real runs: checkout directories are 0700 and must be the
  user's own (no symlink); delete, mode and binary conflicts wait for `resolve: {path: mine | main}`
  instead of being taken as they stand; a directory older than its change never sends a conflicted
  file, and the server checks markers after the merge of a stale write; backup files (`*-e` next to
  a file, `*.orig`, `*.rej`, `*~`, `*.bak`, `*.swp`) are not sent, and replies list `added` and
  `skipped`; edits after a land go into a follow-up change on the same directory; one override rule
  in the native block, schema and hold advice; a same-size edit within 2 s of the manifest write is
  hashed.
- bench/t2: run files record the binary's sha256 and version; arm BN archives start directories;
  `health` checks every state of main of a finished run.
- bench/t2: arm `BN`, arm B connected in native mode (`block-bn.md`), with the scripted harnesses
  driving the native flow through the bridge and the runner landing through it. Its metrics read the
  agents' own `land` replies (holds as warnings, reactions, overrides, conflicts, refusals), and
  `recount --metrics` counts them again for finished runs.

### Fixed
- `grep` with `path` naming a file returned no matches: `path` was taken as a directory prefix only
  (`path/`). It now limits the search to that file or to that directory. A `path` that names neither
  gets an empty `matches` and a `note` that says so. In the T2 click runs 10 of 24 `grep` calls of
  arm B named a file and came back empty.
- A failed `apply` needed several more calls. A `replace` whose old text is missing now reports every
  edit of the call that missed (`edit i of n`), each with the closest text of the file in the tree the
  edit was applied to (best match of the old text by lines, 3 lines of context, `lines a-b`; 4 KB per
  call in all), so one retry can fix all of them; it is said to `read` only when no line resembles the
  old text. An `apply` with `intent` that fails no longer points at the change it opened and
  abandoned; the text it shows is main's. A miss of text that main has while the change is behind
  still says to `rebase`, then apply again (behaviour change: the error text).
- `land` held only on the first 20 `undoes` entries: an entry of another agent's landed lines past
  20 entries that do not hold (lines of the change it is stacked on) now holds too; a held reply lists
  at most 20 and counts the rest in `undoes_more`.
- Queue mode: a second `land` without `undo` of a change already queued with a yes for its current
  head answered `undoes`; it now answers `queued`.
- `grep`: `^` and `$` are line anchors in the indexed and the scan path. A whole-file prefilter ran
  the pattern without multi-line mode, so `^x` matched only on the first line and `x$` only on the
  last; it now runs in multi-line mode (also before `\r\n`), and is skipped for `\A` / `\z`.
- bench/t2: the `guard` metrics (and `lost_lines`) of arm A2 were 0 for every agent that had
  integrated through `t2-integrate` (its diff against main was empty by then). An agent's own diff is
  now what each of its integrations and the runner's merge put on main, in arms A and A2 alike (arm B:
  its changes' diffs); work left out counts from the merge base.

### Changed
- `read` takes `ranges` (several `[start, end]` ranges of one file, reply `ranges`) and `files` (up to
  20 files in one call, each with its own `start`, `end` or `ranges`; reply `files`, a failing file
  gives `{path, error}` and does not fail the others; files without a range share `max_bytes`, and
  the MCP 50 KiB default). In the T2 click runs 16 of 33 `read` calls of arm B followed another
  `read`. The bridge's `checkout` tool says that an `apply` reply with `checkout.dir` has written it.
- Fewer calls per task. `apply` without `change` and with `intent` (and `paths`, ...) opens the change
  and applies in one call; the reply carries the id, overlaps and advice as `open_change` would (a
  failed edit abandons that change). Through the stdio bridge `apply` takes `checkout` and writes the
  change's checkout directory in the same call (`checkout.dir`); the default is true with `--tools
  agent` (the first checkout of this repository, 1 235 files, took about 1 s in a debug build). The
  checkout reply gives `main_moved: true` and the rebase advice only when the landings since the
  change's base changed a file it edits (`main_moved_in`); landings in other files give
  `main_moved_elsewhere: true` and no advice (behaviour change). New `behind_main` in the `change`
  reply. The advice no longer says to call `checkout` again after `rebase` (the bridge rewrites the
  directory). The agent view drops `change`, `state`, `head`, `files`, `seconds` from checkouts. The
  `apply` and `land` descriptions of the agent set state the flow (apply with intent, tests in
  `checkout.dir`, land). The minimal flow is 2 calls instead of 4 (6 when main had moved); agent
  `tools/list` 6 339 to 6 871 bytes.
- `undoes` judges a removed line by what the other agent contributed to it, not by the whole line
  (behaviour change). The contribution comes from that agent's own diff of the path: the words of the
  line that the lines its hunk replaced did not have (every word for a line it added whole). The line
  is quiet while those words stay on the rewritten line, so an agent that rewrites another agent's
  line for its own task keeps both (`.strip().casefold()` to `.strip().lower()` is quiet; to
  `.casefold()` it warns). Earlier changes of a line are followed back through the lines each hunk
  replaced (3 steps). Entries carry `contributed` (`line`, `words`, the other agent's `before` and
  `after`); the advice says to keep that part and still make one's own edit. In two real 5-agent runs
  the old rule reported such a line and the agent reverted its own task. The land hold follows the
  same rule.
- `land` holds a change that removes lines another agent landed (behaviour change): status `undoes`
  with the entries (text included) and `advice` (put them back and land again; if both tasks cannot
  hold, keep the other agent's lines, land the rest and say so; only if the task names the lines,
  land with the override). Override: `land {change, undo: true, reason}` (a non-empty `reason` is
  required). An overridden land is recorded as `undid` (who removed whose lines, which changes, why) in
  the `land` reply and the change, the event `undo_override`, `status` (`undid`, `undid_total`), `log`
  entries and the audit row; a held land publishes the event `undoes`. Not held: own lines, moved or
  re-indented lines, lines of a change it is or was stacked on (`stacked_on` in the change), lines a
  revert put back; revert, `git push` and `replay` never hold. Queue mode holds at enqueue and drops a
  queued change before a batch when its head moved after the yes (review `undoes`, `land_status`
  `undoes`). `[coordination] hold_undoes = false` turns it off. The agent tool set's `land` takes
  `undo` and `reason`; `treeyard workspace land --undo REASON`. In a 5-agent run one agent removed
  another's landed feature (8 lines) and rewrote its test although `undoes` was in two of its replies.
  With no entries the land costs no more than before (`undoes` was computed after the land; now once
  before it, reusing the change's scans when there is no rebase).
- Leaner MCP replies. The bridge and `POST /mcp` send compact JSON. With `--tools agent`, change
  replies drop `base`, `head`, `created`, `group`, `landed_as`, `model`, `session`, `view`, `revisions`
  and every null or empty value; `apply` sends `collisions` / `overlaps` in full only when they changed
  since the last reply of the change, else one line (`collisions_unchanged`, `overlaps_unchanged`);
  `grep` drops `at` and `index`. A `read` through MCP without a range gets `max_bytes` 51 200: bigger
  files return their first lines with `truncated` and a `note` (new `read` parameter `max_bytes` for
  every client). New `grep` parameter `context` (`before`, `after` per match). Bytes with
  `--tools agent`: empty `open_change` 565 to 141, `apply` with a repeated collision 1 480 to 465,
  `grep` 398 to 141, `change` with a conflict 261 935 to 1 594 (docs/mcp.md).
- `undoes` is precise (shape change): a removed line counts only when its text (without whitespace) is
  gone from all paths the change edits and no added line of the same hunk keeps all its words, so moves,
  re-indents, renames, CRLF to LF and the no-final-newline case are quiet, and so are rewrites like
  `value.lower()` to `value.lower().strip()`. Entries carry `text` and `change`; the advice is one
  `undoes_advice` per reply, `undoes_more` counts entries past 20. `apply` sends only entries not sent
  before (`undoes_earlier` counts the others); `resolve`, `change` and `land` (`landed`, `queued`) now
  carry `undoes` too; lines of an open change this one is stacked on count (`landed: false`); the same
  `session` under another agent name is own work; entries obey the token's read globs. Per-path scans
  are kept per change: an `apply` to another file after one that removed a line of a 60 000-line file
  went from about 60 ms to 4 to 5 ms (debug build).
- Conflicts come as regions (shape change): every conflict in a change reply (`change`, `apply`,
  `resolve`, `rebase`, `declare`, `land`, `restore_change`) lists `regions` (marker blocks of the
  change's text with line numbers and 3 lines of context, ready for `replace`) and the blob, mode and
  `size` of `base`/`ours`/`theirs`, instead of `text` with the three whole files (up to 256 KiB each).
  `change {full: true}` returns `text` as before. The per-conflict `resolve` hint is now one `resolve`
  string per change; `conflicts_cut` says when regions were capped. A `land` conflict on an 87 KB file
  went from 262 246 to 2 718 bytes in the MCP bridge (`change`: 261 935 to 2 391).
- The connect instruction block, step 6: test with `checkout` (run the tests in the directory it
  returns; later `apply` calls update it; edits there do not count) instead of copying files into a
  temporary directory by hand. The rest of the block is unchanged.
- `read`: a range from line 1 of an empty file returns empty `content` with `lines` `[1, 0]` instead of
  an error. The docs and the tool description say that `size` is the byte size of the whole file, also
  for a range.
- `connect` writes `default_tools_approval_mode = "approve"` into its own Codex server entry
  (`.codex/config.toml`; key as codex-cli 0.167.0 reads it): `codex exec` refused every TreeYard tool
  call without it. `disconnect` removes it with the block (byte-identical file), and `connect status`
  shows it (`tools approval approve`, JSON `tools_approval`).
- `apply`: a `replace` whose old text (or file) the change lacks now says whether main has it and how
  many landings the change is behind main, and what to do (`rebase`, or `read` with `at` = the change).
  In the pilots, 5 such errors had no hint. `connect` keeps `continuous_rebase` off and `read` keeps
  main as its default (reasons in docs/coordination.md).
- A resolution of a conflicted path is checked for conflict markers once, after all edits of the
  `apply`, so several `replace` edits in one call can remove them; before, the first edit was refused.
  The refusal now says that one `apply` must remove every marker of the file, and how many regions
  are left (6 refusals in the pilots).
- The connect instruction block has two new points: read a line range in few calls, and keep the
  lines other agents landed (put them back when `apply` returns `undoes`). On a development set of 10
  tasks, the old block lost a task in 1 of 2 colliding runs although `apply` returned `undoes`; the new
  rule lost none in 3 runs (`bench/t2/dev/TUNING.md`).
- The connect instruction block tells the agent to test its change on a copy before `land`, and that
  the files on disk do not change at land (`read` shows landed work). In the T1 trial with the old
  block, all 3 agents looked for the landed work in the checkout, and 2 of 3 ran the tests only after
  land. With the new block, 3 of 3 tested on a copy before land, and 1 of 3 still looked in the checkout.
- `open_change`: the `agent` field says that the MCP bridge sets the name and that a value replaces it.
- Line merges run in process (`src/xmerge.rs`, a port of git's histogram diff and `xdl_merge`) instead of
  one `git merge-file` process per merge. Same text and the same clean or conflict answer as
  `git merge-file --diff-algorithm histogram` (`tests/xmerge_git.rs` compares them); `git merge-file` stays
  as the fallback. On git before 2.44 merges now match git 2.44+ too.
- Export: a land reads its parent's commit and trees from memory, and new objects skip the `cat-file`
  existence check (a loose copy of a packed object is valid git). Loose objects use zlib level 1.
- Landing load test (50 agents x 5 changes, 20 % conflicting): 0.46 s instead of 1.1 s, p99 0.11 s
  instead of 0.29 s (docs/benchmarks.md).

### Added
- Tests: `tests/mcp_agent.rs` drives the built bridge with `--tools agent` (tool list, lean change
  replies, a collision sent once, the read cap, grep context, conflict regions, `undoes` after
  `resolve`) and `--tools all`; `tests/undoes.rs` covers the 20-entry cut (`undoes_more`) and the
  read-glob filter of `undoes`.
- `treeyard mcp --tools agent|all` and `POST /mcp?tools=agent`: the agent tool set lists 12 operations
  (`read`, `grep`, `symbols`, `outline`, `list`, `open_change`, `apply`, `change`, `diff`, `rebase`,
  `resolve`, `land`) plus `checkout`, with short descriptions: `tools/list` 5 948 bytes instead of
  22 378. Every operation still answers `tools/call`. `treeyard connect` writes `--tools agent` by
  default; `connect --all-tools` writes the full list (state field `all_tools`).
- `checkout` (a tool of the stdio bridge `treeyard mcp`, and `treeyard checkout CHANGE [--into DIR]`):
  writes the tree of a change into a directory of its own under the temp directory, so an agent runs
  the repository's real tests on exactly what it will land, with no retyping. The bridge rewrites the
  directory after each `apply`, `resolve`, `rebase`, `restore_change` and `land` of the change (only the
  files that differ); local edits there are put back (`reverted`), untracked files are listed (`extra`),
  and `main_moved` says when work landed after the change's base. In the last pilot 0 of 15 TreeYard
  agents ran the tests to green (they retyped files into temp directories), against 9 of 15 with git
  worktrees.
- `read` takes optional `start` and `end` (1-based, inclusive line numbers) and then returns only those
  lines, with `lines` (the range returned, clamped to the file) and `total_lines`; a whole-file read
  of a text file now has `total_lines` too. A range on a binary file or a submodule is an error. In the
  T2 pilot, 90 % of the bytes MCP tools returned came from whole-file `read` calls (11 to 19 KB each).
- `apply` returns `undoes`: lines the change deletes or rewrites that a different agent landed (by
  blame at the change's base), with `path`, `lines`, `agent`, `intent`, `changeset` and `advice`. In
  the T2 pilot one agent removed lines another agent had landed minutes before, with no conflict to warn it.
- bench/t2: the kit for the T2 experiment (plain git worktrees against `treeyard connect`, 5 agents at
  once): 30 tasks on pinned requests, ripgrep and astro, each with a hidden deciding test and a reference
  patch; a runner with per-agent turn, token and time limits and anomaly stops; a metrics collector;
  oracle and noop rehearsals; bench/t2/PREREGISTRATION.md. No real-agent run yet.
- `treeyard connect`, `treeyard connect status` and `treeyard disconnect`: one command attaches a git
  repository to Claude Code, Codex and OpenCode (project-scoped MCP entries, an instruction block in
  AGENTS.md and CLAUDE.md, a loopback server), and one command restores every touched file byte for
  byte (docs/connect.md). `serve --port-file`, `mcp --agent-pool --pool-dir`.
- `treeyard workspace-hook create|remove`: the command an orchestrator runs when it makes and removes
  an agent's workspace (arguments and environment only, the path on stdout, safe to repeat, exit 3 when
  the server does not answer). docs/orchestrators.md: the hook points of Orca, Paradev and Agent
  Orchestrator, read from their source.
- docs/trials/t1/: a smoke trial on real agents (Claude Code, Codex, OpenCode). Each one got a small
  task that does not name TreeYard and went `open_change` -> `apply` -> `land` from the instruction
  block alone (server audit log, test green on main). Driver: `docs/trials/t1/run.py`.
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

[Unreleased]: https://github.com/exit-0-run/treeyard/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/exit-0-run/treeyard/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/exit-0-run/treeyard/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/exit-0-run/treeyard/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/exit-0-run/treeyard/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/exit-0-run/treeyard/releases/tag/v0.1.0
