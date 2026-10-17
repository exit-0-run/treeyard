# treeyard — working notes

Version control server for agent swarms (Rust, single binary). Product docs, plan and state live in
`~/appnice-apps/projects/treeyard/`.

## Commands

- `make check`: `cargo fmt --check`, `clippy -D warnings`, `cargo test`, `make bench-test`. Run before every commit. `make release`.
- Benchmarks: `make bench-quick` (minutes) or `make bench` (about 3 h, ~70 GB scratch peak), both
  with `BENCH_DIR=<scratch>`; optional `BENCH_OSS=<dir with clones>`, `BENCH_REPOS`, `BENCH_N`.
  Results are JSON in `$BENCH_DIR/results`; docs/benchmarks.md quotes them with the commands.
- Rust: Homebrew `rustup` (`/opt/homebrew/opt/rustup/bin`, not on PATH); the Makefile finds it (`CARGO`).
- Run: `treeyard init --data D --from REPO [--history N]`, `treeyard serve --data D`, `treeyard call <op> '<json>'`,
  `treeyard mcp --url URL --agent NAME`, `git clone http://127.0.0.1:7420/git`, `treeyard mirror --data D --remote URL`.
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
- Hardening (SECURITY.md): `TREEYARD_FUZZ_SEEDS=300 cargo test --test fuzz_history` (default 12 seeds);
  `python3 bench/scale.py --repo CLONE --dir SCRATCH --tag T [--history N] [--git]`; `bench/grep_stall.py` (docs/benchmarks.md).
- Release and docs: `sh scripts/dist.sh` (dist/ + SHA256SUMS + install.sh; Linux musl needs cargo-zigbuild and zig on
  PATH, docs/install.md); `sh scripts/check-quickstart.sh docs/quickstart.md|README.md [--write]` (runs ```sh blocks in
  an empty dir); `examples/swarm-demo.sh --agents N [--pairs P]`; `treeyard api-doc > docs/api.md`;
  `python3 scripts/demo-svg.py assets/demo.txt > assets/demo.svg`. `tests/docs.rs` fails when any of these is stale.

## Layout

| File | What |
|---|---|
| `src/model.rs` | Entry, Manifest (im::OrdMap), Changeset (content-addressed), Change, Conflict |
| `src/store.rs` | SQLite: blobs written by agents, changesets, changes, refs, changeset → git commit |
| `src/git.rs` | object ids, loose writes, tree/commit encoding, incremental trees, cat-file readers, lazy object cache |
| `src/merge.rs` | per-path 3-way merge: entry level, then `git merge-file` (the repo) or `diffy` (dry trial merges) |
| `src/safety.rs`, `src/failpoint.rs` | path/size limits, hostile trees, no write through a symlink, loopback check; crash points |
| `src/repo.rs` | changes, rebase, land, revert, export, read queries |
| `src/rpc.rs` | the one operation table (HTTP, MCP and CLI share it) + MCP tool schemas |
| `src/config.rs` | `treeyard.toml`: drivers, landing mode, continuous rebase; gitignore-style globs |
| `src/coord.rs` | event bus (poll + SSE), intents, live collisions (trial merge), semantic guard, reviews |
| `src/drivers.rs` | merge drivers: union, Cargo.lock per package, mergiraf; regenerate is in `repo.rs` |
| `src/landing.rs` | queue mode (batch test, bisect), optimistic mode (test main, bisect, revert), worker |
| `src/server.rs`, `src/client.rs`, `src/mcp.rs`, `src/main.rs`, `src/githttp.rs` | axum API, ureq client, stdio MCP, CLI; git smart HTTP under `/git` (upload-pack; own receive-pack → `land_push`), `mirror` |
| `src/cow.rs` | directory clones: APFS `clonefile`, reflink (`FICLONE`), overlayfs mount, hardlink, copy |
| `src/workspace.rs`, `src/daemon.rs` | node home: bases, workspaces, layers by lockfile hash, ports, `sync`; `notify` daemon |
| `src/replay.rs` | `treeyard replay`: real history in waves of N agents, stale or stacked, checked against git |
| `src/history.rs`, `src/blame.rs`, `src/xdiff.rs` | mainline + changed-path index, `log`; blame cached per (changeset, path); git's Myers diff port |
| `src/search.rs`, `src/symbols.rs`, `src/indexer.rs` | trigram grep index; tree-sitter symbols; eager background indexing |
| `src/bisect.rs`, `src/auth.rs`, `src/audit.rs` | server-side bisect; API tokens with path globs; audit log |
| `src/apidoc.rs` | docs/api.md from `rpc::tools()`; `GROUPS` must list every operation once |
| `tests/swarm.rs`, `tests/replay.rs` | phase-1 gate: 50 agents, merges = `git merge-tree`; synthetic replay (stale, stacked, intents) |
| `tests/coordination.rs` | phase 3 (a)-(f) over HTTP: intents, collisions + SSE, continuous rebase, drivers, landing modes, guard |
| `tests/git_http.rs`, `tests/git_bridge.rs` | clone/push over HTTP, non-fast-forward, mirror; tree bytes vs git, fsck, trailers, revert, MCP |
| `tests/workspace.rs` | create/sync/remove, every clone strategy, layer reuse, ports, daemon, CLI shell flow; overlay (Linux, root) |
| `tests/derived.rs`, `tests/iam.rs`, `tests/xdiff_git.rs` | blame = git blame, log index, grep index = scan, symbols, bisect; tokens and audit; hunks = git diff |
| `tests/hardening.rs`, `tests/fuzz_history.rs` | kill at 6 failpoints + restart, races, retries, 503, lock, bind, hostile input; random histories = git |
| `bench/bench.py`, `bench/test_bench.py`, `bench/derived.py` | phase-0 baseline (worktrees, late conflicts, queue, API) and its unittest; phase-4 blame/log/grep/symbols/bisect vs git |
| `tests/docs.rs`, `tests/swarm_demo.rs`, `tests/claude_hooks.rs` | api.md, spec, links, demo SVG, README demo table; demo end to end; Claude Code worktree hooks |
| `examples/`, `scripts/`, `site/` | swarm demo, Claude Code hooks; dist, install, quickstart check, demo SVG; static landing page (no deploy) |

## Conventions and gotchas

- All state goes through `Repo::with` (one mutex). Landing is serialised by it, so there is no git lock.
- A change's `head` equals `base` while empty, else its only parent is `base`. Stacked on an open change: `land` = `waiting`.
- Keep the commit message across amend, resolve and rebase (`head_message`); replay caught it being dropped.
- New operation: dispatch + `tools()` in rpc.rs, a group in `apidoc::GROUPS`, a name in docs/spec/rpc.md, regenerate docs/api.md.
- Line merges run `git merge-file --diff-algorithm histogram` (merge-ort's choice): the fuzz test found
  `diffy` merging cleanly where git conflicts. Land writes objects, then one SQLite tx, then the ref.
- Cold `manifest`: `ls-tree` of a mapped commit after `MANIFEST_WALK` steps (mapped tree == manifest, always).
- Export is incremental (`write_tree_changed` from the parent commit's tree); `verify_export` compares it
  with a full `write_tree`. A shallow local source is imported by cloning `objects/` copy-on-write.
- Git tree order: a directory sorts as if its name ended in `/` (`write_dir`). Any change to tree or
  commit encoding must keep `import_tree_matches_git_byte_for_byte` green.
- Commits must stay deterministic: their dates and author come from the changeset, never from the clock.
- Human push (`Inner::land_push`): one change per commit on the ancestry path, each through `land`, mapped
  as is (`map_commit`). New packs reach `Odb::learn`; a rejected pack is deleted. Push tests: `cargo test` only.
- Bench: pinned revisions live in `REPOS` in `bench/bench.py`; worktrees come from a local clone in
  `$BENCH_DIR/src`, never from your clones. `replay` reports `first_conflicts` and `flagged_early`; it counts
  only new fsck errors (real histories carry their own, e.g. requests has a bad timezone).
- On APFS `du` counts clones in full; `df_delta_mb` (statvfs) is the physical cost, noisy under other load.
- Workspaces: `sync` sends changed whole files (base64), one `apply` per 8 MiB; new files obey `.gitignore`
  (`ignore` crate). Layer dirs and `.treeyard/` are never synced. Clones keep mtimes (stat cache; `cargo build`
  is a no-op). Never share a `.venv` layer: it holds absolute paths.
- `Repo::with` publishes `Inner::emit` events after the closure succeeds. `events`, `queue_run`, `test_main` run outside the lock.
- `land` = mode dispatch (`queue` enqueues); `land_direct` moves main. `revert` and `land_push` always
  use `land_direct`. An empty change can land only if it once had edits (`predecessors`).
- Drivers run only on line-merge conflicts and only when the base is an ancestor of `onto` (`is_ancestor`); on a
  replay stale-parent base they would apply fake deletions. Continuous rebase sets `view` (stale whole-file writes merge 3-way).
- Blame must equal `git blame --first-parent`: hunks from `xdiff.rs` (git's Myers, indent heuristic), renames
  from `diff-tree -M` on the path plus deleted files. Never `git log --follow` (finds copies). Check: `xdiff_git`.
- Derived data is keyed by content (blob, or changeset + path), so it never goes stale; `touch` holds
  files and `dir/` keys. Bump `INDEX_VERSION` in history.rs when rows change meaning.
- `rpc::call` runs as `Caller::internal()`; HTTP uses `call_as` with the token's caller. Writes and
  denials write an audit row inside the same `Repo::with`. Never put a token secret in a row or a log.
- README: hero of measured multipliers on top (Patryk: full hype), then short ASD-STE100 text; every number sourced in docs/benchmarks.md; hardware only as "15-core CPU, 48 GB RAM". Product name TreeYard in docs; binary, crate, commands stay `treeyard` until the rename.
- Never add `.github/workflows/` (no GitHub Actions on appnice repos). Commit author: 320388883+exit0-run@users.noreply.github.com.
