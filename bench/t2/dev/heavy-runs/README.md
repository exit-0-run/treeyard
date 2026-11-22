# Real runs of the `heavy` set (click, 10 agents)

Development runs, not a preregistered test. The set is described in [../README.md](../README.md):
10 feature-sized tasks (two of them refactors of code that other tasks edit), 9 hidden integration tests.
Four arms:

- **A**: one git worktree per agent; the runner merges at the end and hands conflicts back to the agent.
- **A2**: git; agents are told about each other and integrate themselves with `t2-integrate`.
- **B**: TreeYard, every edit through the MCP tool `apply`.
- **BN**: TreeYard native mode: the agent edits a directory with its own tools and calls `start`, `sync`, `land`.

Agents: Claude Code (`claude-sonnet-5-5`) and Codex. Every run had 0 interventions and a green suite.

## What these runs can and cannot say

- `heavy` is a development set. Arm BN was written after the first A, A2 and B results on this same set.
- Samples are small: 2 to 4 runs per arm and series, in separate series that must not be pooled blindly.
- "Claude USD" is the list price the Claude CLI reports, for Claude agents only (6 of 10 agents in the
  mixed series, all 10 in the Claude-only series). Codex reports no cost.
- Tool calls are not a like-for-like measure of effort: in BN agents batch work into shell commands.
- Wall time is the parallel agent phase plus the end phase of the arm. Arm A is slowest by construction
  (its hand-backs run one after the other).
- Task `h7-byte-size` is lost in 19 of the 47 runs across all four arms for the same reason: the agent missed one
  sentence of its own prompt ("without bounds there is no range item"). It is noise that does not depend
  on the arm, so the tables give tasks with and without it.
- "Integration defect" means a task lost, or an integration test failed, on the final main for a reason
  that involves another agent: a merge resolution that removed another agent's code, or two tasks that
  do not fit together. A task whose own agent got it wrong before any integration is not counted there.

## Series

Run files: [`first/`](first/) (mixed harness, 6 Claude Code + 4 Codex, binary before the audit fixes),
[`claude/`](claude/) (10 Claude Code agents, same binary), [`final-claude/`](final-claude/) (10 Claude Code
agents, binary and kit after the audit fixes, arm order shuffled per repetition),
[`final-mixed/`](final-mixed/) (mixed harness again, after the audit fixes, arm order shuffled).

### First series, mixed harness (reps 1 to 4)

| Arm | Runs | Tasks on main | without h7 | Integration tests | Runs with an integration defect | Tokens M | Tool calls | Wall s | Claude USD |
|---|---|---|---|---|---|---|---|---|---|
| A | 4 | 37/40 | 35/36 | 33/36 | 2/4 | 5.37 | 172 | 328 | 1.62 |
| A2 | 4 | 37/40 | 35/36 | 36/36 | 1/4 | 5.24 | 168 | 170 | 1.50 |
| B | 3 | 28/30 | 27/27 | 27/27 | 0/3 | 7.03 | 205 | 243 | 1.99 |
| BN | 2 | 20/20 | 18/18 | 18/18 | 0/2 | 6.01 | 154 | 164 | 1.60 |

The Codex subscription ran out during this series. One B run was cut in the middle (not in the table,
not in `first/`) and 13 started runs stopped at their first Codex call; none of them is counted.

### Claude-only series (reps 11 to 13)

| Arm | Runs | Tasks on main | without h7 | Integration tests | Runs with an integration defect | Tokens M | Tool calls | Wall s | Claude USD |
|---|---|---|---|---|---|---|---|---|---|
| A | 3 | 29/30 | 26/27 | 27/27 | 1/3 | 5.48 | 174 | 220 | 2.69 |
| A2 | 3 | 28/30 | 27/27 | 27/27 | 0/3 | 6.27 | 209 | 125 | 2.79 |
| B | 3 | 29/30 | 27/27 | 27/27 | 0/3 | 8.15 | 228 | 156 | 3.83 |
| BN | 3 | 29/30 | 27/27 | 26/27 | 1/3 | 6.87 | 177 | 152 | 2.80 |

### Final Claude-only series, after the audit fixes (reps 21 to 23)

| Arm | Runs | Tasks on main | without h7 | Integration tests | Runs with an integration defect | Tokens M | Tool calls | Wall s | Claude USD |
|---|---|---|---|---|---|---|---|---|---|
| A | 3 | 30/30 | 27/27 | 27/27 | 0/3 | 5.56 | 175 | 225 | 2.68 |
| A2 | 3 | 28/30 | 27/27 | 26/27 | 1/3 | 6.77 | 209 | 125 | 2.85 |
| B | 3 | 27/30 | 26/27 | 27/27 | 0/3 | 8.20 | 234 | 189 | 3.95 |
| BN | 3 | 29/30 | 26/27 | 26/27 | 0/3 | 6.23 | 158 | 130 | 2.69 |

In this series BN r21 lost `h8` (and with it integration test i1) and B r23 lost `h9`: both failed at
the agent's own landing, before anything of another agent touched them (checked with
`t2.py health`), so they are not integration defects.

### Final mixed series, after the audit fixes (reps 31 to 33)

| Arm | Runs | Tasks on main | without h7 | Integration tests | Runs with an integration defect | Tokens M | Tool calls | Wall s | Claude USD |
|---|---|---|---|---|---|---|---|---|---|
| A | 2 | 20/20 | 18/18 | 18/18 | 0/2 | 5.87 | 189 | 334 | 1.80 |
| A2 | 3 | 27/30 | 26/27 | 27/27 | 1/3 | 5.46 | 183 | 184 | 1.57 |
| B | 3 | 28/30 | 27/27 | 26/27 | 1/3 | 7.32 | 202 | 217 | 2.14 |
| BN | 2 | 19/20 | 18/18 | 18/18 | 0/2 | 7.02 | 168 | 267 | 1.70 |

The Codex subscription ran out again in the third repetition: A r33 was cut (not counted, not in
`final-mixed/`) and BN r33 never started. BN r31 took 334 s because one Codex agent was slow; wall time
is the slowest agent of a run.

## Every loss that is not h7

| Run | What | Cause |
|---|---|---|
| A r3, A r12, A2 r3, A2 r33 | task h1 lost | Another agent, while resolving its conflict, deleted the function `_build_prompt` that h1 had to keep. h1 passed on main before that. |
| A r3 | integration i1 | h8's behaviour never reached the class h1 introduced; both tasks pass, the pair does not. |
| A r4 | integration i1, i7 | The same kind of mismatch: h7 and h8 never reached h1's class. |
| BN r13, A2 r22, B r33 | integration i6 | h6 calls `write_dl(first_col_width=...)`, h10's formatter does not take it. No tool in any arm tests main after a merge. |
| BN r21 | task h8, integration i1 | The agent's own h8 was wrong at its own landing. |
| B r23 | task h9 | The agent's own h9 was wrong at its own landing. |

In the TreeYard arms the same h1 situation (an agent about to remove `_build_prompt` or the lines of
another task) produced a hold on `land`: 15 holds in the 6 B runs and 23 in the 5 BN runs of the first two
series. Agents put the other part back in most cases and overrode the hold with `undo` + a reason in the
rest (5 times in B, 7 in BN). No TreeYard run lost h1.

## What survives

1. **Native mode removes the cost of TreeYard.** BN costs the same as the git arms within the noise
   (Claude-only: 2.80 and 2.69 USD per run against 2.69/2.68 for A and 2.79/2.85 for A2; mixed after the
   fixes: 1.70 against 1.80 and 1.57) and about 20 to 32 % less than B (3.83, 3.95, 2.14). An exact permutation test on the first two series gives p = 1/120 for
   BN below B. BN reads more input tokens than A; the cost is equal because most of them are cache reads.
2. **Wall time:** A is the slowest arm in every repetition. A2, B and BN are not told apart by this data.
3. **Tasks delivered:** no arm delivers more tasks than another once h7 is set aside.
4. **Integration defects:** seen in A (3 of 12 runs), A2 (3 of 13), B (1 of 12) and BN (1 of 10).
   With these samples no difference between arms is statistically supported (git arms 6/25 against
   TreeYard arms 2/22, Fisher two-sided p = 0.25). Four of the six git defects are one mechanism: a
   conflict resolution that deletes another task's code. That is what the TreeYard hold exists for, and
   it got through in none of the 22 TreeYard runs (4/25 against 0/22, p = 0.11): an observation that
   fits the design, not a measured effect. The two TreeYard defects are of the kind no arm catches.
5. **What no arm catches:** two tasks that each pass but do not fit together (i1, i6, i7). `land`,
   `t2-integrate` and the runner's merge all put work on main without running the tests of the result.

## Audit

An independent read-only audit recomputed every count of the first two series from the transcripts
(250 agents: tool calls, tokens and cost match the run files, no double counting), rebuilt all 250
prompts (hashes match; preamble, limits, models and tools are the same in every arm) and traced every
loss through the history of main. Its findings on native mode were fixed before the final series:
private checkout directories, delete and mode conflicts that wait for a decision, a guard for a stale
directory, backup files that are no longer sent (a `sed -i` backup had landed on main in BN r13), edits
after a landed change that go into a follow-up change, one wording of the override rule. The kit now
records the binary of each run and has `t2.py health` (hidden and integration tests at every commit of
a run's main).
