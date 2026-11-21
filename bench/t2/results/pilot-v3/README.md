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

## Corrections after an audit (2026-11-21)

An audit of the kit after this pilot found faults in the runner. They change how the tables above read.
The run files are unchanged.

- **Tokens of Claude Code are counted about 1.7 times too high.** The runner adds the usage of every
  stream event, and one message has several events. Counted once per message, the B / A token ratios
  are: pilot v2 disjoint 1.29, colliding 1.38, all 1.34; pilot v3 disjoint 1.31, colliding 0.86, all 1.02.
  The token overhead on the disjoint sets did not fall between the two pilots.
- **Every intervention of arm A comes from the Codex sandbox, not from git.** A git worktree keeps its
  metadata outside the directory Codex may write, so a Codex agent cannot finish the merge of a
  hand-back: 7 of 7 failed, against 0 of 15 for Claude Code. The runner then takes one side of the
  conflict, and that removed the lines of another task in arm A.
- **The two arms did not do the same work.** Arm A agents added tests and a changelog entry in most runs.
  Arm B agents did not, and few of them ran the repository tests before `land`. 15 of the 21 merge
  conflicts of arm A in the two pilots are only in the changelog or in a test file.
- **`warned_before_land` and `reacted` count other things than section 5 of the preregistration says.**
  3 of the 10 warned agents of pilot v3 never got the warning in a tool reply, and a rebase does not
  count as a reaction.

With these faults the pilots do not support a conclusion for either arm. The fixes come before any run
on ripgrep or astro.
