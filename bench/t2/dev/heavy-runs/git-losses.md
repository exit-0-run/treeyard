# What git loses in the heavy runs

A count from the stored data of the 25 git runs (arm A: 12, arm A2: 13) described in
[README.md](README.md). No new runs. Numbers come from [`git_cost.py`](git_cost.py) (conflict work,
from the agent transcripts) and from the per-commit test matrix of the audit (which commit of main
broke which task). The purpose is a list of the situations in which git loses, as the base for a
harder task set.

## Conflict work

"Conflict work" is the hand-back invocation in arm A, and everything after the first `t2-integrate`
that reported a conflict in arm A2. Cost is the Claude list price, so the Claude-only series (10 of 10
agents priced) is the clean one.

| Arm | Series | Runs | Conflicts per run | Agents with conflict work | Claude USD on conflicts | Share of run cost | Share of tool calls |
|---|---|---|---|---|---|---|---|
| A | Claude-only | 6 | 6.7 | 6.7 of 10 | 0.59 | 22 % | 25 % |
| A | mixed | 6 | 6.5 | 6.5 of 10 | 0.33 (6 Claude agents) | 20 % | 22 % |
| A2 | Claude-only | 6 | 9.5 | 5.8 of 10 | 0.91 | 32 % | 26 % |
| A2 | mixed | 7 | 9.0 | 6.1 of 10 | 0.30 (6 Claude agents) | 20 % | 24 % |

- Arm A, Claude-only: the end phase (merges and hand-backs, one after the other) takes 129 to 158 s
  of a wall time of 204 to 234 s, about two thirds of the run.
- The agents that get conflict work are always the same: h3, h5, h7, h8 (all edit the prompt code in
  `termui.py` or the option code in `core.py`), h9 and h10 (help output), h6 in 8 of 12 runs. The
  files are `src/click/core.py` and `src/click/termui.py` in every run, then `docs/` and
  `src/click/__init__.py`.
- Every conflict was resolved by the agent: 0 interventions, 0 changes left out.
- This work does not make git dearer than TreeYard native mode in total (same cost per run, see
  README.md). It is a cost git pays at the end and TreeYard pays along the way.

## Work of another agent deleted

Four runs of 25. In each, task h1 (a refactor that moves the prompt text into a new place and must
keep the function `_build_prompt`) passed on main, and a later merge removed what it needs.

| Run | Agents | Who removed it | When |
|---|---|---|---|
| A r3 | mixed | h8 (Codex), hand-back | 8th of 10 merges |
| A r12 | Claude-only | h3 (Claude), hand-back | 3rd of 10 merges |
| A2 r3 | mixed | h8 (Codex), second conflict | last to integrate |
| A2 r33 | mixed | one of h3, h5, h7, h8 (not traced; the run's repository is gone) | after h1 integrated |

The shape is the same every time: the refactor is on main; a feature agent whose base still has the
old function gets a conflict in that function; it reads "main removed this" and deletes its side of
the hunk together with the part the refactor kept. Its own tests and the suite pass, because nothing
in them calls the kept function. Three of the four are in mixed runs; in the 12 Claude-only git runs
it happened once.

## Tasks that pass alone and not together

| Run | Agents | Failed | Cause |
|---|---|---|---|
| A r3, A r4 | mixed | i1, i7 | h7 or h8 resolved its conflict by keeping its feature in the old function; the feature never reached the class h1 introduced. |
| A2 r22 | Claude-only | i6 | h6 calls a formatter argument that h10's formatter does not take. (Also seen in B r33, BN r13, BN r43.) |

## The list

1. **A refactor that moves code which several feature tasks edit.** It produces the deleted work
   (4 runs) and the feature left behind in the old place (2 runs). One refactor of this kind (h1, three
   or four feature tasks on its function) gave a defect in 5 of 25 runs. The other refactor (h2, one
   method per help item) lost nothing in any run; a likely reason is that the features on it add items and do not edit the
   moved body (not checked).
2. **Late merges.** The agent that deletes is one whose merge comes after the refactor and after one
   or more other features in the same function.
3. **A kept name with no caller.** The loss is silent when nothing in the suite or in the agent's own
   test uses what the other task must keep.
4. **Two features across one interface** (i6): no arm catches it, so it cannot separate the arms.
5. **Conflict work**: one fifth to one third of the cost, two thirds of the wall time in arm A. Equal
   total cost, so it separates the arms on time only.

In Claude-only runs, the model the next step uses, git had an integration defect in 2 of 12 runs and
delivered 107 of 108 tasks without h7. A set on which git fails the rule "under 85 % of tasks or a
defect in 2 of 3 runs" needs several times more of points 1 to 3 than `heavy` has: more refactors of
the first kind, more feature tasks on each moved function, and kept names that only hidden tests use.
