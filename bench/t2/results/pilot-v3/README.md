# T2 pilot, preregistration v3: 12 runs on requests

2026-11-21. 5 agents per run (3 Claude Code, 2 Codex), the block and the server of preregistration v3.
Per run: `summary.txt` and the run files. No significance test: 3 repetitions per cell.

| Cell | Tasks on main | Green suites | Conflicts | Interventions | Warned / reacted / false alarms | Median wall s | Tokens |
|---|---|---|---|---|---|---|---|
| A disjoint | 15/15 | 3/3 | 2 | 0 | | 47.65 | 4 196 083 |
| B disjoint | 15/15 | 3/3 | 0 | 0 | 0 / 0 / 0 | 62.79 | 4 927 160 |
| A colliding | 14/15 | 2/3 | 10 | 4 | | 108.68 | 7 595 450 |
| B colliding | 15/15 | 3/3 | 1 | 0 | 10 / 3 / 3 | 73.65 | 5 975 196 |

Tokens of B against A: 1.17 on the disjoint sets, 0.79 on the colliding sets, 0.93 over all 12 runs.

## Stop thresholds

1. **Holds.** 10 agents were warned before their first land and 3 of them applied again after the
   warning. 3 is below half of 10, and the check needs 6 warned agents. By section 8 the experiment stops
   here: ripgrep and astro do not run.
2. Does not hold: B tokens are 0.93 x the A tokens, and B has 30 tasks on main against 29.
3. Does not hold: `user_config_unchanged` is true in all 12 runs, and no paid call happened.

## Reading of the result

- No task was lost in arm B. In the pilot of version 2 the colliding set of B lost 2 of 15. The colliding
  set of requests is not a blind test for the new point of the block (preregistration, section 13).
- The token overhead on the disjoint sets fell from 1.39 to 1.17. Median wall time on the disjoint sets is
  1.32 x arm A, above the 1.10 of the success threshold.
- Stop threshold 1 counts an agent as reacting only when it applies again after the warning. In these 3
  runs the 7 warned agents that did not apply again landed with 1 conflict in total, and all 15 tasks are
  on main with green suites. The threshold was fixed before the runs, so it stands for version 3. Whether
  it measures the right thing is a question for a next version, not for this report.
