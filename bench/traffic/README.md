# Traffic without a model

Does plain git clog when many agents bring changes into one main, and does TreeYard hold where git
does? [`traffic.py`](traffic.py) plays the agents with a script: no model, ready small changes at a
set pace, the same stream for git and for TreeYard native mode. Its header describes both arms.

## Settings, written before the first measured run

- Repository: click at `cf19179e8e2fd54c601378cec9e0d21bb12b6fb1`, as one commit.
- Agents: 10, 20, 50, 100, 200. Each level runs 180 s, then up to 120 s to finish what is ready.
- Pace: every agent makes one change after an exponential wait with mean 30 s on the main it took
  before the wait, so 10 agents offer about 1 200 changes an hour and 200 agents about 24 000.
- Change: one new 2-line function before a random top-level `def` or `class` of one file.
- Same files: half of the changes go to three files (`core.py`, `termui.py`, `types.py`), the other
  half to the 13 other modules of `src/click`. Two inserts at the same place conflict; both arms keep
  both functions.
- Series 1: nothing runs before a change enters. Series 2: a 2 s command must pass on the rebased
  tree first (git: before each push; TreeYard: the land test).
- Measures per level and arm: changes entering main per hour; wait from ready to on main (median,
  95th percentile); retries (git: a rejected push; TreeYard: a `land` reply that is not `landed`,
  and extra land-test rounds); ready changes not on main at the end; landed changes missing on main.
- **Clog point of git**: the first level where the median wait is over 5 times the median at 10
  agents, or retries are more than changes that entered.
- Reading: git does not clog up to 200 agents: stop, git is enough at the traffic of ordinary teams.
  Git clogs and TreeYard keeps its wait at that level (under 5 times its own wait at 10 agents, and
  retries under entries): that level is the setting for runs with real agents. Both clog: TreeYard
  has no advantage; say what blocks it.
- One machine plays the server, the shared repository and every agent, so the agents' own processes
  compete for its cores. The load average is recorded with every level.

## Step 1b: how long the test before entry takes. Settings written before the run

Series 2 (a 2 s test) clogged both arms alike. Step 1b repeats it with tests of 2, 10, 30, 60 and
120 s (`--test-s`, a `sleep` of that length in both arms), to find the ceiling per test length.

- Same repository, change, pace (mean 30 s before each change) and share of hot files (half).
- Agents: 1, 2, 5, 10, 20, 50 for tests of 2, 10 and 30 s; 1, 2, 5, 10, 20 for 60 and 120 s (a run
  at 50 agents would only repeat a clogged level for an hour).
- Length of a level: 180 s, or 6 test lengths when that is longer; then twice the test length plus
  60 s to finish what is ready.
- The five test lengths run at the same time on the one machine (the test is a `sleep`, so they
  wait far more than they compute). Load average is recorded.
- **Ceiling**: the highest number of changes entering main per hour over the levels, per arm and
  test length. Compared with 3600 / test length.
- **Clog point**: the first level where the median wait is over 5 times the median with 1 agent,
  or retries are more than changes that entered. In real agents: the changes per hour offered at
  that level, divided by 28 (a real agent of the heavy runs delivers about 28 changes an hour).
  The offered rate of a scripted agent is 3600 / (30 + test length) per hour at best.
- Reading: the ceiling follows 3600 / test length in both arms: a test before entry sets the
  throughput, not the tool. An arm with a ceiling over 1.5 times the other's at some test length:
  an advantage there, reported with its level.
- Third arm, added before the run: `treeyard_queue`. The server runs in landing mode `queue` with
  the same `sleep` as its test and the default batch of 8: `land` answers `queued`, the server
  tests up to 8 changes on top of each other in one run and lands the batch; the scripted agent
  polls `land_status` every 0.25 s. The `treeyard` arm stays the land test (one test per change on
  the rebased tree, run by the agent's bridge). Expected ceiling of the queue, if it holds: about
  8 x 3600 / test length. A `sleep` never fails, so this measures the queue without failing
  batches; the cost of a failing batch (bisect, more runs) is not in these numbers.
