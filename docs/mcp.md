# Connect agents: Claude Code, Codex, OpenCode

Every agent reaches the server in one of three ways. All three use the one operation table
([api.md](api.md)).

One command does the MCP setup below for one repository, in project scope, and undoes it again:
`treeyard connect` and `treeyard disconnect` ([connect.md](connect.md)).

| Way | When | How |
|---|---|---|
| MCP tools | the agent edits through the API, no checkout | `treeyard mcp --url URL --agent NAME` (stdio), or `URL/mcp` (HTTP) |
| A workspace directory | the agent must build or run tests | `treeyard workspace create NAME` + `treeyard daemon` |
| CLI | scripts and orchestrators | `treeyard call <op> '<json>'` |

`treeyard mcp` reads `TREEYARD_URL` (default `http://127.0.0.1:7420`) and `TREEYARD_TOKEN`. With a
token, the server names the agent from the token. Without one, `--agent` (or `TREEYARD_AGENT`)
fills `agent` in `open_change` and `revert`. The server sends short instructions in its MCP
`initialize` reply: read with `read`/`list`/`grep`, then `open_change`, `apply`, `land`, and how to
handle a conflict.

The command lines below follow each tool's own docs, opened on 2026-10-17:
[Claude Code MCP](https://code.claude.com/docs/en/mcp), [Claude Code hooks](https://code.claude.com/docs/en/hooks),
[Codex MCP](https://developers.openai.com/codex/mcp), [OpenCode MCP servers](https://opencode.ai/docs/mcp-servers/).
The MCP bridge itself is tested with raw JSON-RPC on stdio (`tests/git_bridge.rs`, and step 7 of
[quickstart.md](quickstart.md)). No agent ran in these tests: the harness that built this has no
budget for model calls.

## Tool sets

`tools/list` costs input tokens on every turn of some clients (Codex sends the list with each turn:
30.5 k input tokens per turn with TreeYard against 20.9 k with plain git, in the pilots). Agents used
18 of the 46 tools. `treeyard mcp --tools agent` lists only the agent tool set, with short descriptions
and only the parameters agents use:

`read`, `grep`, `symbols`, `outline`, `list`, `open_change`, `apply`, `change`, `diff`, `rebase`,
`resolve`, `land`, and the bridge's own `checkout`.

These are the tools the connect instruction block names (find, read, edit, inspect the other change,
test, land, settle a conflict), plus `list`, `diff` and `rebase`, which an agent needs to look around,
review its own change and take main's work. Left out: history and server-side jobs (`log`, `blame`,
`bisect`, `revert`), coordination plumbing (`events`, `declare`, `reviews`, `queue`, `land_status`),
best-of-N, checkpoints, attachments, shares, tokens, `audit`, `tree`, `blobs`. A `tools/call` of any of
them still works: the set changes only what is listed. `--tools all` (the default of `treeyard mcp`)
lists every operation. `treeyard connect` writes `--tools agent` unless `--all-tools` (or `--native`, below). The HTTP endpoint
takes the same choice as a query: `POST /mcp?tools=agent` (no `checkout` there).

`--tools native` is a third set, for agents that read, edit and test with their own tools in a
directory of their own: `start`, `sync`, `land`, `change`, `diff`
([Native mode](#native-mode-start-edit-with-your-own-tools-sync-land)). Only the stdio bridge has it.

Measured `tools/list` result, compact JSON: 21 753 bytes before (46 tools), 22 378 bytes with
`--tools all` now, 5 948 bytes with `--tools agent` (13 tools). With the `undo` parameters of `land`
it was 6 339 bytes; the one-call flow below (`apply` with `intent`, the bridge's `checkout`
parameter, the flow in the `apply` and `land` descriptions) makes it 6 871.

### The minimal flow: apply, test, land

Every tool call is a model turn that sends the whole context again, so the fixed calls per task
are the cost. On five small tasks that did not collide, 5 agents with plain git worktrees made 34
calls; with TreeYard 71 (`grep` 17, `read` 9, `checkout` 8, `apply` 7, `open_change` 5, `land` 5,
`rebase` 4, ToolSearch 4, `diff` 2, shell 9). The fixed part of a task is now two calls:

1. `apply` with `intent` (and `paths`) and no `change` opens the change and applies the edits in one
   call. The reply is the change (its `id`), with `overlaps` and `advice` as `open_change` gives
   them, plus `collisions` and `undoes`. If an edit fails, the change opened for it is abandoned
   and the error says so. `change` and `intent` together, or neither, is an error. `open_change`
   stays.
2. Through the stdio bridge with `--tools agent`, that first `apply` also writes the change into its
   checkout directory (`checkout.dir`; `apply {checkout: false}` skips it; with `--tools all` it is
   off unless `checkout: true`). The agent runs its tests there; later calls keep it current.
3. `land` (it rebases on the server).

Before: `open_change`, `apply`, `checkout`, `land` (4), plus `rebase` and `checkout` again whenever
main had moved (6). After: `apply`, `land` (2). Test: `tests/mcp_agent.rs`
(`the_minimal_flow_is_apply_then_land`, a scripted session over the stdio bridge).

The first checkout of a change writes the whole tree once: on a clone of this repository (1 235
files, 9.6 MB) 1.0 to 1.2 s with a debug build, against 0.06 s for a rewrite that writes nothing
(3 runs each, one machine). That is less than one model turn, and every task that runs its tests
needed it anyway, so the agent set writes it by default.

Why agents rebased and checked out again on disjoint tasks: the checkout reply said `main_moved:
true` with the advice "rebase, then checkout again" whenever anything had landed, although `land`
rebases anyway and the bridge already rewrites the directory after `rebase`. Now `main_moved` is
true only when the landings since the change's base changed a file the change edits
(`main_moved_in` lists them, with the advice to rebase); otherwise `main_moved_elsewhere: true`, no
advice. The server's `change` reply carries `behind_main: {"paths": [...]}` for that. In the agent
view a checkout drops `change`, `state`, `head`, `files` and `seconds`, and `main_moved: false`. Tests: `src/mcp.rs` (the set and its
size), `tests/mcp_http.rs` (`tools_agent_lists_the_agent_tool_set`), `tests/connect.rs` (the args).

### The agent view of replies

With the agent set, replies are shaped for a model to read (`src/lean.rs`); `--tools all` returns the
server's replies as they are. Both send compact JSON (pretty printing was 21 to 29 % of a reply).

- Change replies (`open_change`, `apply`, `change`, `rebase`, `resolve`, `declare`, `abandon`, the
  change in `land` and `restore_change`) drop `base`, `head`, `created`, `group`, `landed_as`, `model`,
  `session`, `view` and `revisions` (no agent referenced them in 110 recorded sessions) and every null,
  `[]` and `{}`. An empty change is `{"agent", "conflicted", "id", "intent", "state"}` plus what it
  declared.
- `apply`: `collisions` and `overlaps` come in full when they are new or changed since the last
  `apply` or `change` reply of that change; otherwise one line in `collisions_unchanged` /
  `overlaps_unchanged` (count, kinds, change ids). `change` always lists them in full. The bridge keeps
  this memory per process; the HTTP endpoint keeps one per server.
- `grep` drops `at` and the `index` statistics.

For every MCP client (both sets), a `read` without `start`, `end`, `ranges` or `max_bytes` gets `max_bytes`
51 200: a larger file returns its first whole lines with `truncated: true`, `lines`, `total_lines`
and a `note` that says how to read on (one whole-file read of a 60 000-line file returned 938 KB).
`grep` takes `context` for lines around each match. `read` takes `ranges` (several ranges of one
file) and `files` (several files) in one call; the 51 200 bytes are then shared by the files without a
range.

Reply bytes through the stdio bridge, on a clone of this repository (an 87 KB `src/repo.rs`, plus
a 60 000-line file; two agents change the same line; measured with a script outside the repository):

| Call | before | `--tools all` | `--tools agent` |
|---|---:|---:|---:|
| `open_change` (empty change) | 565 | 461 | 141 |
| first `apply` | 604 | 488 | 165 |
| `apply` with a new collision | 1 460 | 903 | 610 |
| `apply` with the same collision again | 1 480 | 918 | 465 |
| `grep` (1 match) | 398 | 305 | 141 |
| `grep` (2 matches) | 511 | 384 | 221 |
| `read` of the 60 000-line file | 2 618 012 | 55 666 | 55 666 |
| `land` with a conflict | 262 246 | 2 072 | 1 793 |
| `change` with that conflict | 261 935 | 1 915 | 1 594 |

"before" is the build before this series (pretty JSON, whole conflict texts). Tests: `src/lean.rs`,
`tests/read.rs` (`max_bytes`, `context`), `tests/mcp_agent.rs` (every shape above through the built
bridge with `--tools agent`, and `--tools all` unchanged).

## Claude Code

Add the server for one project (local scope) or for all projects (`--scope user`):

```text
claude mcp add --env TREEYARD_URL=http://127.0.0.1:7420 --transport stdio treeyard -- treeyard mcp --agent claude-1
claude mcp get treeyard
```

To share it with a team, put it in `.mcp.json` at the project root:

```json
{
  "mcpServers": {
    "treeyard": {
      "command": "treeyard",
      "args": ["mcp", "--agent", "claude"],
      "env": { "TREEYARD_URL": "http://127.0.0.1:7420" }
    }
  }
}
```

### Claude Code worktrees become TreeYard workspaces

Claude Code runs a `WorktreeCreate` hook instead of `git worktree add` for `claude --worktree`,
subagents with `isolation: "worktree"`, workflows and background sessions. The hook prints the
directory as its last stdout line. `examples/claude-code/` has both hooks:

- `worktree-create.sh` reads `name` from stdin, runs `treeyard workspace create NAME --git`
  (set `TREEYARD_SETUP="cargo build -q"` to build once), and prints the workspace path.
- `worktree-remove.sh` reads `worktree_path`, syncs the last edits to the server, then deletes the
  directory. The change stays open on the server, so a removed worktree loses nothing that synced.

Add them to `.claude/settings.json` (absolute paths; see `examples/claude-code/settings.json`), and
run `treeyard daemon &` on the machine so edits sync while the session works:

```json
{
  "hooks": {
    "WorktreeCreate": [{ "hooks": [{ "type": "command", "command": "sh /path/to/examples/claude-code/worktree-create.sh" }] }],
    "WorktreeRemove": [{ "hooks": [{ "type": "command", "command": "sh /path/to/examples/claude-code/worktree-remove.sh" }] }]
  }
}
```

Test: `tests/claude_hooks.rs` feeds both scripts the hook JSON from the Claude Code docs, edits a
file in the workspace, removes it, and reads the edit back from the open change.

Limits that the Claude Code docs state: the hook does not run when Claude types `git worktree add`
in Bash; `.worktreeinclude` is not processed (copy `.env` files in your setup command). When a
subagent finishes, Claude Code keeps the worktree and does not run the remove hook: land or remove
those workspaces yourself (`treeyard workspace list`).

### One checkpoint per turn

`examples/claude-code/checkpoint.sh` is a `UserPromptSubmit` hook. Before each prompt it finds the
workspace of the session (the `.treeyard/env` above `cwd`), syncs it, and calls `checkpoint` with the
label `turn of session <id>`. A turn without edits adds no checkpoint. It prints nothing on stdout (that
would become model context) and never blocks the prompt. Undo a turn with
`treeyard workspace restore NAME --to N`, or ask the agent to call `restore_change`; `checkpoints` lists
them with the paths each turn changed.

```json
{ "hooks": { "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "sh /path/to/examples/claude-code/checkpoint.sh" }] }] } }
```

Test: `tests/claude_hooks.rs` (two turns give two checkpoints; outside a workspace the hook does nothing).

## Codex

```text
codex mcp add treeyard --env TREEYARD_URL=http://127.0.0.1:7420 -- treeyard mcp --agent codex-1
codex mcp list
```

Or in `~/.codex/config.toml`:

```toml
[mcp_servers.treeyard]
command = "treeyard"
args = ["mcp", "--agent", "codex-1"]
default_tools_approval_mode = "approve"   # else `codex exec` refuses every tool call

[mcp_servers.treeyard.env]
TREEYARD_URL = "http://127.0.0.1:7420"
```

Codex has no worktree hook (openai/codex issue #26648 asks for a custom worktree backend). For a
directory, create a workspace first and start Codex in it:
`cd "$(treeyard workspace path fix-1)" && codex`.

## OpenCode

In `opencode.json` (project) or the global OpenCode config:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "treeyard": {
      "type": "local",
      "command": ["treeyard", "mcp", "--agent", "opencode-1"],
      "enabled": true,
      "environment": { "TREEYARD_URL": "http://127.0.0.1:7420" }
    }
  }
}
```

Check with `opencode mcp list`. For a directory, start OpenCode inside a workspace as for Codex.

## Test a change: `checkout`

An agent that edits through `apply` has no files on disk, so it cannot run the tests. The stdio
bridge `treeyard mcp` has one tool of its own for that, `checkout` (it is not in the server's
operation table: only the bridge runs on the agent's machine):

```text
checkout {"change": "ch_..."}   ->   {"dir": "/tmp/.../treeyard-checkouts-501/ch_...", "files": 120, "written": 120, ...}
```

- `dir` holds exactly the tree of the change as the server has it: its base plus its edits, exec
  bits and symlinks, without the files it deleted. The agent runs the repository's own tests there
  (`cd DIR && make test`). No file is retyped.
- The default directory is one per change under the system temp directory (`$TMPDIR`, else `/tmp`).
  Both Claude Code and Codex under `sandbox_mode = "workspace-write"` let shell commands write there,
  and nothing is written into the repository checkout or a workspace that another agent or the daemon
  syncs. `into` names another directory; it must be new, empty or an earlier checkout
  (`.treeyard/checkout.json` marks one), so a checkout never overwrites a working tree.
- After the first `checkout` (or an `apply` with `checkout`, the default of `--tools agent`), every
  `apply`, `resolve`, `rebase`, `restore_change` and `land` of that change through the same bridge
  rewrites the directory, and the reply has a `checkout` field. Only files that differ are written,
  so build output (`target/`, `__pycache__/`) and mtimes stay.
- Edits made in the directory never reach the change: the next rewrite puts the change's content back
  and lists the paths in `reverted`. Files that the change does not have stay; those that no
  `.gitignore` excludes are listed in `extra` (they would not land).
- `main_moved: true`: work landed after the change's base in files the change edits
  (`main_moved_in`), so the directory lacks it. `rebase` (it rewrites the directory) to test what
  `land` will put on main. Landings in other files only give `main_moved_elsewhere: true` and no
  advice: `land` rebases. After a `land`, the directory holds the landed tree.

`treeyard checkout CHANGE [--into DIR]` does the same from a script. Test: `tests/checkout.rs` drives the
bridge over stdio against a real server: the fixture's test script fails in the checkout of the
repository and passes in the directory of the change, local edits are reverted, a landing by another
agent in another file sets only `main_moved_elsewhere`, one in an edited file sets `main_moved`, `rebase` brings it in, and after `land` every file of main equals the file in
the directory. A second case runs the bridge inside a `workspace-hook` workspace: the workspace and its
change stay untouched, and the `apply` reply still has the `conflict` collision.

Why not the other two ways. (a) The bridge could keep the agent's working directory in step with its
change. But that directory is often shared: after `treeyard connect` every agent of the repository runs
in the one checkout, and in a `workspace-hook` workspace the daemon would sync the written files into the
workspace's own change, a second change of the same agent that then collides with the first. (b) Agents
could edit on disk and land with `treeyard workspace land`. Then the collision warnings would come only
at sync time, not in each `apply` reply. A separate directory per change has neither problem and costs
one call.

## Native mode: `start`, edit with your own tools, `sync`, `land`

With the agent set, every edit travels as an `apply` payload and most reading goes through `read` and
`grep`. In ten-agent runs of feature-sized tasks that cost about 1.5 times the tokens of plain git
worktrees (a Codex agent made 102 tool calls through TreeYard against 57 with git, where it does most
work in a few shell commands), for the same tasks delivered. In native mode the agent reads, edits and
tests a real directory with its own tools, and TreeYard only receives the result, warns and integrates.
`treeyard connect --native` sets it up ([connect.md](connect.md)); by hand it is
`treeyard mcp --url URL --agent NAME --tools native`.

| Tool | Does |
|---|---|
| `start {intent, paths?}` | opens the agent's change and writes a full checkout of it to a directory of its own (reply: `id`, `dir`, `files`, intent `overlaps`). The directory is the one `checkout` would use, under `$TMPDIR`. |
| `sync {change, rebase?}` | sends what changed in `dir` since the last write or sync into the change, through `apply`. The reply is `apply`'s in the agent view: `collisions`, `overlaps`, `undoes` (with `contributed`), plus `sent`, `deleted`, and `main_moved_in` when work landed in files the change edits. `rebase: true` then rebases onto main and rewrites `dir`. |
| `land {change, undo?, reason?}` | syncs, lands with the usual semantics (held on `undoes`, `undo` + `reason` overrides), and rewrites `dir`. |
| `change`, `diff` | as in the agent set. |

How it works:

- The manifest (path to mode, blob id and stat of every file written to `dir`, and the head it was
  written from) lives beside the directory, never in it: `<dir>.native.json`, with the lock
  `<dir>.native.lock`. The tree holds only the repository's files, so the tests and the agent's own
  `grep` see nothing of TreeYard. A new bridge process (the harness restarted it) finds the checkout
  again by the change id.
- `sync` scans `dir` against the manifest (stat first, then content hashes): modified, added and
  deleted files, with symlinks and exec bits. New files that a `.gitignore` excludes are not sent, nor
  caches and build output of common tools (`__pycache__/`, `*.pyc`, `.pytest_cache/`, `.mypy_cache/`,
  `.ruff_cache/`, `.hypothesis/`, `.tox/`, `.nox/`, `.venv/`, `node_modules/`, `*.egg-info/`,
  `.DS_Store`, `.coverage`); files the change already has are always sent. The files go as whole
  contents through `apply` with `seen` = the head `dir` was written from, so when the change moved
  under the directory (a rebase elsewhere), the write merges 3-way instead of overwriting it. Then
  `dir` is rewritten to the change's head: only files that differ are written.
- A `land` conflict leaves the change rebased with conflict markers in the conflicted files. The
  rewrite puts them in `dir`: `<<<<<<< main` .. `=======` is the work on main, `=======` ..
  `>>>>>>> change` the agent's. The reply lists them in `conflicted` (no regions: the agent has the
  files). The agent fixes them in place and calls `land` again. A conflicted file that still holds a
  marker block is never sent: `sync` reports it in `markers_left`, `land` answers status `refused`
  with `markers_left`, and the file stays as the agent left it (a half-done fix is not overwritten).
  Other files are still sent.
- Conflicts with no markers to edit (a file deleted on one side and changed on the other, a mode or
  binary change) are never settled silently: nothing is sent for them, the reply lists them in
  `decide` (path, kind, what each side did), and `land` answers `refused` until the agent passes
  `resolve: {"path": "mine" | "main"}` on `land` or `sync` (`mine` = the file as it is in `dir`,
  absent = deleted; `main` = main's side).
- When `dir` is older than the change's head (a rewrite after a conflict failed, or the change was
  rebased elsewhere), its copy of a conflicted file is never sent as the resolution: the copy goes to
  `<dir>.native-stale/`, `dir` gets the conflict with markers, and the reply lists
  `stale_conflicts`. The server also checks a resolution for markers after the 3-way merge with a
  stale view, so marker text cannot land through `apply` either.
- New files that look like backups (`*.orig`, `*.rej`, `*~`, `*.bak`, `*.swp`, and `name-e` next to a
  file `name`, what BSD `sed -i -e` leaves) are not sent. Every reply lists the new files it sent in
  `added` and the ones it skipped in `skipped`.
- After `landed`, `dir` holds main as of the land, with the change. The files in the agent's working
  directory never change. When the agent edits `dir` again and calls `sync` or `land` with the landed
  change, a follow-up change on main as it is now takes over the same directory and gets those edits
  (merged 3-way with what main has now); the reply names it in `follow_up` (`from`, `id`, `files`),
  and later calls with the old id go to it. With nothing unsent, the reply says to call `start`.
- The checkout root `$TMPDIR/treeyard-checkouts-<uid>` and each checkout are created 0700; a root
  that is a symlink or belongs to another user is refused. A file recorded in the manifest within 2 s
  of the manifest write is hashed on the next scan even when its size, mtime and mode match.

Test: `tests/native.rs` drives the built bridge over stdio against a real server: a complete tree in
`dir` with the manifest beside it; edits, a new file and a deletion sent, build output, `*.pyc` and
`*.egg-info` not; a new bridge process syncing the same change; `undoes` in a `sync` reply when an
edit removes another agent's landed line, the land held until the line is back; a conflict written as
markers, `land` refused while they remain, then landed; `main_moved_in` and `rebase: true`; a stale
directory merging 3-way.

What an agent can lose: `land` syncs first, and a rewrite of `dir` never overwrites a file that
changed since it was last written or sent (it waits for the next sync), so edits are not lost while
the change is open; edits after a land go into a follow-up change. New files that a `.gitignore`,
the cache list or the backup list excludes never land (they are listed in `skipped`).

A held land (`undoes`) has one rule everywhere (block, `land` schema, reply advice): put the other
agent's part back; `undo: true` with a `reason` only when the agent's own task names those lines for
removal; otherwise keep its part, land the rest, and say what could not hold.

## Many agents, one machine or many

- One server serves all agents on all machines. Each agent needs only the URL (and a token when
  the server is not on loopback: [access.md](access.md)).
- Give each agent its own `--agent` name, or its own token. Blame, log and audit show that name.
- Agents learn about each other through `apply` replies (`collisions`, and `undoes` when an edit
  removes text another agent wrote; `resolve` and `land` say it too) and the `events` tool. A `land`
  that removes lines another agent landed does not land: status `undoes` with the entries and advice
  (agent view: `status`, `change`, `advice`, and per entry `path`, `lines`, `agent`, `intent`,
  `change`, `text`). The `land` tool takes `undo` and `reason` for the case where the task names those
  lines for removal; the land is then recorded (`undid`, event `undo_override`).
  Tell your agents to call `events` with `wait_ms` before a long task, or let the orchestrator
  watch `GET /api/events`.

## MCP over HTTP: `POST /mcp`

The server also speaks MCP itself, over Streamable HTTP at `/mcp`, with the same tools and the same
token rules as `/api`. Use it when the client cannot start a process: a remote agent, or an MCP
gateway that serves a company's own MCP servers to its members.

```text
claude mcp add --transport http treeyard https://HOST/mcp --header "Authorization: Bearer $TREEYARD_TOKEN"
```

- Stateless: every POST is answered with JSON; there is no session id and no server-sent stream
  (`GET` and `DELETE` are `405`). Notifications alone get `202`. A batch gets an array.
- No or unknown token: `401` with `WWW-Authenticate: Bearer`, already on `initialize`, so a
  gateway's connection check sees at once that each member needs a key.
- `Authorization` may hold `Bearer <token>`, HTTP Basic, or the bare token: a gateway that
  forwards a member's key verbatim needs no prefix.
- A non-admin token acts as its own agent, as on `/api`. Its write globs hold for `apply`.

Tested in `tests/mcp_http.rs`. An MCP gateway that stores one key per member and sends it in
`Authorization` reached it end to end on 2026-10-21: each member's agent acted as that member,
with no bridge in between.

## A raw MCP session

```text
$ printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"status","arguments":{}}}' \
  | treeyard mcp --agent a1
```

The first reply has `serverInfo.name` `treeyard` and the instructions. The second has the status
JSON as one text item.
