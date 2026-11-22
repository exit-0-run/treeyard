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
