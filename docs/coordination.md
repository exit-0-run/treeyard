# Coordination (phase 3)

TreeYard sees every agent's edits while the agents work. Phase 3 uses that to make conflicts appear
early and small, and to land without a global lock. Six parts, each configured per repo in
`<data>/treeyard.toml` (a commented template is written by `treeyard init`; restart `serve` after a
change). The file lives in the data directory and not in the repo tree, because it names commands that
the server runs: an agent's change must never change them.

| Part | Operations and events | Code | Test |
|---|---|---|---|
| (a) Intents | `open_change` / `declare` with `paths` (globs) and `symbols`; reply `overlaps`, `advice`; event `intent_overlap` | `src/coord.rs` | `a_intents_report_overlaps_at_open` |
| (b) Live collisions | `apply` reply `collisions`; events `collision`; `events` (poll, long poll), `GET /api/events` (server-sent events) | `src/coord.rs`, `src/server.rs` | `b_live_collisions_trial_merge_and_events` |
| (c) Continuous rebase | `land` reply `rebased`; events `rebased`, `conflict`; `apply` `seen` | `src/repo.rs` | `c_continuous_rebase_…` |
| (d) Merge drivers | `land` reply `drivers` | `src/drivers.rs`, `src/repo.rs` | `d_merge_drivers_…`, unit tests in `src/drivers.rs` |
| (e) Landing modes | `land` (queued), `queue`, `queue_run`, `test_main`; events `queued`, `rejected`, `test_failed`, `reverted`, `test_passed` | `src/landing.rs` | `e_queue_mode_…`, `e_optimistic_mode_…` |
| (f) Semantic guard | `reviews`, `review_close`; `land` reply `reviews`; event `review` | `src/coord.rs`, `src/repo.rs` | `f_guard_…`, `tests/replay.rs` |

All tests are in `tests/coordination.rs` unless named otherwise. Every operation is in the one table in
`src/rpc.rs`, so HTTP, MCP and `treeyard call` get it at once.

## Demo (two agents, one function)

`/private/tmp/rootstock-harness/009/demo.sh` (not in the repo; the commands are below) ran on
2026-10-17 against `treeyard serve` with `continuous_rebase = true`:

```sh
treeyard call open_change '{"agent":"alice","intent":"fix parse","paths":["lib.rs"],"symbols":["parse"]}'
treeyard call open_change '{"agent":"bob","intent":"speed up parse","symbols":["parse"]}'
treeyard call apply '{"change":"<alice>","edits":[{"path":"lib.rs","replace":{"old":"    1","new":"    10"}}]}'
treeyard call apply '{"change":"<bob>","edits":[{"path":"lib.rs","replace":{"old":"    1","new":"    100"}}]}'
treeyard call land '{"change":"<alice>"}'
treeyard call events '{"change":"<bob>","kinds":["collision","conflict"]}'
```

Output (fields picked by the script):

```
alice opened ch_9e5229d6155f
bob open_change overlaps: [('ch_9e5229d6155f', ['parse'])] | 1 open change(s) plan to touch the same code. To avoid a late conflict: wait for them to land (watch events for kind landed), or stack on one with open_change base = ch_9e5229d6155f, or narrow your paths.
bob apply collisions: [('ch_9e5229d6155f', 'conflict', {'mine': [[2, 3]], 'theirs': [[2, 3]]})]
alice land: landed | rebased: [('ch_5d9244f23620', ['lib.rs'])]
events for bob: [(4, 'collision', None, 'symbol'), (5, 'collision', None, 'conflict'), (8, 'conflict', 'continuous_rebase', None)]
bob conflict task: [('lib.rs', 'content', 'alice')]
```

Bob learns three times before he finishes: at `open_change` (Alice declared `parse`), at his first
`apply` (a trial merge with Alice's change conflicts on line 2), and when Alice lands (his change now
holds the conflict as a task naming Alice).

## (a) Intents

`open_change` takes `paths` (globs, gitignore style: a pattern without `/` matches at any depth, a
leading `/` anchors it at the root, a plain directory name covers everything below it) and `symbols`
(`name` or `path#name`). `declare` replaces them later. The reply lists every open change that
overlaps:

- paths: my globs match its edited paths, or both declared globs match the same file of main;
- symbols: it declared the same name, or one of its changed hunks lies inside the definition of the
  symbol in its base. For Rust, Python, Go, JavaScript and TypeScript the definition comes from
  tree-sitter (`src/symbols.rs`). Otherwise a regex for common keywords (`fn`, `struct`, `impl`,
  `def`, `class`, `function`, …) finds it, and it ends at the next line at the same or smaller indent.

Each overlap carries `via` (`edited` or `declared` per path). `advice` names the choices: wait, stack
(`base = <change>`), or narrow the paths. The server never blocks an open; the orchestrator decides.

## (b) Live collisions and events

Every `apply` (the workspace daemon's `sync` too, since it calls `apply`) checks the change against all
other open changes. A shared path gets a trial 3-way merge with the older of the two bases as the merge
base, the same merge `land` would run, without storing anything. The reply's `collisions` gives
`kind` = `conflict` or `overlap` per path, and the changed line ranges of both sides (`hunks.mine`,
`hunks.theirs`, half-open, 1-based, in base lines). Edits inside another change's declared paths
(`declared`) or symbols (`symbol`) count too. Each new or changed collision of a pair and path is
published once as a `collision` event.

Events: `opened`, `intent_overlap`, `collision`, `conflict` (`cause` = `land`, `continuous_rebase`,
`stale_write` or `queue`), `rebased`, `landed`, `abandoned`, `review`, `queued`, `rejected`,
`test_failed`, `reverted`, `test_passed`. Each event has `seq`, `time`, `kind`, `changes` and `agents`.

- `events {since, change?, agent?, kinds?, limit?, wait_ms?}`: poll; `wait_ms` (max 30 000) makes it a
  long poll. Pass the reply's `next` as the next `since`. MCP clients use this one.
- `GET /api/events?since=N&change=…&agent=…&kinds=a,b`: server-sent events. The first message is
  `hello` with `epoch`; each event has `id` = seq, so `Last-Event-ID` resumes.

Events live in memory (the last 20 000). A restart starts a new `epoch` at seq 0. The reply says
`dropped: true` when the client fell behind the kept window.

## (c) Continuous rebase

With `[coordination] continuous_rebase = true`, every land rebases each other open change onto the new
main, except changes that are stacked on an open change, queued, or already conflicted. A conflict
appears on the change at once, as a task, with a `conflict` event (`cause: continuous_rebase`).

The owner's files can be stale after such a rebase. The server keeps the head that the owner saw
(`view`). A later whole-file write (`content`, `content_b64`, `delete`) to a path that changed since
then is merged 3-way (view, write, current head) instead of overwriting main's edits. `replace` edits
work on the current text and need no merge. A client that read the current head passes it as `seen`;
that write goes in as is and clears `view`. Test `c_…` checks that a stale whole-file write keeps both
edits.

## (d) Merge drivers

Drivers run only where the line merge conflicts (content or add/add), so a clean merge is the same as
git's. First matching rule wins:

```toml
[merge]
syntax = "auto"                  # mergiraf for the rest when it runs; "mergiraf" = must run; "off"
# mergiraf = "/path/to/mergiraf"
[[merge.driver]]
paths = ["Cargo.lock"]
driver = "cargo-lock"            # 3-way per [[package]]; dependency lists merge as sets
[[merge.driver]]
paths = ["CHANGELOG.md", "HISTORY.md"]
driver = "union"                 # both sides' lines, ours first (git merge=union)
[[merge.driver]]
paths = ["package-lock.json", "pnpm-lock.yaml", "poetry.lock", "uv.lock"]
driver = "regenerate"
command = "pnpm install --lockfile-only --offline"
```

- `cargo-lock`: packages keyed by name, version and source. A package that one side added or removed
  follows that side. A package both sides changed merges per field. It stays a conflict when both sides
  change one field differently, or replace the same package version with different new versions.
  Adding `command` runs that command on the merged lockfile afterwards.
- `union`: git's union rule. git's own manual warns that it "tends to leave the added lines in the
  resulting file in random order" ([gitattributes](https://git-scm.com/docs/gitattributes), opened
  2026-10-17). Use it for changelogs and registries only.
- `regenerate`: writes the merged tree (main plus the change) to a temp dir under `<data>/tmp`, runs the
  command there with `TREEYARD_PATH` set, and reads the path back. A failing command leaves the
  conflict. The command runs under the repo lock (the limit is below).
- `ours` / `theirs`: take one side.
- `syntax`: [mergiraf](https://mergiraf.org/languages.html) (opened 2026-10-17; it lists Rust, Python,
  TypeScript, Go, Java and more) on the remaining conflicts. Test `d_…` resolves two `use` lines added
  at the same place in a Rust file with mergiraf 0.20.0 when `MERGIRAF=/path` is set. Without it, the
  test checks that the path stays a conflict.

Safety rule: drivers run only when the change's base is an ancestor of main. On another base (work main
does not have, as in `treeyard replay`'s stale-parent model), main looks as if it deleted that work, and a
driver would apply the deletion silently. The first replay with drivers on that model showed it:
conflicts fell from 111 to 49, but no wave matched history (21 of 32 before), because CHANGELOG.md and
Cargo.lock lost lines (run `t-drivers`, 2026-10-17, ripgrep at 16 agents).

## (e) Landing modes

```toml
[landing]
mode = "queue"                   # or "optimistic"; default "direct"
test = "cargo test --offline"    # sh -c in a temp copy of the tree; exit 0 = pass
batch = 8
timeout_s = 600
```

- `direct` (default): `land` moves main at once. No tests.
- `queue`: `land` returns `queued`. A step takes up to `batch` changes, rebases them one on top of the
  other without moving main, and tests the top. Pass: the batch lands. Fail: bisect the prefixes, land
  the longest passing prefix, and send the first failing change back to its owner (review task
  `tests_failed`, event `rejected`; the change stays open). A change that conflicts only with a
  batch-mate waits for the next batch; a conflict with main is real and goes to the owner. Test `e_queue…`:
  4 changes, the second breaks the test, 3 test runs, 1 lands, then the other 2 land in 1 run.
- `optimistic`: `land` moves main at once. A step tests main. When it fails, it bisects the main
  changesets since the last green one, reverts the first failing changeset through the normal `revert`
  operation, and opens a `reverted_by_bisect` review task for its owner. Test `e_optimistic…`.

`treeyard serve` starts a worker thread for `queue` and `optimistic`. The `queue_run` and `test_main`
operations run one step by hand (tests and CLI). Tests run outside the repo lock: agents keep reading
and applying during a test. Before the queue lands a batch it checks that main and each change are
unchanged; if not, the step reports `stale` and runs again. A `revert` and a human `git push` always land
directly.

## (f) Semantic guard

The replay found merges that are clean but wrong: a fix ("remove stray `dbg!`") lands before the
commit that added the `dbg!`, becomes a no-op, and the bug lands after it. Every rebase now checks each
of the change's own edits. If the result no longer carries a hunk of base → ours against main, that edit
"vanished": main had the same edit already, or main lacks the code the edit changed. Each such path
opens a review task `absorbed_edit` with the vanished hunks (unified diff text), `whole_file`, and the
main changesets that touched the path. A change that a rebase empties lands as `already_on_main` with a
second task `already_on_main`. Review tasks are signals for an external reviewer (RFC: "w v1 jako
sygnał dla zewnętrznego review"); `review_close {id, state: done | dismissed}` closes one.

In the replay of ripgrep at 16 agents (stale parents, 2026-10-17), the guard opened 1 `absorbed_edit`
task, and it is the `dbg!` fix: commit `e92e2ef8137a` "cli: remove stray `dbg!`", path
`crates/cli/src/lib.rs`, a path that is wrong at the end of its wave. Numbers for all runs:
[benchmarks.md](benchmarks.md#phase-3-coordination-replay).

## Limits

- Events are in memory only; a restart loses them (new `epoch`).
- Collision checks scan all open changes on each `apply`: O(open changes) trial merges per shared path.
  Measured cost is in the land and apply numbers of the replay, not separately.
- Symbol ranges come from tree-sitter for Rust, Python, Go, JavaScript and TypeScript (since phase 4,
  docs/derived.md); other languages, and names tree-sitter does not define, use the regex heuristic.
- `regenerate` and queue/optimistic tests copy the whole tree to a temp dir. `regenerate` runs under
  the repo lock, so a slow command blocks every agent for its duration.
- mergiraf can take seconds on large files: in an early wave-model replay one land took 5.2 s (run
  `t3-wfull`, land max 5216 ms). TreeYard passes `--timeout 5000` to cap one file.
- mergiraf prints a `mergiraf review <id>` hint after each solved conflict. Where it keeps that data
  was not checked.
- No auth: anyone who reaches the port can read events.
