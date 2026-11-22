# Real runs of the `hard` set (click, 13 tasks)

The set is described in [../README.md](../README.md). Development runs, not a preregistered test.

## Solo trial

Each task done by one agent alone on the clean pin, 2 repetitions (`t2.py solo`, Claude Code,
`claude-sonnet-5-5`, same prompt and limits as an agent of arm A). Rule written before the first run:
a task that does not pass its hidden test 2 of 2 is fixed or removed before any run with 13 agents.

Result: 26 of 26 pass, every suite green. No task is fixed or removed. Cost 5.02 USD (limit 6):
0.10 to 0.32 USD and 22 to 81 s per task. Run files and the two summaries: [`solo/`](solo/).

| Group | Tasks | Passed | Mean USD | Mean wall s |
|---|---|---|---|---|
| Strong refactors (3 or more features edit the moved body) | h1, h2, h11, h14 | 8/8 | 0.15 | 40 |
| Weak refactors (2 features edit the moved body) | h12, h13 | 4/4 | 0.13 | 33 |
| Features | h3, h5, h6, h7, h8, h9, h10 | 14/14 | 0.24 | 63 |

So a task lost in a 13-agent run of this set is not lost because one agent cannot do it alone.
h7, which 19 of 47 runs of `heavy` lost through one sentence of its prompt, passes 2 of 2 with the
rewritten prompt.

## Git with 13 agents (arm A), 3 runs: rule written before the first run

Arm A (one worktree per agent, the runner merges at the end and hands conflicts back), 13 Claude Code
agents, repetitions 51 to 53, limit 11 USD of Claude list price for the three runs (no run starts when
the cost so far plus the dearest run so far would pass it). Not run at the same time as anything else
on the machine.

- The runs go one after the other, with a stop (added before the first run): when run 1 has 13 of
  13 tasks, 21 of 21 integration tests and no integration defect, runs 2 and 3 are not started and
  the conclusion is "the set is not a cheap field for comparisons". Reason: on another dense set
  (10 agents, 33 of 45 pairs conflicting, 3 refactors) git with the same model gave a clean run the
  same day, so a clean first run here makes two more unlikely to change the reading. With a loss or
  a defect in run 1, runs 2 and 3 follow and the rule below is read over the three.
- The set is a field for comparisons when git delivers under 85 % of the tasks over the 3 runs (33 or
  fewer of 39 hidden tests pass on the final main) or 2 or 3 runs have an integration defect (a task
  or an integration test lost for a reason that involves another agent).
- Otherwise the conclusion is "the set is not a cheap field for comparisons" and the quality line is
  closed. Three runs cannot show that git does not lose, and this page will not say so.
- Every loss gets its cause from the history of main, by [`losses.py`](losses.py): own miss, own
  merge, broken by another agent's merge (its code removed or changed), tasks that do not fit.
  Integration defect = broken by another agent, do not fit, or an integration test that follows a
  task lost that way. Results are given apart for the 4 strong refactors (h1, h2, h11, h14) and the 2
  weak ones (h12, h13), with wall time, cost and the number of conflicts handed back.
- A run cut by a usage limit or a kit crash is invalid, listed, and run again within the limit.

### Check of the loss classifier before the runs

`losses.py` was run on the stored histories of main of five `heavy` runs whose losses were traced by
hand ([`losses_check.py`](losses_check.py), data in `losses-check.json`): 7 of 8 losses get the same
cause. The one difference: in A r3 the hand reading of i1 is "the two tasks do not fit", the script
says "follows h1", because h1 itself is lost in that run and the script cannot look past a lost
partner. So "follows" can hide a second cause; such entries are read by hand. The histories of the
other five traced runs (A2 r22, A2 r33, BN r21, B r23, B r33) were deleted with their run
directories and could not be checked.

### Result of the git runs (2026-11-22)

| Run | Tasks | Integration tests | Integration defects | Conflicts handed back | Wall s | Claude USD | Cost of the hand-backs | Merge phase, hand-backs one after the other |
|---|---|---|---|---|---|---|---|---|
| r51 | 13/13 | 21/21 | 0 | 10 (all resolved, 0 interventions) | 379 | 3.79 | 1.08 (28 %) | 269 s (71 %) |

| Group | Tasks in r51 | Solo |
|---|---|---|
| Strong refactors (h1, h2, h11, h14) | 4/4 | 8/8 |
| Weak refactors (h12, h13) | 2/2 | 4/4 |
| Features | 7/7 | 14/14 |

`losses.py` finds no loss and no break at any state of main. By the stop rule runs 2 and 3 were not
started. **Conclusion: the set is not a cheap field for comparisons; the quality line is closed.**
One run does not show that git cannot lose here. What it does show: with 10 of 13 agents handed a
conflict, plain git with one model delivered everything. The merge phase took 269 of 379 s because
this arm hands conflicts back one after the other; that is a property of the arm, not of git. The
hand-backs cost 1.08 of 3.79 USD.
Run file and classifier output: [`git/`](git/).
