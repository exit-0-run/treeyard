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

## Check of series 1: git without automatic housekeeping. Settings written before the run

At 75 agents git reported 1 683 fetch errors, some naming "possible repository corruption on the
remote side". To see whether git's automatic housekeeping under load made the clog, git alone is
run again at 50, 75 and 100 agents with `gc.auto=0`, `maintenance.auto=false` and
`receive.autogc=false` in the shared repository and in every clone (`--no-gc`), nothing else
changed, with nothing else of this test running. Reported beside the first result: fetch errors and
the clog point.

## Results (2026-11-22)

**In one sentence: TreeYard's advantage in throughput exists without a test before entry and from
hundreds of agents on; with a test before entry on the rebased tree (git, or TreeYard's land test)
there is none, both stop at about 3600 / test length changes an hour. Only the queue mode, which
tests a batch of changes in one run, goes above that.**

Raw results: [`results/`](results/).

### Series 1, no test before entry

| Agents | Arm | Per hour | Median wait | 95th percentile | Retries | Entered | Ready, not entered |
|---|---|---|---|---|---|---|---|
| 10 | git | 1 280 | 0.19 s | 0.35 s | 3 | 64 | 0 |
| 10 | TreeYard | 1 280 | 0.04 s | 0.09 s | 2 | 64 | 0 |
| 20 | git | 2 500 | 0.17 s | 0.35 s | 10 | 125 | 0 |
| 20 | TreeYard | 2 500 | 0.04 s | 0.07 s | 7 | 125 | 0 |
| 50 | git | 5 960 | 0.37 s | 1.2 s | 219 | 298 | 0 |
| 50 | TreeYard | 6 040 | 0.04 s | 0.06 s | 47 | 302 | 0 |
| 100 | git | 2 500 | 20 s | 205 s | 6 161 | 174 | 26 |
| 100 | TreeYard | 12 420 | 0.06 s | 0.21 s | 107 | 622 | 0 |
| 200 | git | 780 | 115 s | 242 s | 5 770 | 63 | 164 |
| 200 | TreeYard | 24 080 | 0.08 s | 0.34 s | 252 | 1 205 | 0 |

Git clogs between 50 and 100 agents (retries 35 times the entries at 100); TreeYard holds to 200
(wait twice its wait at 10). No change that entered is missing on main in any arm or series. In that
first run 15 of the 100 scripted git agents stopped on a fetch error; the script then made a failed
git command a retry, and git was run again:

| Agents | Git as it comes: per hour / median / retries / fetch errors | Git without automatic housekeeping: the same |
|---|---|---|
| 50 | 5 960 / 0.24 s / 139 / 0 | 6 000 / 0.16 s / 104 / 0 |
| 75 | 5 120 / 0.53 s / 5 606 / 1 683 | 8 200 / 0.50 s / 1 539 / 0 |
| 100 | 2 860 / 3.7 s / 5 999 / 0 | 4 200 / 0.88 s / 6 485 / 0 |

With housekeeping off the fetch errors are gone (1 683 to 0) and git does better at 75 agents, but
the clog point by the rule is the same: retries pass entries at 75 agents (3.5 times) and at 100
agents 48 ready changes never enter. The second column ran on a quieter machine (load average 31
against 126 at 75 agents), so the two columns differ in more than the setting. The clog is about
6 000 to 8 000 changes an hour, the traffic of 210 to 290 agents that each deliver 28 changes an hour.

### Series 2 and step 1b, a test before entry

| Test | 3600 / test | Arm | Ceiling per hour | Ceiling / (3600 / test) | Clog point (scripted agents) | Offered there per hour | In real agents (28 per hour each) |
|---|---|---|---|---|---|---|---|
| 2 s | 1800 | git | 1620 | 0.90 | 10 | 1125 | 40 |
| 2 s | 1800 | TreeYard, land test | 1660 | 0.92 | 10 | 1125 | 40 |
| 2 s | 1800 | TreeYard, queue (batch 8) | 5340 | 2.97 | none up to 50 | - | - |
| 10 s | 360 | git | 340 | 0.94 | 5 | 450 | 16 |
| 10 s | 360 | TreeYard, land test | 340 | 0.94 | 5 | 450 | 16 |
| 10 s | 360 | TreeYard, queue (batch 8) | 2580 | 7.17 | none up to 50 | - | - |
| 30 s | 120 | git | 100 | 0.83 | 5 | 300 | 11 |
| 30 s | 120 | TreeYard, land test | 100 | 0.83 | 5 | 300 | 11 |
| 30 s | 120 | TreeYard, queue (batch 8) | 660 | 5.50 | none up to 50 | - | - |
| 60 s | 60 | git | 50 | 0.83 | 5 | 200 | 7 |
| 60 s | 60 | TreeYard, land test | 50 | 0.83 | 5 | 200 | 7 |
| 60 s | 60 | TreeYard, queue (batch 8) | 330 | 5.50 | none up to 20 | - | - |
| 120 s | 30 | git | 25 | 0.83 | 5 | 120 | 4 |
| 120 s | 30 | TreeYard, land test | 25 | 0.83 | 5 | 120 | 4 |
| 120 s | 30 | TreeYard, queue (batch 8) | 165 | 5.50 | none up to 20 | - | - |

- Git and the land test have the same ceiling at every test length, 0.83 to 0.94 of 3600 / test
  length: one change enters per test run, because every entry of another change makes the others
  test again. They clog at the first level where agents offer more than that (between 5 and 10
  scripted agents for a 2 s test, between 2 and 5 for 10 s and longer). In agents that deliver 28
  changes an hour the ceiling is 58, 12, 3.6, 1.8 and 0.9 agents for tests of 2, 10, 30, 60 and 120 s.
- The queue (batch of 8, one test run per batch) did not clog at any level run. Its highest rate was
  3 to 7 times 3600 / test length; at the top levels of the 2 s and 10 s tests it was still limited
  by what the agents offered, at 30 s and longer it sat at 5.5 times (a batch of 8 per test run,
  less the gaps between runs). Its median wait is about twice the test length.
- A `sleep` never fails. A failing batch costs the queue more test runs (bisect); that cost is not
  measured here. The queue arm is not what `treeyard connect --land-test` sets up today.

### Limits

- One machine plays the shared repository or server and every agent, and other work ran on it during
  parts of series 1, 2 and 1b (load average is in the result files). The clog point of git in series
  1 depends on this machine's cores; the pile-up of retries does not.
- Step 1b was cut by a restart of the machine after 58 of 84 levels; the other 26 ran afterwards on
  a quieter machine.
- Changes are 2-line inserts that always merge; real conflicts need an agent and are not in this test.
- In the land-test arm with tests of 30 s and more, the bridge process of 1 to 7 agents per level
  exited in the middle of a run with nothing on stderr (seen again in a repeat). Not explained yet;
  the agent's changes after that are counted as not entered.
- 28 changes an hour per real agent comes from short tasks (about 2 minutes each); agents on long
  tasks deliver fewer, so the same ceilings serve more of them.
