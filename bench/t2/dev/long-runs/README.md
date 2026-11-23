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
