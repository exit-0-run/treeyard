# treeyard — working notes

Version control server for agent swarms (Rust, single binary). Product docs, plan and state live in
`~/appnice-apps/projects/treeyard/`.

## Commands

- `make check`: `cargo fmt --check`, `clippy -D warnings`, `cargo test`. Run before every commit.
- `make test` · `make release` (binary at `target/release/treeyard`).
- `make check` also runs `make bench-test` (stdlib unittest of `bench/bench.py` on a tiny repo).
- Benchmarks: `make bench-quick` (minutes) or `make bench` (about 3 h, ~70 GB scratch peak), both
  with `BENCH_DIR=<scratch>`; optional `BENCH_OSS=<dir with clones>`, `BENCH_REPOS`, `BENCH_N`.
  Results are JSON in `$BENCH_DIR/results`; docs/benchmarks.md quotes them with the commands.
- Rust comes from Homebrew `rustup` (`/opt/homebrew/opt/rustup/bin`), not on the default PATH; the Makefile
  finds cargo there (`CARGO`), plain `cargo` in a shell needs that dir on PATH.
- Run: `treeyard init --data D --from REPO [--history N]`, `treeyard serve --data D`, `treeyard call <op> '<json>'`,
  `treeyard mcp --url URL --agent NAME`, `git clone http://127.0.0.1:7420/git`,
  `treeyard mirror --data D --remote URL [--every N]`.
- Workspaces (node side, `TREEYARD_HOME`, default `~/.treeyard`): `treeyard workspace create NAME
  [--setup CMD] [--layer DIR=LOCK]`, `list`, `sync`, `land`, `exec NAME -- CMD`, `env`, `path`, `remove
  [--abandon]`; `treeyard daemon` syncs every workspace after edits settle. `make bench-cow`.
- Coordination (docs/coordination.md): `<data>/treeyard.toml` (template written by `init`).
  `treeyard replay ... [--config TOML] [--intents] [--wave-base]`. `MERGIRAF=/path cargo test --test
  coordination` also runs the mergiraf branch of the drivers test.
- Derived data and IAM (docs/derived.md, docs/access.md): `treeyard token create --data D --name N
  [--admin] [--read G] [--write G]`; clients send `TREEYARD_TOKEN`. `python3 bench/derived.py --repo
  CLONE --dir SCRATCH [--history N] [--only blame,log,grep,symbols,bisect,land]`. Real-repo checks
  (ignored): `cargo test --release --test derived --test xdiff_git -- --ignored` (`TREEYARD_REAL_REPO`).

## Layout

| File | What |
|---|---|
| `src/model.rs` | Entry, Manifest (im::OrdMap), Changeset (content-addressed), Change, Conflict |
| `src/store.rs` | SQLite: blobs written by agents, changesets, changes, refs, changeset → git commit |
| `src/git.rs` | object ids, loose-object writes, tree/commit encoding, `git cat-file --batch` reader |
| `src/merge.rs` | per-path 3-way merge (entry level, then `diffy` line merge) |
| `src/repo.rs` | changes, rebase, land, revert, export, read queries |
| `src/rpc.rs` | the one operation table (HTTP, MCP and CLI share it) + MCP tool schemas |
| `src/config.rs` | `treeyard.toml`: drivers, landing mode, continuous rebase; gitignore-style globs |
| `src/coord.rs` | event bus (poll + SSE), intents, live collisions (trial merge), semantic guard, reviews |
| `src/drivers.rs` | merge drivers: union, Cargo.lock per package, mergiraf; regenerate is in `repo.rs` |
| `src/landing.rs` | queue mode (batch test, bisect), optimistic mode (test main, bisect, revert), worker |
| `src/server.rs`, `src/client.rs`, `src/mcp.rs`, `src/main.rs` | axum API, ureq client, stdio MCP, CLI |
| `src/githttp.rs` | git smart HTTP under `/git` (upload-pack; own receive-pack → `land_push`), `mirror` |
| `src/cow.rs` | directory clones: APFS `clonefile`, reflink (`FICLONE`), overlayfs mount, hardlink, copy |
| `src/workspace.rs`, `src/daemon.rs` | node home: bases, workspaces, layers by lockfile hash, ports, `sync`; `notify` daemon |
| `src/replay.rs` | `treeyard replay`: real history in waves of N agents, stale or stacked, checked against git |
| `src/history.rs`, `src/blame.rs`, `src/xdiff.rs` | mainline + changed-path index, `log`; blame cached per (changeset, path); git's Myers diff port |
| `src/search.rs`, `src/symbols.rs`, `src/indexer.rs` | trigram grep index; tree-sitter symbols; eager background indexing |
| `src/bisect.rs`, `src/auth.rs`, `src/audit.rs` | server-side bisect; API tokens with path globs; audit log |
| `tests/replay.rs` | replay on a synthetic linear history (stale merges match git; stacked and intents reproduce it) |
| `tests/coordination.rs` | phase 3 (a)-(f) over HTTP: intents, collisions + SSE, continuous rebase, drivers, landing modes, guard |
| `tests/swarm.rs` | phase-1 gate: 50 agents, every merge checked against `git merge-tree` |
| `tests/git_http.rs`, `tests/git_bridge.rs` | clone/push over HTTP, non-fast-forward, mirror; tree bytes vs git, fsck, trailers, revert, MCP |
| `tests/workspace.rs` | create/sync/remove, every clone strategy, layer reuse, ports, daemon, CLI shell flow; overlay (Linux, root) |
| `tests/derived.rs`, `tests/iam.rs`, `tests/xdiff_git.rs` | blame = git blame, log index, grep index = scan, symbols, bisect; tokens and audit; hunks = git diff |
| `bench/bench.py`, `bench/test_bench.py`, `bench/derived.py` | phase-0 baseline (worktrees, late conflicts, queue, API) and its unittest; phase-4 blame/log/grep/symbols/bisect vs git |

## Conventions and gotchas

- All state goes through `Repo::with` (one mutex). Landing is serialised by it, so there is no git lock.
- A change's `head` equals `base` while it is empty; otherwise its only parent is `base`.
- Stacked changes: if the base changeset belongs to another open change, `land` returns `waiting`.
- Keep the commit message across amend, resolve and rebase (`head_message`); replay caught it being dropped.
- Real histories carry their own fsck errors (requests has a bad timezone); replay counts only new ones.
- Git tree order: a directory sorts as if its name ended in `/` (`write_dir`). Any change to tree or
  commit encoding must keep `import_tree_matches_git_byte_for_byte` green.
- Commits must stay deterministic: their dates and author come from the changeset, never from the clock.
- Loose refs are written next to the packed-refs of the import clone; the loose ref wins.
- Human push (`Inner::land_push`): one change per commit on the ancestry path, each through `land`;
  pushed commits are mapped as they are (`map_commit`), never re-encoded. New pack objects must reach
  `Odb::learn`, or `commit_for` looks for their blobs in SQLite.
- The workspace push guard hook blocks `git push` to a localhost URL typed in a shell; push tests run
  inside `cargo test`.
- Bench: pinned revisions live in `REPOS` in `bench/bench.py`; worktrees come from a local clone in
  `$BENCH_DIR/src`, never from your clones. `replay` reports `first_conflicts` and `flagged_early`.
- On APFS `du` counts clones in full; `df_delta_mb` (statvfs) is the physical cost, noisy under other load.
- Workspaces: `sync` sends whole files (base64) that differ from `meta/<name>.state.json`, one `apply`
  per 8 MiB. Tracked files are always checked; new files obey `.gitignore` (the `ignore` crate, no git
  needed). Layer dirs and `.treeyard/` are never synced. The stat cache needs clones that keep mtime.
- Layers keep mtimes, so `cargo build` in a new workspace is a no-op. A `.venv` holds absolute paths
  (shebangs, editable installs): do not share it as a layer unless it is relocatable.
- Linux paths (reflink on XFS/btrfs, overlay as root) are tested in Docker: docs/workspaces.md.
- `Repo::with` publishes the events of `Inner::emit` after the closure succeeds; a long replay wave
  holds them until its end. `events`, `queue_run`, `test_main` run outside the lock (rpc.rs).
- `land` = mode dispatch (`queue` enqueues); `land_direct` moves main. `revert` and `land_push` always
  use `land_direct`. An empty change can land only if it once had edits (`predecessors`).
- Drivers run only on line-merge conflicts and only when the base is an ancestor of `onto`
  (`is_ancestor`); on a replay stale-parent base they would apply fake deletions.
- Continuous rebase sets `view`; whole-file writes to paths that moved since are merged 3-way.
- Blame must equal `git blame --first-parent`: hunks come from `xdiff.rs` (git's Myers, indent heuristic,
  `trim_common_tail`), renames from `diff-tree -M` with a pathspec of the path plus the deleted files
  (git's `single_follow`). `git log --follow` also finds copies: do not use it. Check with `xdiff_git`.
- Derived data is keyed by content (blob, or changeset + path), so it never goes stale; `touch` holds
  files and `dir/` keys. Bump `INDEX_VERSION` in history.rs when rows change meaning.
- `rpc::call` runs as `Caller::internal()`; HTTP uses `call_as` with the token's caller. Writes and
  denials write an audit row inside the same `Repo::with`. Never put a token secret in a row or a log.
- README: short, ASD-STE100, numbers only in its marked Benchmarks table (source: docs/benchmarks.md); details in docs/reference.md.
- Never add `.github/workflows/` (no GitHub Actions on appnice repos). Commit author: 320388883+exit0-run@users.noreply.github.com.
