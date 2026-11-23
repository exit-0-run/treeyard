# The land queue with real agents (set `hard`, 13 agents)

Development runs. The rules below were written before the first paid run; everything under
"Results" was written after.

## Question

The traffic simulation ([`../../../traffic/README.md`](../../../traffic/README.md) on the `traffic`
branch) says: with a test of 60 s per change, git and a test before entry stop at about 50 changes
an hour, and a queue with batch 8 gives 2.6 to 6.4 times that. Does the gain show with real agents?

## Rules, written before the first paid run

- Set `hard` on click, 13 tasks, 13 agents, Claude Code with the CLI default model, the set's own
  limits (the same as in run r51 of arm A), same preamble and tools in both arms.
- **The test is the repository's suite (about 3 s) made longer to 60 s with a `sleep`**
  (`--test-pad-s 57`). It is the same command in both arms. An agent's own run of the whole suite
  is padded too; a run of single test files is not.
- Arm G: git, a test for every change. The kit's integrate command merges main, runs the test on
  the result and moves main only when it passes; when main moved meanwhile it starts again.
- Arm Q: TreeYard native mode with the land queue, batch 8, server default for `parallel`. The
  binary is built from the `queue` branch, which is not released.
- 2 runs per arm, one after the other, nothing else measured on the machine. Order by a seeded
  draw (seed 20261123): Q, G, then G, Q. Runs r61 and r62 of each arm.
- A run cut by a usage limit, a kit crash or a machine fault is invalid: moved aside, listed, run
  again. Nothing else removes a run. Limit 22 USD of Claude list price; no run starts when the
  spend so far plus the dearest run so far (5 USD before the first) passes it.
- **Main measure, entry time**: from the first change ready to enter (first integrate or `land`
  call of any agent) to the last entry on main. Beside it: tasks on main of 13, integration tests
  of 21, integration defects, changes the queue rejected and whether they entered after a fix,
  test runs failed at entry and repeats in arm G, test runs, wall time, cost.
- Expectation: arm G needs 13 test runs one after the other at best, 13 to 16 minutes; arm Q two
  batches when nothing fails, a few minutes. In stored runs 0 of 43 changes failed the suite at
  entry.
- **The gain shows** when in both pairs arm Q's entry time is at most half of arm G's, arm Q
  delivers in total no fewer tasks and integration tests than arm G less one, and no change that
  fails the test is on main. Otherwise it does not show, and the report says in which run and why.
- Two runs per arm give no interval. The result is a check that the simulation's finding holds
  with real agents at 13, never a size: no "N times faster" without "in these 2 runs, with a suite
  made longer to 60 s". If the entry time is shorter and the whole run is not, that is reported.
- Not checked before the start, because it needs a real agent: that the agent's tools wait long
  enough for an integrate or a `land` of several minutes. A run spoiled by that is a kit fault.

Free rehearsal with scripted agents (both arms, 60 s test): 13 of 13 tasks and 21 of 21 integration
tests in both; with one failing change the queue rejected it, the agent side saw it, and it entered
after the fix; arm G kept it out.
