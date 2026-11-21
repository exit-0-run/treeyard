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
  and 3 or 4 lines of agent 1 are not on final main. See "Analysis after the pilot".
- The repository suite was red on final main in 2 of 3 colliding runs of arm B and in 1 of 3 of arm A.
- Without collisions arm B cost 39 % more tokens.

The limits of this pilot: one repository, 3 repetitions per cell, no significance test.

## Analysis after the pilot

These notes come from the final trees and the agent transcripts of the 12 runs. They are not part of
the preregistered metrics.

### The lost task

No merge lost it. In runs r2 and r3 of the colliding set of arm B, the Codex agent with task
`c3-lower-keys` removed the lines of task `c1-rel-lower` in its own `apply`. Those lines were on main
when it opened its change. Its prompt says "Parameter values keep their case", and the lines of `c1`
make the value of `rel` lower case. The agent read the landed lines as a fault against its own task.

- r2: commit `d6a581a` (codex-1) on the final main removes `if key.lower() == "rel": value = value.lower()`.
- r3: commit `bbd08f5` (codex-2) removes the same lines.
- In arm A an agent never sees the work of the others, and git keeps both edits in a text merge. In
  arm B an agent sees landed work, and nothing told it that the lines belong to another agent's task.

### The token overhead

In the 3 disjoint runs of arm B, the MCP tools returned 491 720 bytes to the agents. 453 315 of them
(92 %) came from `read`, which returns a whole file: 29 calls, 11 456 bytes per call for Claude Code
and 19 023 for Codex. In arm A the agents read a line range of the file on disk. The token count sums
the input of every turn, so one large result counts again in each later turn.

Input tokens per turn, disjoint sets: Claude Code 22 120 in A and 28 784 in B; Codex 23 643 in A and
26 548 in B.

### What changed in the server after the pilot

Two changes answer these two findings. Neither was part of the pilot, and no run has measured them yet.

- `apply` now returns `undoes`: the lines an edit deletes or rewrites that another agent landed, with
  that agent's name and intent. The case of `c1` and `c3` is a test in `tests/coordination.rs`.
- `read` takes `start` and `end` and returns only those lines, with `total_lines`. On
  `src/requests/utils.py` (1 155 lines) the MCP result for lines 965 to 1010 is 1 513 bytes; the whole
  file is 38 053 bytes.

The instruction block of the pilot names neither of them. A new block and a new preregistration come
before the next measurement.

## Corrections after an audit (2026-11-21)

An audit of the kit after this pilot found faults in the runner. They change how the tables above read.
The run files are unchanged.

- **Tokens of Claude Code are counted about 1.7 times too high.** The runner adds the usage of every
  stream event, and one message has several events. Counted once per message, the B / A token ratios
  are: pilot v2 disjoint 1.29, colliding 1.38, all 1.34; pilot v3 disjoint 1.31, colliding 1.01, all 1.12 (a second fault: a resumed Codex session reported its
  thread total, so every Codex hand-back counted twice; `t2.py recount` gives the numbers).
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
