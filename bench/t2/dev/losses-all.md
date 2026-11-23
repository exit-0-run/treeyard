# Did one agent's merge break or delete another agent's work? Every real run so far

A table made after the fact (2026-11-23) from runs of different task sets, arms and agent mixes.
It is not a comparison that was planned, and the runs were not drawn for it.

"Broke or deleted" means: a task passed on main, and a later merge or conflict resolution of
another agent made it fail or removed what it needs. "Work alone, not together" means: both tasks
pass at the end and their integration test does not. A task its own agent got wrong is neither.

| Runs | Set | Arm | Runs | Broke or deleted another's work | Work alone, not together | How counted |
|---|---|---|---|---|---|---|
| Git, merge at the end | heavy | A | 12 | 2 (r3, r12) | 2 (r3, r4) | per-commit table of the audit; 5 runs of the set re-checked by script |
| Git, agents integrate | heavy | A2 | 13 | 2 (r3, r33) | 1 (r22) | the same; r33's history is gone |
| Git, merge at the end | hard | A | 1 | 0 | 0 | script on main's history |
| Git, a 60 s test for every change | hard | G | 2 | 1 (r61) | 2 (r61, r62) | script on main's history |
| **Git, all** | | | **28** | **5** | **5 runs** | |
| TreeYard, API mode | heavy | B | 12 | 0 | 1 (r33) | per-commit table of the audit |
| TreeYard, native mode | heavy | BN | 10 | 0 | 1 (r13) | per-commit table of the audit |
| TreeYard, native with land test | heavy | BN | 3 | 0 known, 1 not traced (r42) | 1 (r43) | final main only; the histories are gone |
| TreeYard, land queue v1 | hard | Q | 1 | 0 | 0 | every state of main (`t2.py health`) |
| **TreeYard, all** | | | **26** | **0 known, 1 not traced** | **3 runs** | |

- In the five git runs the shape is the same: a refactor is on main, a feature agent settles a
  conflict in the moved code and removes or bypasses what the refactor needs, and no test in the
  repository's suite notices. In r61 of arm G the test before entry ran and passed.
- Three of the four heavy cases are in runs with two kinds of agent; with one model it happened in
  2 of 16 git runs (heavy A r12, hard G r61).
- Run r42 of the land-test series lost a task and nobody traced why before its history was
  deleted, so the TreeYard count is "0 known".
- The sets, arms, binaries and prompts differ between rows. The table says where the loss was seen,
  not how much more often it happens with git.

Sources: [`heavy-runs/`](heavy-runs/README.md), [`hard-runs/`](hard-runs/README.md),
[`queue-runs/`](queue-runs/README.md).
