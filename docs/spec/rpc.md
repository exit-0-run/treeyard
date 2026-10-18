# Agent Repo RPC, version 0 (draft)

An open protocol for a server that gives coding agents a repository as an API: read without a
checkout, open cheap changes, land through a server-side merge, and get conflicts back as typed
data. TreeYard is the first implementation. Other servers may implement this protocol; the
licence of this text is Apache-2.0, the same as the repository.

Status: draft v0, 2026-10-18. Breaking changes are allowed until v1. The operation list in this
file is checked against the implementation by `tests/docs.rs` (every operation of the table must
be named here). The full parameter tables are in [../api.md](../api.md), generated from the code.

Key words: MUST, SHOULD and MAY have the meaning of RFC 2119.

## 1. Model

| Term | Meaning |
|---|---|
| Repository | One tree of files with one mainline, `main`. Git is the source of truth: each mainline state maps to one git commit. |
| Entry | One path in a tree: `blob` (content id), `mode` (`100644`, `100755`, `120000` symlink, `160000` submodule). |
| Changeset | An immutable snapshot: parent ids, the per-path changes against the first parent, and metadata (agent, model, session, intent, message, time). Its id is the SHA-256 of its canonical JSON. Two servers that store the same changeset give it the same id. |
| Change | A mutable pointer with a stable id (`ch_` + 12 hex), owned by one agent. Fields: `base`, `head`, `state` (`open`, `landed`, `abandoned`), `intent`, `conflicts`. It replaces a branch. While the change is empty, `head` equals `base`; otherwise `head` is a changeset whose only parent is `base`. |
| Revision | A string that names a changeset: `main`, a change id (its `head`), a changeset id or a unique prefix of one, or a 40-hex git commit id. |
| Land | Moves `main` to a changeset that holds the change. If `main` moved since `base`, the server rebases the change first (a 3-way merge per path). |
| Conflict task | A typed record of one path that the server could not merge: `path`, `kind`, `base` / `ours` / `theirs` entries and texts, `theirs_by` (who changed the path on main, with their intent), and a `resolve` hint. `ours` is the change; `theirs` is main. |
| Intent | The paths (globs) and symbols that a change plans to touch, declared at `open_change` or `declare`. |
| Event | A record in an ordered stream: `seq` (increasing), `time`, `kind`, `changes`, `agents`, plus fields of the kind. |

Conflict kinds: `content` (both sides edited the same lines), `add_add`, `modify_delete`, `mode`,
`binary` (binary file, symlink or submodule).

## 2. Transport

A server MUST offer the HTTP binding. It SHOULD offer the MCP binding.

### 2.1 HTTP

- `POST /api/<op>` with a JSON object body (an empty body is `{}`). The reply is a JSON object.
- `<op>` is lowercase ASCII letters and `_`. Another name gets `404`.
- Status codes: `200` success; `400` the body is not JSON; `401` no valid token; `403` the token has
  no right for this call; `422` the operation failed, including an unknown operation; `503` the
  server is busy. An error body is `{"error": "<text>"}`. A `503` body also has `"retry": true`.
- Auth: `Authorization: Bearer <token>`. For git clients, HTTP Basic with the token as the password
  MUST also work. A server MAY run without tokens on a loopback address only.
- `GET /health` returns `200`. `GET /api/events` is the server-sent event stream of `events`.
- `POST /api/tools` returns `{"tools": [...]}`: every operation as an MCP tool definition (name,
  description, JSON Schema of the parameters).

### 2.2 MCP

Each operation is one MCP tool with the same name and the same parameters (`inputSchema`). The
tool result is one `text` content item that holds the JSON reply. A failed call sets `isError`.
TreeYard serves MCP over stdio (`treeyard mcp --url URL --agent NAME`) and forwards each call
to the HTTP binding.

### 2.3 Retries

A client SHOULD retry on `503` and on a lost connection, with a short backoff. `land` MUST be
safe to retry: a second `land` of a change that landed returns `status` `landed` with
`"repeated": true` and does not move `main` again (TreeYard test:
`lost_land_reply_is_retried_and_lands_once` in `tests/hardening.rs`).

## 3. Operations

A server declares one or more profiles. The core profile is required.

### 3.1 Core profile (required)

| Operation | Reply, in short |
|---|---|
| `status` | `main`, `commit`, `files`, `open_changes` |
| `read` | `content` (UTF-8) or `content_b64`, `blob`, `mode`, `at` |
| `list` | `entries` of one directory (`name`, `kind` = `file` or `dir`) |
| `grep` | `matches` (`path`, `line`, `text`) |
| `diff` | unified diff of a change, or between two revisions |
| `log` | mainline changesets, newest first, with their metadata |
| `open_change` | the new change; `overlaps` with other open changes and `advice` |
| `apply` | the change after the edits; `overlaps` and `collisions` |
| `land` | `status` (see 3.4), `commit` when landed, `change` with `conflicts` on conflict |
| `change` | one change: paths, conflict tasks, overlaps |
| `changes` | changes in one state |
| `resolve` | the change after one conflicted path takes `ours`, `theirs`, `base` or `delete` |
| `rebase` | the change on current main; conflicts come back as tasks |
| `abandon` | the change, `state` `abandoned` |
| `revert` | a new change with the inverse of one landed changeset, landed |
| `checkpoint` | marks the head of an open change; `checkpoint` with `id` (`cp_` + 12 hex) and `n` |
| `checkpoints` | the checkpoints of a change: `id`, `n`, `changeset`, `label`, `time`, `paths` changed since the previous one |
| `group` | best-of-N: the changes of a group (set with `open_change` `group`) with state and diff size |
| `land_one_of` | lands one change of a group; once it is on main, the group's other open changes are `abandoned` |
| `attach` | stores a `transcript`, `summary` or `note` with an open change; `dropped_lines` = lines removed as secrets |
| `attachments` | the attachments of a change; `note` names the git note of a landed change |
| `restore_change` | a new head whose tree equals checkpoint `to` (id, number, or a changeset of the change); `undo` names the checkpoint of the head before |

Edits in `apply`: each edit has a `path` and one of `content` (UTF-8), `content_b64`,
`delete: true`, or `replace: {old, new, all?}`. A `replace` without `all` MUST fail unless `old`
occurs exactly once. Optional: `executable`, `symlink`. An optional `seen` names the revision that
whole-file contents are based on; the server merges if the change moved since.

### 3.2 Coordination profile

| Operation | Purpose |
|---|---|
| `declare` | replace a change's intent; returns overlaps |
| `events` | events after `since`; long poll with `wait_ms` |
| `reviews`, `review_close` | review tasks: edits a rebase made vanish, changes already on main, failed tests, reverts |
| `queue`, `queue_run`, `test_main`, `config` | landing modes and the settings in effect |
| `land_status` | what happened to a change after `land`: `passed`, `pending`, `queued`, `rejected`, `reverted` (with the failing output), `not_tested` |

Collision rule: on every `apply`, the server compares the change with every other open change that
touches a shared path. It runs a trial 3-way merge and reports `kind` `conflict` or `overlap` per
path, with the changed line ranges of both sides. The server never blocks an `apply` or an
`open_change` because of an overlap.

Event kinds: `opened`, `intent_overlap`, `collision`, `conflict`, `rebased`, `landed`, `abandoned`,
`review`, `queued`, `rejected`, `test_failed`, `reverted`, `test_passed`. A reply of `events` has
`next`; the client passes it as `since` in the next call.

### 3.3 Optional profiles

| Profile | Operations |
|---|---|
| Node | `tree`, `blobs` (a node builds a workspace directory from them) |
| History | `blame`, `symbols`, `outline`, `bisect` |
| Git | `export_change` (a change head as a git commit under `refs/treeyard/changes/<id>`) |
| Access | `whoami`, `token_create`, `token_list`, `token_revoke`, `audit` |
| Share | `assign`, `claim`, `apply_assigned` (hunk ownership when agents share one directory) |

### 3.4 Land statuses

| `status` | Meaning | Next step of the agent |
|---|---|---|
| `landed` | `main` holds the change; `commit` is its git commit | done |
| `conflict` | the rebase left conflict tasks in `change.conflicts` | write the final content with `apply` or call `resolve`, then `land` again |
| `waiting` | the change is stacked on an open change that has not landed | `land` again after that change lands (event `landed`) |
| `queued` | landing mode `queue`: the change waits for a batch test | watch events `landed` or `rejected` |
| `already_on_main` | main already holds every edit; a review task is opened | nothing to land |

## 4. Rules for a server

1. Merges MUST be deterministic. A clean 3-way merge SHOULD give the same tree as
   `git merge-tree --write-tree` on the same three commits. TreeYard checks this on every merge in
   `tests/swarm.rs`, `tests/fuzz_history.rs` and `treeyard replay`.
2. A land MUST be atomic: `main` moves to the new changeset or does not move.
3. A clean merge MUST NOT drop an edit of either side. A merge driver (union, lockfile, syntax)
   MAY solve a line conflict. It MUST NOT change a merge that is clean without it.
4. Each mainline changeset MUST map to one git commit. The commit SHOULD carry the agent and the
   intent (TreeYard writes git trailers).
5. A server MUST NOT run a command that a request names, unless an operator configured that
   command by name (TreeYard: `treeyard.toml`, never the repository tree).
6. A token MAY limit reads and writes to path globs. A call outside the globs gets `403`.

## 5. Versioning

This is version 0. A v0 server answers `status` and `tools`. A later version adds a `protocol`
field to `status`. New optional fields and new operations are not breaking changes. A client MUST
ignore reply fields that it does not know.
