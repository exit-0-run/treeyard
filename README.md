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

`land` answers with one of four statuses: `landed` (with the git commit), `conflict` (with tasks),
`already_on_main` (the rebase found every edit already there), or `waiting`. A change opened with
`base` set to another change id is stacked on it, and it waits until that change lands.

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

## Replay on real history

`rootstock replay --src REPO --from REV [--to REV] --agents N [--stacked] --data DIR` splits consecutive
first-parent commits into waves of N agents. Without `--stacked`, each agent starts on its commit's true
parent (a stale base) and the wave lands in random order. A conflicted change is thrown away and redone.
With `--stacked`, each agent builds on the previous agent's change and the server makes it wait. Every
server merge is checked against `git -c merge.renames=false merge-tree`, and after every wave main is
compared with the original tree.

Last 500 commits of ripgrep and of requests (2026-10-17, seed 1):

| Repo | Agents | Merges checked | Same tree as git | Server clean, git conflict | Conflicts (redone) | Waves equal to history | Final tree equal |
|---|---|---|---|---|---|---|---|
| ripgrep | 4 | 517 | 496/496 clean | 0 | 21 (19) | 120/125 | yes |
| ripgrep | 16 | 602 | 491/491 clean | 0 | 111 (97) | 21/32 | yes |
| ripgrep | 50 | 737 | 490/490 clean | 0 | 247 (231) | 1/10 | no |
| requests | 4 | 537 | 496/496 clean | 0 | 41 (35) | 121/125 | yes |
| requests | 16 | 635 | 497/497 clean | 0 | 138 (132) | 30/32 | yes |
| requests | 50 | 736 | 497/497 clean | 0 | 239 (233) | 0/10 | no |
| both | 4–50, `--stacked` | 0 | n/a | 0 | 0 | all | yes |

Every clean merge gives the same tree as git. In 2 cases the server reported a conflict where git
merged. Waves diverge for a reason no merge can see. One example: a fix ("remove stray `dbg!`") lands
before the commit that added the `dbg!`. Main does not have the line yet, so the fix is a no-op, and then
the bug lands. Stacked changes prevent this, at the price of landing serially. Land time p95 is under
5 ms.

## Not yet

Copy-on-write workspaces and the node daemon, git smart-HTTP push for people, live collision events and
continuous rebase, merge drivers (lockfiles, generated code), blame and indexes, per-agent permissions,
Postgres/S3. See the plan in the project docs.

## Licence

Apache-2.0. Inspired by Anthropic's write-up of Scion and by Meta's Mononoke (Bonsai changesets).
