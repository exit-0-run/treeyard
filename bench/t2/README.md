# T2 experiment kit

Five coding agents at once on one repository (ten in the development sets `mixed` and `heavy`, [dev/README.md](dev/README.md)): plain `git worktree` per agent (arm A), worktrees plus a
git-native way to integrate early (arm A2), `treeyard connect` (arm B), and `treeyard connect --native`
(arm BN, section "Arm BN"; not in any preregistration), and two arms with a test before entry: git with a
test in `t2-integrate` (arm G) and TreeYard with the land queue (arm Q; section "Arms G and Q"). The design, the metrics and
the thresholds of version 3 are frozen in [PREREGISTRATION.md](PREREGISTRATION.md); its pilot on
requests ran in [results/pilot-v3/](results/pilot-v3/README.md) and an audit after it found faults in
the runner. This kit fixes them (section "Since preregistration v3"). Those fixes change the hand-back,
the prompt, the scoring and the metrics, so a run with this kit is not a run of version 3: the next
measurement needs a new preregistration.

Standard library Python. Run every command from the worktree root with a scratch directory `S`
(clones, venvs, cargo target, the runner's run directories; about 1 GB).

```sh
python3 bench/t2/t2.py setup --scratch S       # pinned clones, requests venv, warm cargo build
python3 bench/t2/t2.py generate --scratch S    # tasks/ from taskdefs.py (prompts, hidden tests, patches)
python3 bench/t2/t2.py validate --scratch S    # results/validate.txt
python3 bench/t2/t2.py rehearse --scratch S    # oracle and noop runs of all cells, arms A, A2, B and BN (--arms to choose)
python3 bench/t2/t2.py collect --summary bench/t2/results/rehearsal-summary.txt
python3 bench/t2/t2.py run --scratch S --arm A2 --repo requests --set colliding --rep 1 --harness real --dry-run
python3 bench/t2/t2.py recount --runs S/t2/runs --results bench/t2/results/pilot-v3
```

`validate` and `rehearse` take `--set NAME` (repeatable) to run only those sets; without it they run every
set the repository has (an extra set writes its own `validate-<set>.txt`). `run`, `rehearse` and `validate` take `--results DIR` (default: `results/`, or `dev/results/` for the
development set). Point it at a scratch directory for checks: the run files in `results/` are frozen.
`run`, `rehearse` and `pilot` take `--agent-root DIR` (see "Layout").

## Files

| Path | What |
|---|---|
| `repos.json` | pinned URL and commit per repository; `decide`, `suite`, `test_prefix` (`t2-test ARGS`), `test_globs` (test files, for `suite_green_pinned`) |
| `taskdefs.py` | the 30 tasks: prompt, hidden test, reference edits |
| `taskdefs_<name>.py` (optional, next to a `taskdefs.py`) | sets beyond `disjoint` and `colliding`, with their set-level settings and integration tests; today `dev/taskdefs_mixed.py` and `dev/taskdefs_heavy.py` (its reference solutions as patches in `dev/heavy/`, read into `edits` and `combined_edits`) |
| `tasks/<repo>/<set>/<k>-<id>/` | `prompt.md`, `hidden/<path>`, `reference.patch`, `resolved-<k>.patch` (colliding), `meta.json` |
| `tasks/MANIFEST.json` | sha256 of every task file of `disjoint` and `colliding` (checked by `test_t2.py`) |
| `tasks/<repo>/<set>/set.json`, `tasks/MANIFEST-<set>.json` | an extra set: agent count, harness slots, limits, integration tests (`integration/<id>/`); its own manifest |
| `block-b.md` | the instruction block that `treeyard connect` writes; arm B refuses another one |
| `astro_hook.mjs` | node resolve hook: astro tests run on the `.ts` sources with no install and no build |
| `t2.py` | setup, generate, validate, run, rehearse, pilot, collect, budget, recount; the scripted agents |
| `test_t2.py` | stub-agent tests of the stops, the hand-back, arm A2, the layout, the metrics; kit and dev-set tests |
| `results/` | `validate.txt`, rehearsal run files, `metrics.json`, `rehearsal-summary.txt`, `stop-tests.txt`, the pilots |
| `dev/` | the development set (click, 10 tasks) for tuning the arm B block, never for a measurement: [dev/README.md](dev/README.md) |

## Arms

- **A**: one worktree and branch `agent-<k>` per agent. When all agents stop (5; an extra set can have
  more, `mixed` has 10), the runner commits what each agent left (commit rule below) and merges `agent-1`
  to `agent-N` into `main` with `git merge --no-ff`.
- **A2**: A plus the block `A2_BLOCK` in the prompt and `t2-integrate` on the agent's PATH. The agent
  integrates itself when its change passes `t2-test`: the command merges `main` into its branch and,
  when that is clean, fast-forwards `main` to it; on a conflict it leaves the markers in the files and
  the agent resolves them and runs it again. It runs no tests. The runner then merges only what agents
  left unintegrated, with the same hand-back. A2 is not part of preregistration v3.
- **A3** ("git, merge often"): A2 with the block `A3_BLOCK` instead of `A2_BLOCK`. It adds a concrete rule:
  commit and run `git merge main` before starting, after each logical step, at least every 10 minutes of
  work and right before integrating, and resolve a conflict at once, so conflicts come early and in small
  pieces. Same `t2-integrate`, same runner. `rehearse` runs it only when named (`--arms A,A2,A3`); not part
  of preregistration v3.
- **B**: `treeyard connect`, one workspace per agent, MCP tools; the runner lands every open change of
  each agent in slot order.

Every prompt is `[arm block, empty line,] PREAMBLE, empty line, task prompt`. The task prompts and their
hashes are unchanged. `PREAMBLE` (t2.py) is the same text in every arm: what to deliver (the code change
and one regression test in the repository's existing test file for that module; no changelog or release
notes; no unrelated files), the test command `t2-test`, and that the task is finished only when it passes.
The run file records `preamble_sha256`, `arm_block_sha256` (A2, A3, B), `block_sha256` (B), `handback_sha256`
and each agent's `prompt_sha256`.

`arm_b_test_note()` in t2.py is a marked hook: in arm B the agent edits through `apply`, so the files on
disk do not hold its edits until a product feature writes them. The hook returns no text until that
feature exists. `t2-test` called from a directory outside the agent's own (a copy of the tree, as block B
point 6 says) runs in that directory.

## Arm BN

Arm B connected in native mode: `treeyard connect --native`, so each agent's MCP entry runs
`treeyard mcp --tools native` (`start`, `sync`, `land`, `change`, `diff`; [docs/mcp.md](../../docs/mcp.md))
and the block is `block-bn.md` (the run refuses any other; `tests/connect.rs` ties it to the constant
in `src/connect.rs`). The agent calls `start`, reads, edits and runs `t2-test` in the directory `dir` of
its reply with its own tools, and lands with `land`. Everything else is arm B: the same workspaces as
cwd, server, metrics, hand-back and stops. What differs:

- The preamble note (`arm_bn_test_note()`): run `t2-test` from `dir`.
- `dir` is in the agent's TMPDIR (`box/tmp/treeyard-checkouts-<uid>/<change>`, its manifest beside it), so
  it is inside the Codex writable root (the box). Claude Code gets `--add-dir box/tmp` to edit there.
- The runner lands an open change that has a `start` directory through the bridge with the agent's
  environment, so edits the agent made and never synced go in (after the commit rule removed env and
  lock files from the directory too; `skipped_paths` lists them as `<change>:<path>`). On a conflict the
  bridge writes the markers into `dir`; the hand-back (`HANDBACK_BN`: the same text, naming `dir`)
  resumes the agent there, and the runner's next bridge `land` sends the files, or answers `refused`
  while markers are left (the hand-back failed). `lands[].runner_landed[]` marks these with `native`.
- `warned` counts collisions in the agent's own `sync` replies too (op `sync`); a `start` reply names the
  agent's change; a `sync` after the warning is a reaction. The agent's own `land` replies count as well:
  a hold (status `undoes`) is a warning of kind `undoes` (`land_holds`); a later land that lands without
  `undid` is a reaction (`hold_reacted`), one with `undid` an override (`undoes_overridden`).
  `agent_land_conflicts` and `land_refused` count conflict and markers-left replies (per agent in
  `warnings`). `undoes_held` (per agent `lands[].undoes_held_changes`) counts the distinct changes held by
  the agent's land or the runner's; `undoes_held_runner` is the runner's alone, what arm B counts.
- Every run file records the binary: `treeyard_sha256`, `treeyard_version`. Arm BN archives each start
  directory (with its manifest and stale copies, without caches) in `agents/box/agent-<k>/start/`.
- `health --scratch S --run S/t2/runner/<run> --repo R --set SET` runs the hidden and integration tests at
  every first-parent commit of the run's final main (in a clone in the temp directory) and lists the
  breaks: a test that passed and fails at a later commit, transient when it passes again.
- `recount --metrics --runs S/t2/runner --results DIR [--out DIR2]` counts these again for finished arm B
  and BN runs from their run files and transcripts, without a run: one row per run (with the recorded
  values), and with `--out` the recounted run files (the originals stay; `false_alarms` keeps its value).
- The scripted harnesses drive the native flow for real through a bridge on stdio (`NativeBridge`):
  the oracle starts, `git apply`s `reference.patch` in `dir`, syncs, lands in the fixed order and on a
  conflict writes main plus `resolved-<k>.patch` over the markers in `dir`; `noop` calls `start` only;
  the stub modes write `a.txt` in `dir` and sync or land (in the `claude` and `codex` dialects as MCP
  tool calls).

```sh
python3 bench/t2/t2.py run --scratch S --arm BN --repo click --set heavy --rep 1 --harness oracle --results S/bn
python3 bench/t2/t2.py run --scratch S --arm BN --repo click --set heavy --rep 1 --harness real --dry-run
python3 bench/t2/t2.py run --scratch S --arm BN --repo click --set heavy --rep 1 --harness real --yes-real
```

## Arms G and Q (a test before entry)

Two arms for a comparison where every change must pass a test before it is on main. Not in any
preregistration. `rehearse` runs them only when named (`--arms G,Q`).

- **G** ("git, a test for every change"). Each agent has its own clone of main, on branch `agent-<k>`,
  with no remote. `t2-integrate` (block `G_BLOCK`) commits the agent's work, fetches main, merges it into
  the branch, runs `t2-test` on the result and moves main to it only when the test passes. The move is a
  compare-and-swap: when main moved during the test, the command fetches, merges and tests again. Calls
  of different agents run at the same time. Its answers: a conflict (the files with markers, as in A2),
  the test failed (with the end of its output; main did not change), or integrated. Main's repository
  stays with the runner: its HEAD is detached and a hook refuses every push, so an agent cannot move
  main around the test. After the agents stop, the runner sends work left out of main through the same
  command; a conflict there is handed back once; work that still conflicts or fails the test stays out
  (`needs_human`).
- **Q** (TreeYard with the land queue). Arm BN with `connect --land-queue <t2-test> --land-batch N`
  (`--land-batch`, default 8; the server's default `parallel`). `land` waits until the server has tested
  a batch of changes on main in its own test slots. A change that fails comes back to its agent as
  `tests_failed` and stays open; the agent fixes it and lands again. The prompt is BN's: the frozen
  connect block, then `Q_NOTE` (two sentences on the wait and on `tests_failed`), then the preamble. The
  queue's test is a copy of `t2-test` in the runner's directory, with the kit's venv and
  `PYTHONPATH=src`; click needs no `--land-queue-setup`. The runner lands what agents left open through
  the bridge, which waits for the queue too; a change the queue rejects there stays out.

Both arms give Claude Code longer tool timeouts (`LONG_TOOL_ENV`), because `t2-integrate` and `land` wait
for tests. Arm Q gives Codex `tool_timeout_sec = 3600` for the MCP server.

### The padded test

`run --test-pad-s S` makes `t2-test` the repository's suite followed by `sleep S`, whatever the suite's
result; the exit code is the suite's. It is the same command for the agents' own runs, for arm G's
integrate and for arm Q's queue. By default only `t2-test` without arguments is padded
(`--test-pad-scope suite`); `--test-pad-scope all` pads every call. The run name ends in `-pad<S>`, and
the run file has `test_pad_s` and `test_pad_scope`. Without the option nothing changes.

### Records of arms G and Q

The run file has `entry`:

| Field | Arm G | Arm Q |
|---|---|---|
| `first_ready_s` | first `t2-integrate` call of any agent (seconds after the agents started) | first `land` call (server audit, ms) |
| `last_entry_s` | last move of main | last `landed` event (server, 1 s resolution) |
| `entry_time_s` | `last_entry_s - first_ready_s` | the same |
| `entries` | moves of main (`entries_by_runner`: after the agents stopped) | `landed` events |
| `test_runs`, `test_runs_failed`, `test_seconds` | tests run by `t2-integrate` | `test_run` events of the queue |
| `repeats_main_moved` | tests run again because main moved during a test | - |
| `calls`, `statuses` | every `t2-integrate` call: start, end, tests, repeats, answer | - |
| `rejected_changes`, `rejections`, `rejected[]` | - | changes the queue sent back; per change its slot, how often, and `entered_later` |
| `verdicts` | - | the answers of every `land`: the agents' own (from transcripts) and the runner's |
| `server_events` | - | counts of `queued`, `rejected`, `landed`, `test_run`, `conflict` |

Tasks on main, integration tests, cost and wall time are recorded as in every arm. Arm Q also records
`land_queue` (batch, parallel, setup) and `q_note_sha256`.

### Stub modes for these arms

`T2_STUB_MODES` takes `gate-ok` and `gate-fail` (arm G), `queue-ok` and `queue-fail` (arm Q). Each stub
adds a file of its own (`t2_stub_<k>.txt`), so no two stubs conflict, and enters at once. A `-fail` stub
also adds `tests/test_t2_stub_fail.py`, a test that fails; after the failure it removes that file and
enters again. With 13 stubs entering at the same moment, these modes load the entry path much harder
than agents that finish at different times.

```sh
python3 bench/t2/t2.py run --scratch S --arm G --repo click --set hard --rep 1 --harness oracle --test-pad-s 57 --results S/g
T2_STUB_MODES=queue-ok,queue-fail,queue-ok,... python3 bench/t2/t2.py run --scratch S --arm Q --repo click --set hard --rep 1 --harness stub --test-pad-s 57 --results S/q
```

## Broken or deleted work, every real run so far

Did one agent's merge break or delete another agent's work (a task passed on main and a later
merge or conflict resolution of another agent made it fail)? Counted by script where main's
history exists and from the audit's per-commit table where it does not; per run in
[`dev/losses-all.md`](dev/losses-all.md).

| Runs | Set | Runs | Broke or deleted another's work | Work alone, not together | How counted |
|---|---|---|---|---|---|
| Git, merge at the end (A) | heavy | 12 | 2 | 2 | audit table |
| Git, agents integrate (A2) | heavy | 13 | 2 | 1 | audit table |
| Git, merge at the end (A) | hard | 1 | 0 | 0 | script |
| Git, a 60 s test for every change (G) | hard | 2 | 1 | 2 | script |
| **Git, all** | | **28** | **5** | **5** | |
| TreeYard, API mode (B) | heavy | 12 | 0 | 1 | audit table |
| TreeYard, native mode (BN) | heavy | 10 | 0 | 1 | audit table |
| TreeYard, native with land test (BN) | heavy | 3 | 0 known, 1 not traced | 1 | final main only |
| TreeYard, land queue v1 (Q) | hard | 1 | 0 | 0 | script, every state of main |
| **TreeYard, all** | | **26** | **0 known, 1 not traced** | **3** | |

Git 5 of 28 runs, TreeYard 0 known of 26, with two limits that belong to these numbers: one
TreeYard run (r42 of the land-test series) lost a task and its history was deleted before anyone
traced why, so the count is "0 known", not 0; and this is a table made after the fact from runs of
different sets, arms, binaries and prompts that were not drawn for this comparison, in which three
of the four `heavy` cases come from runs with two kinds of agent, and with one model the count is
2 of 15 git runs. The table says where the loss was seen, not how much more often it happens with
git.

## Hand-back

The same mechanics in every arm. On a conflict at the runner's merge (A, A2) or land (B):

1. The runner prepares the conflict as files with markers in the agent's own directory, with the labels
   of arm B: `<<<<<<< main`, the lines on main, `=======`, the agent's lines, `>>>>>>> change` (no base
   section). Arm A: `git merge --no-commit main` in the worktree, then `git merge-file` with these labels
   on each conflicted file. Arm B: the change's conflicted paths as the server holds them (`read` at the
   change); `workspace land` writes them itself for the workspace change; for a change made with
   `open_change` the runner writes them into the workspace and puts the workspace files back afterwards.
2. The agent is resumed once with `HANDBACK` (t2.py): edit the listed files, keep both sides, remove every
   marker, do not merge, commit or land.
3. The runner checks the files for markers. None: arm A commits the merge in the worktree and merges the
   branch into main; arm B writes the files into the change with `apply` and lands it. A path that the
   agent left as prepared but settled through its own tools is taken from the server.
4. When the check or the merge or land fails, the agent is `needs_human`: its change stays out of main,
   the final tree is scored without it, and it counts as 1 intervention. Nothing is resolved by taking a
   side (no `-X theirs`, no `resolve take = ours`).
5. Arm B: a land the server holds with status `undoes` (the change removes lines another agent landed)
   is handled like a failed hand-back: not landed, `needs_human`, 1 intervention, no hand-back, and the
   runner never lands it with `undo`. `lands[].undoes_held` lists the change and its entries (path,
   lines, agent, change, text); `undoes_held` counts them per run.

The limits hold over all invocations of an agent: a hand-back resumes the session with what is left of
the turns, the tokens and the wall clock, and Claude Code gets the remaining `--max-turns` and
`--max-budget-usd`. An agent with no time left is not resumed, and its hand-back fails.

## Layout

| Where | What | Within an agent's reach |
|---|---|---|
| `<agent root>/<run>/wt/agent-<k>` (A, A2), `.../node/ws/agent-<k>` (B) | the agent's directory | yes |
| `<agent root>/<run>/repo` (A, A2) | the worktrees' common repository | yes (git) |
| `<agent root>/<run>/box/agent-<k>/` | `home/` (HOME), `tmp/` (TMPDIR), `bin/` (`t2-test`, `t2-integrate`), `cfg/` (CLAUDE_CONFIG_DIR or CODEX_HOME), real runs: `venv/`, `target/`, `cargo/` | yes |
| `<agent root>/<run>/bin/treeyard` | the runner's copy of the binary (B), so `ps` shows no kit path | yes |
| `S/t2/runner/<run>/` | `ctx.json`, transcripts `agent-<k>.jsonl`, `final/`, `final-pinned/`, B's server repository, after the run `agents/` (the archived agent area) | no |
| `S/t2/{src,env,validate,pairs}` | pinned clones, the kit's venvs and warm cargo target, reference trees | no |

The agent root of a real run is a new directory in the system temp dir (or `--agent-root`), never the
scratch directory. `reach_check` refuses a real run when an ancestor of the agent directory (below the
home directory) holds the kit, the tasks, `S/t2`, the runner's directory or the results, or when the agent
root holds other runs. Scripted harnesses use `S/agents`. After the run the boxes lose `home`, `tmp`,
the env copies and the copied Codex login, and the whole agent area moves to `S/t2/runner/<run>/agents/`,
so no later agent can see an earlier run's trees. Run files replace local paths with `<agents>`,
`<run>`, `<scratch>`, `<kit>` and `~`.

The agent's environment is built new: `PATH` (without kit or scratch entries), locale, `TERM`, `USER`,
`SHELL`; `HOME`, `TMPDIR` and the XDG dirs in the box; a neutral git identity; `UV_OFFLINE`,
`PIP_NO_INDEX`, `CARGO_NET_OFFLINE`. Claude Code gets `CLAUDE_CONFIG_DIR` and its OAuth token in the
environment; Codex gets `CODEX_HOME` with a copy of the login, `workspace-write` with the box as an extra
writable root (`sandbox_workspace_write.writable_roots`).

Test environment per repository (`t2-test`, absolute paths, no network):

- requests, click: the agent's own venv (same Python and packages as the kit's, from the uv cache),
  `PYTHONPATH=src`; `t2-test` = the `suite` command, `t2-test ARGS` = pytest with ARGS.
- ripgrep: a copy-on-write clone of the warm `S/t2/env/rg-target` per agent as `CARGO_TARGET_DIR`, a
  `CARGO_HOME` of its own that reads the downloaded registry through symlinks, offline.
- astro: node with a copy of the resolve hook in the box.

Commit rule (arm A, A2): a path that the pin tracks is always committed; a new path only when the
repository's ignore rules and the kit list (`KIT_IGNORE_FILES`, `KIT_IGNORE_DIRS`: lock files the pin does
not track, venvs, caches, build dirs) both let it in. A kept-out path that the agent staged is unstaged.
Arm B removes the same files from the workspace before the runner lands it. The run file lists them
(`skipped_paths`).

## Harnesses

- `oracle`: applies `reference.patch`. Arm A: it commits; on the hand-back it writes main's content of
  its files plus `resolved-<k>.patch`. Arm A2: it integrates in the fixed order 1 to N and settles its
  conflicts the same way. Arm B: it syncs its workspace and lands in the fixed order 1 to N; on a land
  conflict it writes main plus `resolved-<k>.patch`, then lands again. An oracle that does not get its
  change in (held, an error, three conflicts) still lets the next one go on, so the run ends.
- `noop`: changes nothing and makes one tool call, so the zero-tool-call stop does not fire.
- `stub`: scripted behaviour for the tests (`T2_STUB_MODES` per slot; `T2_STUB_AS` per slot = `claude` or
  `codex` makes it print that CLI's transcript format, so the parser and resume path of each harness run).
- `real`: 3 Claude Code, 2 Codex (an extra set's `set.json` gives its own slots: `mixed` and `heavy` have 6
  Claude Code, 4 Codex). It needs `--yes-real`: the owner's yes and a budget.

## Stops

Per agent, the runner kills the process group at the turn limit, the token limit or the wall-clock
timeout, and the run goes on. The whole run stops with exit 3 and a `stop_reason` on one of these
anomalies: the agent count is not the set's (5, or `agents` of its `set.json`); the arm B server does not answer `/health`; an agent makes zero
tool calls in its first invocation (a hand-back answered in words is not an anomaly).

## Metrics of a run file (`"kit": 2`)

| Field | What |
|---|---|
| `suite_green_all` (`suite_green`) | the `suite` command on final main as it is, agent-written tests included |
| `suite_green_pinned` | the same with the test files of the pin (`test_globs`): agent tests removed, changed ones restored; `test_files_restored`, `suite_tail_all`, `suite_tail_pinned` |
| `tasks_on_main`, `done`, `n_tasks` | hidden deciding tests that pass on final main; the set's task count |
| `integration`, `integration_passed`, `integration_total` | extra sets: hidden integration tests (both behaviours of a designed pair at once) on final main, a metric apart from `tasks_on_main` |
| `set_agents`, `set_mix` | extra sets: the agent count and the harness slots of its `set.json` |
| `merge_conflicts` | A, A2: runner merges with a conflict at the first try; B: `conflict` events with cause `land` |
| `interventions` = `needs_human` | failed hand-backs (agents left out of main) |
| `merges` / `lands` | per agent: conflict, `handback_files`, `handback`, `handback_failure`, `needs_human`, `skipped_paths`, `merge_output`; B: every runner land with its `status` and `error` text (`empty` = an idle workspace change), `undoes_held` (lands held with status `undoes`) |
| `undoes_held` | B: runner lands held because the change removes lines another agent landed (each is also `needs_human`) |
| `integrations`, `integrate_conflicts`, `integrated_by_agent` | A2, A3: each `t2-integrate` call and its outcome |
| `conflict_work` | A, A2, A3, Claude agents: `claude_seconds` and `claude_usd` spent on conflict work, `episodes`, their shares of the Claude agents' totals (`share_of_claude_seconds`, `share_of_claude_usd`), `per_agent`. A: the hand-back invocations. A2, A3: each episode from a `t2-integrate` or `git merge`/`git pull` that reported a conflict to the next clean integrate or merge (or the `git commit` that concludes a merge); an episode's cost is the invocation's cost times its share of price-weighted tokens (input 1, cache write 2, cache read 0.1, output 5) |
| `flow` | A, A2, A3: `changes_on_main` and `changes_per_hour` (moves of main by an agent's work: successful `t2-integrate` calls and runner merges), `wait_s_median`, `wait_s_p95` (A2, A3: from the first integrate attempt of a piece of work to its work on main), `retries` (attempts that did not put the work on main because main had moved, followed by another attempt) |
| `warned`, `reacted`, `warnings`, `warned_pairs`, `false_alarms` | from each agent's own tool replies: warned = a reply of its own call held a file-level collision (`path`, kind overlap or conflict; with main counts); reacted = after it, before its next land, a `change`, `rebase`, `apply` or `resolve`. Descriptive only |
| `legacy_warned_before_land`, `legacy_reacted`, `legacy_false_alarms`, ... | the event-stream metrics of preregistration v3 (in old run files: `warned_before_land`, `reacted`) |
| `guard` | per agent: files, lines added and removed, test lines added, paths of its own diff: what its integrations (A2), the runner's merge (A, A2) or its landed changes (B) put on main; work left out counts from the merge base |
| `test_command`, `test_passed_agents` | per agent: `t2-test` runs in its transcript, and whether one passed |
| `tokens_in`, `tokens_out`, `tokens_fresh_in`, `tokens_cached_in`, `tokens_cache_write`, `by_harness` | all input (cached or not) and output; the price-free split; sums per harness |
| `cost_usd_claude_only` (`cost_usd`) | what Claude Code reports; Codex reports no cost |
| `models` | model names per harness (Claude: stream events; Codex: its rollout files) |
| `agent_stats` | per agent: turns, tool calls, tokens, cost, wall time, `invocations`, `killed` |

Token counting: Claude Code prints one `assistant` event per content block, all with the same message id
and usage, so usage and turns count once per message id while it runs; the `result` event's `usage` and
`num_turns` replace them at the end of each invocation. `total_cost_usd` and Codex's `turn.completed`
usage are session totals: a resumed hand-back adds only the difference. Codex prints usage only at
`turn.completed`; for the live token limit the runner reads the `token_count` events of its rollout file
in `CODEX_HOME` every 2 s (Codex writes one after each model call), so the kill can come up to one model
call and 2 s late. `recount --runs DIR`
counts old transcripts both ways; old run files stay as they are.

`collect` reads old and new run files: for old ones `warned_before_land` and `reacted` show as the legacy
columns and the new transcript metrics are empty. The summary has the tokens per harness.

## Since preregistration v3

- The hand-back text and mechanics (`HANDBACK`; v3's `HANDBACK_A` and `HANDBACK_B` stay in t2.py for
  reference), and `needs_human` in place of the runner taking one side.
- The common preamble in every prompt, and arm A2.
- Tokens counted once per Claude message and without the Codex resume double count (the pilots'
  tokens are restated by `recount`; the README of pilot v3 has the corrections).
- The new metrics above; the v3 metrics stay under `legacy_*` keys.

## Gotchas

- `treeyard workspace ...` defaults to `--url http://127.0.0.1:7420`. The kit always passes `--url`.
- ripgrep's `grep-matcher` sets `autotests = false`, so the hidden test of `d4-match-overlaps` lives in
  `crates/searcher/tests/`. ripgrep has unit tests inside `src/` too: `suite_green_pinned` cannot
  restore those.
- `fill` and `make_box` put the absolute venv, cargo and hook paths into commands at run time; no path is
  in a file of the kit.
