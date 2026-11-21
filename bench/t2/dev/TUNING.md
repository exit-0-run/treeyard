# Tuning of the arm B block on the development set

12 real runs on click, 5 agents each (3 Claude Code, 2 Codex), 2026-11-21. Run files: `results/*real*.json`.
Blocks: `blocks/base.md` (the block of preregistration v2), `blocks/v2.md`, `blocks/v3.md`. The server had
`undoes` and the ranged `read` in every run. This is tuning, not a measurement: 1 or 2 runs per cell.

| Run | Tasks on main | Suite | Conflicts | Tokens | Wall s |
|---|---|---|---|---|---|
| A colliding r1 | 3/5 | red | 3 | 3.14 M | 160 |
| B colliding r1, base | 4/5 | green | 1 | 4.11 M | 134 |
| B colliding r2, base | 5/5 | green | 1 | 4.05 M | 117 |
| B colliding r1, v2 | 5/5 | green | 0 | 3.69 M | 95 |
| B colliding r1, v3 | 5/5 | red | 0 | 4.14 M | 211 |
| B colliding r2, v3 | 5/5 | green | 1 | 4.68 M | 184 |
| A disjoint r1 | 5/5 | green | 2 | 2.20 M | 94 |
| B disjoint r1, base | 5/5 | green | 0 | 2.27 M | 66 |
| B disjoint r2, base | 5/5 | green | 1 | 3.06 M | 69 |
| B disjoint r1, v2 | 5/5 | green | 1 | 3.51 M | 137 |
| B disjoint r1, v3 | 5/5 | green | 0 | 2.08 M | 62 |
| B disjoint r2, v3 | 5/5 | green | 0 | 2.99 M | 72 |

What the runs show, and what they do not:

- With the base block one colliding run lost task `c1-strip-spaces`, although `apply` returned `undoes` in
  5 replies of that run. With v2 and v3 no task was lost in 3 colliding runs. The warning alone did not
  stop the removal; the rule in the block is there for it.
- Agents read line ranges with every block: the tool description is enough for that.
- v2 said "do not read a whole file of more than 200 lines". One Codex agent then made 21 small reads and
  29 shell calls in the disjoint run, and the run cost more tokens than the base block. v3 asks for one
  search and one read that covers the function.
- Tokens of base and v3 are the same within the spread of 2 runs. These runs do not show a token gain of
  v3 over base.
- The red suite of `B colliding r1, v3` is `tests/test_imports.py`: the agent of `c3-did-you-mean`
  imported `difflib` at module level, and click has a test on its import list. It is not a merge fault.

v3 is the block of preregistration v3 and of `treeyard connect`.
