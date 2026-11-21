# Shake-down of the reworked kit and server on click

2026-11-21, after the audit of the two pilots. 18 real runs on the development set (click), 5 agents each
(3 Claude Code, 2 Codex). These runs check that the kit works and show where the product stood after
each round of fixes. They are not a measurement: 1 to 3 runs per cell, and the server and the block of
arm B changed between the rounds.

Arms: A is plain git worktrees with the runner merging at the end, A2 is git worktrees where agents are
told about each other and integrate themselves, B is `treeyard connect`. All arms get the same preamble
(the change plus one regression test, one test command) and the same file-editing hand-back.

| Round | Run | Tasks on main | Conflicts | Tokens | Tool calls | Wall s |
|---|---|---|---|---|---|---|
| 1 | A colliding | 5/5 | 4 | 1.19 M | | 110 |
| 1 | A2 colliding | 5/5 | 0 | 1.68 M | | 88 |
| 1 | B colliding | 4/5 | 1 | 2.08 M | | 76 |
| 2 | A disjoint | 5/5 | 0 | 0.65 M | 34 | 31 |
| 2 | A2 disjoint | 5/5 | 0 | 0.70 M | | 34 |
| 2 | B disjoint | 5/5 | 0 | 1.61 M | 71 | 59 |
| 2 | B colliding r1 | 4/5 | 0 | 1.86 M | | 87 |
| 2 | B colliding r2 | 4/5 | 3 | 2.32 M | | 78 |
| 2 | B colliding r3 | 5/5 | 1 | 2.09 M | | 91 |
| 3 | A colliding | 5/5 | 4 | 1.20 M | 60 | 110 |
| 3 | A2 colliding | 5/5 | 0 | 1.67 M | 73 | 86 |
| 3 | B colliding r1 | 5/5 | 1 | 1.78 M | 74 | 90 |
| 3 | B colliding r2 | 5/5 | 0 | 1.86 M | 79 | 96 |
| 3 | B colliding r3 | 5/5 | 0 | 1.69 M | 73 | 88 |
| 3 | A disjoint | 5/5 | 0 | 0.68 M | 39 | 31 |
| 3 | A2 disjoint | 5/5 | 0 | 0.77 M | 42 | 34 |
| 3 | B disjoint r1 | 5/5 | 0 | 1.04 M | 47 | 42 |
| 3 | B disjoint r2 | 5/5 | 0 | 1.19 M | 52 | 46 |

Every run had a green suite (all tests and pinned tests), 0 interventions, and 5 of 5 agents ran the
test command to a pass.

What each round showed:

- Round 1: the new kit works with both CLIs in all three arms. With the Codex sandbox fault of the old
  hand-back gone, arm A has no intervention. Arm B lost a task: an agent removed another agent's landed
  feature while it resolved a conflict, after two `undoes` warnings. Fix: `land` holds such a change.
- Round 2: arm B lost a task in 2 of 3 colliding runs for the opposite reason. An agent merged its edit
  with another agent's edit of the same line, got an `undoes` warning for that line, and took its own
  task back. Arm B needed 71 tool calls where arm A needed 34. Fixes: `undoes` judges what the other
  agent added to a line; the first `apply` opens the change and writes the checkout.
- Round 3: arm B kept all tasks in 3 of 3 colliding runs. Its tokens are about 1.5 x arm A on both sets,
  and about the same as arm A2 on the colliding set (1.78 M against 1.67 M, one A2 run).

On these tasks plain git with one merge at the end also delivers every task. The runs show no gain of
arm B in tasks on main here, and a token cost above arm A.
