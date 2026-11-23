# Real runs of the `long` set (networkx, 9 tasks)

The set is described in [../README.md](../README.md). Question: does plain git lose time or work when
agents work long, not a minute? Development runs. Everything below the rules was written after them.

## Rules, written before the first paid run

- All agents Claude Code, the CLI default model (`claude-sonnet-5-5` in the earlier runs), cap 1.5 USD
  per agent. Limit for steps 1 and 2 together: 30 USD of Claude list price. Nothing else is measured
  on the machine while a step runs. Estimate before the start: one solo pass 5 to 10 USD, one run
  with 9 agents 7 to 12 USD (not measured; the cap bounds each at 13.5).
- **Step 1, solo**: each task once, one agent alone on the clean pin, one after the other
  (`t2.py solo --parallel 1`, stop at 12 USD). A task that does not pass is fixed or removed. When
  the median wall time of the tasks is under 10 minutes the set is not long: stop and report, no step 2.
- **Step 2, git only, 1 run each**: arm A (work apart, merge at the end, conflicts handed back) and
  arm A3 (agents merge main often and integrate themselves). Measured: tasks on main, integration
  tests, integration defects by `../hard-runs/losses.py`, wall time, cost, and the share of time and
  of cost that went into work after a conflict (`conflict_work` in the run file).
  - Conflict work under 10 % of time and of cost in both runs and no loss: stop. An early signal has
    nothing to save here.
  - Conflict work 20 % or more of time or of cost in a run, or a lost task, or an integration defect:
    there is a field for a comparison; plan step 3.
  - Between: a second run of each way if it fits the limit.
- Step 3 (git, git with a conflict signal, TreeYard native, alternating) is not run without a new yes.

## Step 1, solo (2026-11-23)

One agent alone per task, one after the other, nothing else measured on the machine. Result files:
[`solo/`](solo/).

| Task | Hidden test | Wall time | Tool calls | Claude USD |
|---|---|---|---|---|
| l1 quoted names | pass | 4.7 min | 12 | 0.41 |
| l2 graph header | fail (1 test function) | 2.8 min | 5 | 0.31 |
| l3 line errors | fail (1 test function) | 4.8 min | 16 | 0.33 |
| l4 output format | pass | 3.1 min | 16 | 0.43 |
| l5 file bytes | pass | 3.3 min | 15 | 0.43 |
| l6 compression | pass | 3.4 min | 15 | 0.41 |
| l7 line parser | pass | 2.1 min | 5 | 0.17 |
| l8 line writer | pass | 2.1 min | 11 | 0.19 |
| l9 text file | pass | 12.9 min | 24 | 0.36 |

7 of 9 hidden tests pass, total 3.03 USD, the repository's suite green after every task. **Median
wall time 3.3 minutes (2.1 to 12.9), under the 10 minutes of the rule: the set is not long. Stop;
step 2 is not run.**

- The tasks were sized by the length of the reference change (305 to 435 lines) and an estimate of
  15 minutes each. The agent wrote them in 2 to 5 minutes; only l9 took longer. Lines of change do
  not make an agent work long.
- l2 and l3 each failed one function of the hidden test. Whether the agent or the test is at fault
  was not examined, because the set stops here.
- One run per task, one model; wall time on a subscription varies with the service's load.
