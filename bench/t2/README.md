# T2 experiment kit

Five coding agents at once on one repository: plain `git worktree` per agent (arm A), worktrees plus a
git-native way to integrate early (arm A2), and `treeyard connect` (arm B). The design, the metrics and
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
python3 bench/t2/t2.py rehearse --scratch S    # oracle and noop runs of all cells, arms A, A2 and B
python3 bench/t2/t2.py collect --summary bench/t2/results/rehearsal-summary.txt
python3 bench/t2/t2.py run --scratch S --arm A2 --repo requests --set colliding --rep 1 --harness real --dry-run
python3 bench/t2/t2.py recount --runs S/t2/runs --results bench/t2/results/pilot-v3
```

`run`, `rehearse` and `validate` take `--results DIR` (default: `results/`, or `dev/results/` for the
development set). Point it at a scratch directory for checks: the run files in `results/` are frozen.
`run`, `rehearse` and `pilot` take `--agent-root DIR` (see "Layout").

## Files

| Path | What |
|---|---|
| `repos.json` | pinned URL and commit per repository; `decide`, `suite`, `test_prefix` (`t2-test ARGS`), `test_globs` (test files, for `suite_green_pinned`) |
| `taskdefs.py` | the 30 tasks: prompt, hidden test, reference edits |
| `tasks/<repo>/<set>/<k>-<id>/` | `prompt.md`, `hidden/<path>`, `reference.patch`, `resolved-<k>.patch` (colliding), `meta.json` |
| `tasks/MANIFEST.json` | sha256 of every task file (checked by `test_t2.py`) |
| `block-b.md` | the instruction block that `treeyard connect` writes; arm B refuses another one |
| `astro_hook.mjs` | node resolve hook: astro tests run on the `.ts` sources with no install and no build |
| `t2.py` | setup, generate, validate, run, rehearse, pilot, collect, budget, recount; the scripted agents |
| `test_t2.py` | stub-agent tests of the stops, the hand-back, arm A2, the layout, the metrics; kit and dev-set tests |
| `results/` | `validate.txt`, rehearsal run files, `metrics.json`, `rehearsal-summary.txt`, `stop-tests.txt`, the pilots |
| `dev/` | the development set (click, 10 tasks) for tuning the arm B block, never for a measurement: [dev/README.md](dev/README.md) |

## Arms

- **A**: one worktree and branch `agent-<k>` per agent. When all 5 stop, the runner commits what each
  agent left (commit rule below) and merges `agent-1` to `agent-5` into `main` with `git merge --no-ff`.
- **A2**: A plus the block `A2_BLOCK` in the prompt and `t2-integrate` on the agent's PATH. The agent
  integrates itself when its change passes `t2-test`: the command merges `main` into its branch and,
  when that is clean, fast-forwards `main` to it; on a conflict it leaves the markers in the files and
  the agent resolves them and runs it again. It runs no tests. The runner then merges only what agents
  left unintegrated, with the same hand-back. A2 is not part of preregistration v3.
- **B**: `treeyard connect`, one workspace per agent, MCP tools; the runner lands every open change of
  each agent in slot order.

Every prompt is `[arm block, empty line,] PREAMBLE, empty line, task prompt`. The task prompts and their
hashes are unchanged. `PREAMBLE` (t2.py) is the same text in every arm: what to deliver (the code change
and one regression test in the repository's existing test file for that module; no changelog or release
notes; no unrelated files), the test command `t2-test`, and that the task is finished only when it passes.
The run file records `preamble_sha256`, `arm_block_sha256` (A2, B), `block_sha256` (B), `handback_sha256`
and each agent's `prompt_sha256`.

`arm_b_test_note()` in t2.py is a marked hook: in arm B the agent edits through `apply`, so the files on
disk do not hold its edits until a product feature writes them. The hook returns no text until that
feature exists. `t2-test` called from a directory outside the agent's own (a copy of the tree, as block B
point 6 says) runs in that directory.

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
  its files plus `resolved-<k>.patch`. Arm A2: it integrates in the fixed order 1 to 5 and settles its
  conflicts the same way. Arm B: it syncs its workspace and lands in the fixed order 1 to 5; on a land
  conflict it writes main plus `resolved-<k>.patch`, then lands again.
- `noop`: changes nothing and makes one tool call, so the zero-tool-call stop does not fire.
- `stub`: scripted behaviour for the tests (`T2_STUB_MODES` per slot; `T2_STUB_AS` per slot = `claude` or
  `codex` makes it print that CLI's transcript format, so the parser and resume path of each harness run).
- `real`: 3 Claude Code, 2 Codex. It needs `--yes-real`: the owner's yes and a budget.

## Stops

Per agent, the runner kills the process group at the turn limit, the token limit or the wall-clock
timeout, and the run goes on. The whole run stops with exit 3 and a `stop_reason` on one of these
anomalies: the agent count is not 5; the arm B server does not answer `/health`; an agent makes zero
tool calls in its first invocation (a hand-back answered in words is not an anomaly).

## Metrics of a run file (`"kit": 2`)

| Field | What |
|---|---|
| `suite_green_all` (`suite_green`) | the `suite` command on final main as it is, agent-written tests included |
| `suite_green_pinned` | the same with the test files of the pin (`test_globs`): agent tests removed, changed ones restored; `test_files_restored`, `suite_tail_all`, `suite_tail_pinned` |
| `tasks_on_main`, `done` | hidden deciding tests that pass on final main |
| `merge_conflicts` | A, A2: runner merges with a conflict at the first try; B: `conflict` events with cause `land` |
| `interventions` = `needs_human` | failed hand-backs (agents left out of main) |
| `merges` / `lands` | per agent: conflict, `handback_files`, `handback`, `handback_failure`, `needs_human`, `skipped_paths`, `merge_output`; B: every runner land with its `status` and `error` text (`empty` = an idle workspace change), `undoes_held` (lands held with status `undoes`) |
| `undoes_held` | B: runner lands held because the change removes lines another agent landed (each is also `needs_human`) |
| `integrations`, `integrate_conflicts`, `integrated_by_agent` | A2: each `t2-integrate` call and its outcome |
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
