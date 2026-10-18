# TreeYard reference

Details moved from the README: operations, workspaces, coordination, git over HTTP, the data model, tests and the replay results.

## Operations

The same table serves `POST /api/<op>` (JSON in, JSON out), MCP tools, and `treeyard call`.

| Group | Operations |
|---|---|
| Read | `status`, `read`, `list`, `grep`, `diff`, `log` (`at` = `main`, a change id, a changeset id or prefix, or a git commit); `tree`, `blobs` (for the node daemon) |
| History and search | `blame`, `log` filters (`path`, `paths` globs, `agent`, `since`, `until`), `symbols`, `outline`, `bisect` ([docs/derived.md](derived.md)) |
| Access | `whoami`, `audit`, `token_create`, `token_list`, `token_revoke` ([docs/access.md](access.md)) |
| Write | `open_change`, `apply` (content, base64, delete, exact-once `replace`, executable bit, symlink), `resolve`, `rebase`, `abandon` |
| Land | `land`, `revert` |
| Inspect | `change`, `changes`, `export_change` (writes a change head to `refs/treeyard/changes/<id>`) |
| Coordinate | `declare`, `events` (poll or long poll; `GET /api/events` is the server-sent event stream), `reviews`, `review_close`, `queue`, `queue_run`, `test_main`, `config` |

`open_change` takes optional `paths` (globs) and `symbols` and returns `overlaps` with other open changes
plus `advice`. `apply` returns `overlaps` (shared paths) and `collisions` (a trial merge per shared
path: `conflict` or `overlap`, with the changed line ranges of both sides).

`land` answers with one of five statuses: `landed` (with the git commit), `queued` (landing mode
`queue`), `conflict` (with tasks), `already_on_main` (the rebase found every edit already there; a review
task is opened), or `waiting`. A change opened with
`base` set to another change id is stacked on it, and it waits until that change lands.

## Workspaces

`treeyard workspace create NAME [--setup CMD] [--layer DIR=LOCKFILE] [--git] [--strategy S]` opens a
change and makes `$TREEYARD_HOME/ws/NAME` (default `~/.treeyard`). The node keeps one materialised base
per base changeset. Each workspace is a copy-on-write clone of it: APFS `clonefile` on macOS, reflink on
btrfs and XFS, overlayfs on Linux with mount rights, and hardlinks or copies elsewhere. Dependency
directories are layers keyed by a hash of their lockfiles, built once and cloned into later workspaces.
`.treeyard/env` gives each workspace `PORT` and a block of 10 ports. `treeyard daemon` watches every
workspace and syncs after edits settle (300 ms): each sync is one new changeset of the workspace's change,
so `overlaps` sees everyone's work live. Also `list`, `sync`, `land`, `exec NAME -- CMD`, `env`, `path`,
`spare`, `remove [--abandon] [--force]` (sends the last edits first; refuses without `--force` when it cannot).
A repo file `.treeyard/workspace.toml` (node override `<home>/workspace.toml`) sets `setup`, `teardown`,
`layers`, `copy` and `copy_from` (gitignored files such as `.env`), `ports` and `run`; the flags
`--setup`, `--teardown`, `--run`, `--ports N`, `--copy-from DIR` override it. Each record has a `state` (`creating`,
`setting_up`, `ready`, `removing`, `failed`), `create_log`, `setup_log` and `counts`. `workspace gc [--max N]
[--max-gb G] [--dry-run]` removes landed or abandoned workspaces first, then the least recently synced, but
never one younger than 10 minutes or one with unsynced edits; `treeyard daemon --gc-every HOURS` runs it. The daemon
does not sync a `--git` workspace during a merge, rebase, cherry-pick or with a fresh `index.lock`; it
retries after 1 s, 5 s, 30 s, 120 s. `workspace mirror NAME --into DIR` (`--off` stops it) writes the change's files into
one fixed directory after every sync, touching only files that differ. Details and limits: [docs/workspaces.md](workspaces.md). Tests:
`tests/workspace.rs`; Linux paths: `make test-linux` (Docker).

50 extra agents, each ready to build, on 15-core CPU, 48 GB RAM, 8 at once (`bench/bench.py cow` against
`bench/bench.py workspaces`, 2026-10-17, [docs/benchmarks.md](benchmarks.md)):

| Repo | `git worktree` + setup | TreeYard workspace | TreeYard, warm pool |
|---|---|---|---|
| ripgrep (`cargo build`) | 69.66 s, 7 789 MB | 1.22 s, 48.3 MB | 0.86 s, 5.1 MB |
| astro (`pnpm install --offline`) | 352.53 s, 5 859 MB | 85.30 s, 2 197 MB | 26.19 s, 1 506 MB |

## Coordination

Per repo in `<data>/treeyard.toml` (`treeyard init` writes a commented template). Details, the demo
output and limits: [docs/coordination.md](coordination.md). Tests: `tests/coordination.rs`.

```toml
[coordination]
continuous_rebase = true          # rebase open changes after every land; conflicts show up at once
[merge]
syntax = "auto"                   # mergiraf on the remaining conflicts, when installed
[[merge.driver]]
paths = ["Cargo.lock"]
driver = "cargo-lock"             # 3-way per package; also union, regenerate (a command), ours, theirs
[landing]
mode = "queue"                    # or "optimistic" (test after landing, bisect, revert); default "direct"
test = "cargo test --offline"
```

Every rebase also runs a semantic guard: an edit of the change that the merged result no longer carries
(main already had it, or main lacks the code it changed) opens a review task `absorbed_edit`. In the replay
of ripgrep at 16 agents it flags exactly the known `dbg!` reordering (commit `e92e2ef8137a`).

## Plain git for people

`treeyard serve` also speaks git smart HTTP under `/git`: `git clone`, `fetch`, `pull` and `push`.
A push to main must be a fast-forward of main's exported commit. Each pushed commit becomes a change
that goes through `land`, so it takes the same path as an agent's change, and agents read it at once.
A non-fast-forward push is refused with a hint to run `git pull --rebase`. `treeyard mirror` pushes main
to a read-only mirror (never forced). Rules, the exact rejection text and timings:
[docs/git-http.md](git-http.md). Tests: `tests/git_http.rs`.

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
  `TreeYard-Change`, `TreeYard-Changeset`, `TreeYard-Agent`, `TreeYard-Model` and `TreeYard-Session`
  trailers.

Storage: SQLite (`<data>/treeyard.db`) holds metadata and the blobs that agents wrote. The imported
content stays in the git repo and is read through one `git cat-file --batch`.

## Tests

`make check` (bench unit tests, fmt, clippy, tests). `tests/swarm.rs` is the phase-1 gate: 50 agents open changes on the
same main at once, edit and land through the HTTP API. Every server-side merge is replayed with
`git merge-tree --write-tree` on the exported commits and must produce the same tree. The exported repo
must pass `git fsck --strict`, and every agent's edit must survive.

Phase 4: `tests/derived.rs` (blame equals `git blame --first-parent` on imported history, after agents
land, and at an open change; the `log` index equals the scan and `git log`; indexed `grep` equals the
scan; symbols; bisect finds a planted change and `revert` removes it; derived rows persist and grow
incrementally), `tests/iam.rs` (401 and 403 cases, read filtering, write globs, ownership, audit rows,
git clone and push with tokens, MCP with `TREEYARD_TOKEN`). On real clones (ignored by default):
`cargo test --release --test derived --test xdiff_git -- --ignored` with `TREEYARD_REAL_REPO=<clone>`.

Hardening (SECURITY.md): `tests/hardening.rs` kills a real `treeyard serve` (SIGABRT through
`TREEYARD_FAILPOINT`) at each of 6 points of a land, restarts it, and checks that SQLite main, the git
ref and `git fsck --strict` agree and that the land can be repeated. It also covers duplicate lands that
race, a client that waits for a late server, a lost land reply (a TCP proxy drops it), `503 busy` and
client retries, the data-dir lock, the loopback rule, hostile paths, a hostile push and hostile imported
trees. `tests/fuzz_history.rs` builds random histories (edits, deletes, renames as delete plus add, mode
flips, binary, empty, symlink, CRLF, no final newline, deep and non-ASCII paths; fresh and stale bases;
rebase, resolve, abandon) and checks every land against `git merge-tree`, every export against the
manifest (`git ls-tree -r`) and a full tree write, and every sampled `read` against `git cat-file`.
`TREEYARD_FUZZ_SEEDS=N` runs more seeds; `TREEYARD_FUZZ_SEED=S` replays one.

## Replay on real history

`treeyard replay --src REPO --from REV [--to REV] --agents N [--stacked] [--config TOML] [--intents]
[--wave-base] --data DIR` splits consecutive
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

The replay also reports `first_conflicts` (changes that conflict on their first land) and
`flagged_early` (those of them that `overlaps` reported before any change of the wave landed).

With coordination (phase 3, `--intents --config full.toml`, same 500 commits, seed 1, 2026-10-17; details
in [docs/benchmarks.md](benchmarks.md#phase-3-coordination-replay)):

| Repo | Agents | Thrown away / forced, no coordination | With intents | Waves equal to history | Changes landing per round |
|---|---|---|---|---|---|
| ripgrep | 16 | 111 / 7 | 0 / 0 | 21/32 → 32/32 | 2.34 (stacked: 1) |
| ripgrep | 50 | 247 / 8 | 0 / 0 | 1/10 → 10/10 | 2.93 |
| requests | 16 | 138 / 3 | 0 / 0 | 30/32 → 32/32 | 2.75 |
| requests | 50 | 239 / 3 | 0 / 0 | 0/10 → 10/10 | 3.36 |

The replay declares each commit's exact paths, so this is an upper bound for agents that guess. The
trial landing of `collisions` (both orders on today's main) is right for 72 % to 90 % of warned pairs at recall 0.99 to 1.0; a shared path alone
has precision 0.20 to 0.42.

## Benchmark commands

Commands: `make bench-workspaces`, `make bench-conflicts`, `make bench-queue`, `make bench-history`,
`make bench-treeyard` with `BENCH_DIR=<scratch>`; `python3 bench/bench.py --dir <scratch> report` prints
the tables. `make bench-cow` measures the workspaces. The last two rows: `python3 bench/derived.py --repo
<clone> --dir <scratch> --history 2000` (ripgrep 3fce3b5, 2 000 commits) and `--history 20000` (go
dfc6f7db9bd7, 20 000 commits); raw numbers in [docs/derived.md](derived.md).
