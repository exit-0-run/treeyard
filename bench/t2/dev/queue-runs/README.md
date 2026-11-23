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

## Results (2026-11-23)

Three runs, not four: after the first queue run the second one was held back to find out why the
queue tested one change at a time. Both git runs were done by then. The queue run below is
"queue v1". Result files: [`results/`](results/).

| Measure | Git r61 | Git r62 | Queue v1 r61 |
|---|---|---|---|
| Tasks on main | 12/13 | 13/13 | 13/13 |
| Integration tests | 20/21 | 20/21 | 21/21 |
| **Entry time**, first change ready to last entry | 21.0 min | 19.9 min | 18.5 min |
| **Test runs at entry** | 48 | 46 | 13 |
| Test runs at entry that failed | 1 | 0 | 1 (the change entered after its fix) |
| Repeats because main moved | 34 | 33 | - |
| Conflict answers to agents | 37 | 41 | 30 |
| Wall time of the run | 22.7 min | 21.4 min | 20.5 min |
| Claude USD | 4.54 | 4.54 | 5.56 |

**The gain does not show.** The rule asked for an entry time in arm Q of at most half of arm G's.
Queue v1 was 7 to 12 % faster, in these runs, with a suite made longer to 60 s. One queue run and
two git runs give no interval.

- **Test runs.** The queue ran the test 13 times, git 46 and 48 times. In git every change that
  enters before yours makes your tested result stale, so the test runs again on the new main; the
  queue tests a change once it is at the front.
- **The queue tested one change at a time.** 13 test runs for 13 entries and 1 rejection, so
  no batch was larger than two. The expectation of two batches was wrong for this set.
- **Where the time went in queue v1** (agent time from an agent's first `land` to its entry, summed
  over 13 agents, about 102 minutes, from the transcripts; the parts overlap a little):
  waiting in `land` that ended with the change on main 25 min; waiting in `land` that ended with a
  conflict answer 24 min (30 answers, 48 s each on average); the agents' own runs of the whole
  suite 47 min; editing after a conflict about 5 min. The agents ran the whole suite 73 times. In
  13 of the 14 `land` calls that reached the queue's test the agent had just run the whole suite
  on the same change itself.
- **Losses.** Git r61: task h11 passed on main and the merge of the h8 agent broke it; the suite
  does not cover it, so the test before entry let it through. Git r61 and r62: integration test
  i21 fails, two tasks that work alone and not together. Queue v1: nothing lost, and no state of
  main broke a task that passed before. Three runs do not show that the queue prevents such losses.
- The change the queue rejected had passed its agent's own run of the suite; it failed with the
  other agents' changes under it.
