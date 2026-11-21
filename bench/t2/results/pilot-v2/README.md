# T2 pilot, preregistration v2: 12 runs on requests

Date: 2026-11-21. Agents per run: 3 Claude Code, 2 Codex. Kit commit: `add85c8`. One run at a time, in the
order of section 9. No run stopped. No limit killed an agent. `user_config_unchanged` is true in all 12
runs. The run files are in this directory. `summary.txt` and `metrics.json` come from
`t2.py collect`, and `budget.json` from `t2.py budget`. The agent transcripts are not here yet: they
hold paths of the host and need a pass that removes them.

## Sums per cell (3 runs each)

| Set | Arm | Tasks on main | Suite green | Conflicts at merge or land | Interventions | Warned | Reacted | False alarms | Lost lines | Median wall s | Tokens |
|---|---|---|---|---|---|---|---|---|---|---|---|
| disjoint | A | 15/15 | 3/3 | 1 | 0 | 0 | 0 | 0 | 0 | 65.89 | 4 293 295 |
| disjoint | B | 15/15 | 3/3 | 0 | 0 | 0 | 0 | 0 | 0 | 77.88 | 5 962 230 |
| colliding | A | 15/15 | 2/3 | 8 | 2 | 0 | 0 | 0 | 6 | 213.72 | 6 755 705 |
| colliding | B | 13/15 | 1/3 | 1 | 0 | 13 | 8 | 5 | 8 | 100.63 | 8 676 336 |

## Check against the preregistration

Stop threshold (section 8), checked after the pilot:

1. Agents ignore warnings: 8 of 13 warned agents reacted. This is not below half. The rule does not hold.
2. Token overhead above 20 % with no gain in correctness: arm B used 14 638 566 tokens and arm A
   11 049 000, a ratio of 1.32. The B sum of tasks on main is 28 and the A sum is 30. **The rule holds.**
3. User config or unpaid calls: no change, no paid call. The rule does not hold.

Rule 2 holds, so the experiment stops here. The runs on ripgrep and astro do not start.

Success threshold (section 7), on the pilot runs only, for the record:

1. Interventions in colliding sets: B 0, A 2. The condition holds.
2. Correctness: B 28 tasks and 4 green suites, A 30 tasks and 5 green suites. The condition fails.
3. Disjoint sets: median wall time ratio 1.18, token ratio 1.39. The condition fails.

## What the runs show

- In colliding sets arm B had 1 conflict against 8 and no intervention against 2. Its median wall time
  was less than half.
- Arm B lost task `c1-rel-lower` in 2 of 3 colliding runs. The run files show no land conflict for it,
  and 3 or 4 lines of agent 1 are not on final main. The cause is not known yet.
- The repository suite was red on final main in 2 of 3 colliding runs of arm B and in 1 of 3 of arm A.
- Without collisions arm B cost 39 % more tokens.

The limits of this pilot: one repository, 3 repetitions per cell, no significance test.
