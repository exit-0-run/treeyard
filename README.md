# Rootstock

A self-hosted version control server for agent swarms, with git underneath as the source of truth.

Agents talk to the server instead of a local checkout: they read, grep and diff the repo through an
API, open a *change* (no branch), apply edits, and *land*. The server rebases on landing, and a conflict
comes back as a typed task with base, ours, theirs, and who touched the path. Every landed change becomes
an ordinary git commit that continues the history of the repo you imported, so people keep using
`git clone`, `git log` and GitHub.

Status: v0.1, phase 1 of the plan (server core). Single node, single binary.

## Quick start

```sh
make release
./target/release/rootstock init --data /tmp/rs-data --from /path/to/your/repo --branch main
./target/release/rootstock serve --data /tmp/rs-data            # http://127.0.0.1:7420

rootstock call status
rootstock call grep '{"pattern": "fn main"}'
rootstock call open_change '{"agent": "me", "intent": "fix the typo in README"}'
rootstock call apply '{"change": "ch_…", "edits": [{"path": "README.md", "replace": {"old": "teh", "new": "the"}}]}'
rootstock call land '{"change": "ch_…"}'

git clone /tmp/rs-data/git   # the landed commit is on main
```

MCP for Claude Code, Codex or OpenCode (stdio, forwards to the running server):

```sh
claude mcp add rootstock -- rootstock mcp --url http://127.0.0.1:7420 --agent claude-1
```

## Operations

The same table serves `POST /api/<op>` (JSON in, JSON out), MCP tools, and `rootstock call`.

| Group | Operations |
|---|---|
| Read | `status`, `read`, `list`, `grep`, `diff`, `log` (`at` = `main`, a change id, a changeset id or prefix, or a git commit) |
| Write | `open_change`, `apply` (content, base64, delete, exact-once `replace`, executable bit), `resolve`, `rebase`, `abandon` |
| Land | `land`, `revert` |
| Inspect | `change`, `changes`, `export_change` (writes a change head to `refs/rootstock/changes/<id>`) |

`apply` also returns `overlaps`: the other open changes touching the same paths.

## Model

- **Changeset**: immutable, content-addressed (SHA-256). Parents plus the changed paths, each mapped to a
  git blob id and mode (or a deletion), relative to the first parent, plus metadata: change, agent, model,
  session, intent, message, time. The import is one root changeset that lists the whole tree.
- **Change**: a stable id owned by one agent. It points at one head changeset on top of its base and
  survives amend and rebase. Main is the only ref.
- **Conflict**: stored on the change, which keeps living with it. Text conflicts hold the file with
  markers until the path is written again or settled with `resolve`.
- **Git bridge**: each main changeset maps to one deterministic commit (dates and author come from the
  changeset), written as loose objects into the bare repo at `<data>/git`. The commit message carries
  `Rootstock-Change`, `Rootstock-Changeset`, `Rootstock-Agent`, `Rootstock-Model` and `Rootstock-Session`
  trailers.

Storage: SQLite (`<data>/rootstock.db`) holds metadata and the blobs that agents wrote. The imported
content stays in the git repo and is read through one `git cat-file --batch`.

## Tests

`make check` (fmt, clippy, tests). `tests/swarm.rs` is the phase-1 gate: 50 agents open changes on the
same main at once, edit and land through the HTTP API. Every server-side merge is replayed with
`git merge-tree --write-tree` on the exported commits and must produce the same tree. The exported repo
must pass `git fsck --strict`, and every agent's edit must survive.

## Not yet

Copy-on-write workspaces and the node daemon, git smart-HTTP push for people, live collision events and
continuous rebase, merge drivers (lockfiles, generated code), blame and indexes, per-agent permissions,
Postgres/S3. See the plan in the project docs.

## Licence

Apache-2.0. Inspired by Anthropic's write-up of Scion and by Meta's Mononoke (Bonsai changesets).
