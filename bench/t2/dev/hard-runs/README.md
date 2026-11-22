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
