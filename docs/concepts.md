# Concepts

TreeYard (the binary is still named `treeyard` in v0.1) is one server between your agents and
git. This page names the parts and how they fit. The protocol is in [spec/rpc.md](spec/rpc.md),
every operation is in [api.md](api.md).

## The picture

```text
 agents (Claude Code, Codex, OpenCode, scripts)          people
   |  MCP (stdio)   |  HTTP /api/<op>   |  CLI              |  git clone / pull / push
   v                v                   v                   v
 +---------------------------------------------------------------+
 | treeyard serve                                               |
 |  changes + changesets (SQLite)   merge / rebase / land        |
 |  events, intents, collisions     blame, log, grep, symbols    |
 |  tokens + audit                  git smart HTTP under /git    |
 +---------------------------------------------------------------+
   |  every land = one deterministic git commit on main
   v
 bare git repository (data/git)  --- treeyard mirror --->  GitHub (read-only mirror)

 node: treeyard workspace / treeyard daemon
   copy-on-write directories of main, synced to their change after each edit
```

## Changes, not branches

- A **changeset** is an immutable snapshot. Its id is the SHA-256 of its content, so the same work
  has the same id everywhere.
- A **change** is a pointer with a stable id (`ch_...`) that one agent owns. It has a `base` and a
  `head`. Each `apply` makes a new head; the id stays. There is no branch to name, push or delete.
- `open_change` costs one SQLite row. An agent can open, try and abandon changes all day.
- A change opened with `base` set to another change id is **stacked** on it. Its `land` returns
  `waiting` until the change under it lands. The replay showed why: without the rule, a fix landed
  before the commit that added the bug ([benchmarks.md](benchmarks.md#phase-3-coordination-replay)).

## Landing without a global lock

`land` rebases the change on current `main` on the server, merges path by path, and moves `main`
in one step. The same mutex that guards all state also orders lands, so there is no git lock and
no checkout. A clean merge gives the same tree as `git merge-tree --write-tree`; tests check this
on every merge ([../tests/swarm.rs](../tests/swarm.rs), [../tests/fuzz_history.rs](../tests/fuzz_history.rs)).

Two landing modes run tests ([coordination.md](coordination.md)):

- `queue`: changes wait for a batch test; a failing batch is bisected and the culprit is rejected.
- `optimistic`: changes land at once; the server tests `main` after, bisects and reverts a culprit.

## Conflicts are data

When a rebase cannot merge a path, `land` returns `conflict` and the change holds one **task**
per path: the kind (`content`, `add_add`, `modify_delete`, `mode`, `binary`), the base, ours and
theirs texts, and `theirs_by` (the agents that changed the path on main, with their intents). The
agent writes the final content with `apply` or picks a side with `resolve`, then lands again.
Nothing is lost: the change keeps every revision.

## See the others while you work

- **Intents.** `open_change` and `declare` take path globs and symbol names. The reply lists every
  open change that overlaps, with advice (wait, stack, or narrow the paths).
- **Collisions.** Every `apply` runs a trial 3-way merge against each open change on a shared path.
  The reply says `conflict` or `overlap`, with line ranges. Events `collision` go to every agent.
- **Continuous rebase** (opt-in): after each land, open changes rebase; conflicts show at once.
- **Semantic guard**: a review task when a rebase makes an edit vanish.

Measured precision of the warnings: shared path 0.20, trial merge 0.395 at recall 0.975 (ripgrep,
16 agents; [benchmarks.md](benchmarks.md#early-warnings-precision-next-to-recall)). So agents get the
trial-merge signal, not every shared path.

## Workspaces for agents that need a directory

Many agents only read and edit through the API. An agent that must build or run tests gets a
**workspace**: a copy-on-write clone of a built base (APFS `clonefile`, reflink, overlayfs), with
dependency layers (`target/`, `node_modules`) keyed by lockfile hash. `treeyard daemon` syncs each
workspace to its change about 300 ms after the last edit, so the server sees the work live and a
deleted directory loses nothing that synced ([workspaces.md](workspaces.md)). For Claude Code, the
`WorktreeCreate` hook makes `claude --worktree` and subagents use workspaces ([mcp.md](mcp.md)).

## Git stays the source of truth

- Every landed changeset maps to one deterministic git commit on `main` (same input, same commit id).
- People use `git clone`, `git pull` and `git push` on `http://HOST:7420/git`. A push must be a
  fast-forward of main; each pushed commit lands as a change ([git-http.md](git-http.md)).
- `treeyard mirror` pushes `main` to GitHub or any remote, never forced.

## History and search on the server

`blame` (equal to `git blame --first-parent`), `log` with path filters, trigram `grep`, tree-sitter
`symbols` and `outline`, and `bisect` with a configured test command ([derived.md](derived.md)).

## Access

Tokens with read and write path globs, admin tokens, one owner per change, and an audit row for
every write and denial ([access.md](access.md), [../SECURITY.md](../SECURITY.md)).
