# T2 experiment kit

Five coding agents at once on one repository: plain `git worktree` per agent (arm A) against
`treeyard connect` (arm B). The design, the metrics and the thresholds are frozen in
[PREREGISTRATION.md](PREREGISTRATION.md). This kit has had no real-agent run yet.

Standard library Python. Run every command from the worktree root with a scratch directory `S`
(clones, venv, cargo target, run directories; about 1 GB).

```sh
python3 bench/t2/t2.py setup --scratch S       # pinned clones, requests venv, warm cargo build
python3 bench/t2/t2.py generate --scratch S    # tasks/ from taskdefs.py (prompts, hidden tests, patches)
python3 bench/t2/t2.py validate --scratch S    # results/validate.txt
python3 bench/t2/t2.py rehearse --scratch S    # oracle and noop runs of all 12 cells, both arms
python3 bench/t2/t2.py collect --summary bench/t2/results/rehearsal-summary.txt
python3 bench/t2/t2.py run --scratch S --arm B --repo requests --set colliding --rep 1 --harness real --dry-run
```

## Files

| Path | What |
|---|---|
| `repos.json` | pinned URL and commit per repository, the deciding test and suite commands |
| `taskdefs.py` | the 30 tasks: prompt, hidden test, reference edits |
| `tasks/<repo>/<set>/<k>-<id>/` | `prompt.md`, `hidden/<path>`, `reference.patch`, `resolved-<k>.patch` (colliding), `meta.json` |
| `tasks/MANIFEST.json` | sha256 of every task file (checked by `test_t2.py`) |
| `block-b.md` | the instruction block that `treeyard connect` writes; arm B refuses another one |
| `astro_hook.mjs` | node resolve hook: astro tests run on the `.ts` sources with no install and no build |
| `t2.py` | setup, generate, validate, run, rehearse, pilot, collect, budget; the scripted agents |
| `test_t2.py` | stop tests with stub agents (`make bench-test`) |
| `results/` | `validate.txt`, one JSON per rehearsal run, `metrics.json`, `rehearsal-summary.txt`, `stop-tests.txt` |

## Harnesses

- `oracle`: applies `reference.patch`. Arm A: it commits; on the hand-back it merges `main` and applies
  `resolved-<k>.patch`. Arm B: it syncs its workspace, lands in the fixed order 1 to 5, and on a land
  conflict writes main plus `resolved-<k>.patch`, then lands again.
- `noop`: changes nothing and makes one tool call, so the zero-tool-call stop does not fire.
- `stub`: scripted misbehaviour for the stop tests (`T2_STUB_MODES`).
- `real`: 2 Claude Code, 2 Codex, 1 OpenCode. It needs `--yes-real`: the owner's yes and a budget.

## Stops

Per agent, the runner kills the process group at the turn limit, the token limit or the wall-clock
timeout, and the run goes on. The whole run stops with exit 3 and a `stop_reason` when the agent count
is not 5, the arm B server does not answer `/health`, or an agent makes zero tool calls. `test_t2.py`
has one test per stop.

## Gotchas

- `treeyard workspace ...` defaults to `--url http://127.0.0.1:7420`. The kit always passes `--url`.
- ripgrep's `grep-matcher` sets `autotests = false`, so the hidden test of `d4-match-overlaps` lives in
  `crates/searcher/tests/`.
- `fill` puts the absolute venv, cargo and hook paths into commands at run time; no path is in a file.
