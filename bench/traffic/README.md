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

## Results (2026-11-22 and 23)

**In one sentence: TreeYard's advantage in throughput exists without a test before entry and from
hundreds of agents on; with a test before entry on the rebased tree (git, or TreeYard's land test)
there is none, both stop at about 3600 / test length changes an hour. Only a queue that tests a
batch of changes in one run goes above that, and it loses most of that gain when changes fail the
test.**

Batch queues are not special to TreeYard: plain git has them as services around the repository
(GitHub merge queue, GitLab merge trains, Bors, Mergify, Zuul). TreeYard has one built in, and on
the `queue` branch an agent's `land` waits in it for the verdict. This page does not compare
TreeYard's queue with any of those.

Raw results: [`results/`](results/).

### No test before entry: git without automatic housekeeping against TreeYard

One go, the arms alternating level by level ([`results/fair.json`](results/fair.json)).

| Agents | Arm | Per hour | Median wait | 95th percentile | Retries | Entered | Ready, not entered |
|---|---|---|---|---|---|---|---|
| 50 | git | 5 980 | 0.19 s | 0.41 s | 98 | 299 | 0 |
| 50 | TreeYard | 6 260 | 0.04 s | 0.09 s | 39 | 313 | 0 |
| 75 | git | 9 040 | 0.21 s | 0.42 s | 237 | 452 | 0 |
| 75 | TreeYard | 9 560 | 0.02 s | 0.04 s | 73 | 478 | 0 |
| 100 | git | 7 200 | 0.29 s | 124 s | 5 456 | 409 | 35 |
| 100 | TreeYard | 12 260 | 0.03 s | 0.08 s | 96 | 613 | 0 |
| 200 | git | 1 200 | 85 s | 259 s | 7 916 | 92 | 160 |
| 200 | TreeYard | 23 880 | 0.06 s | 0.21 s | 266 | 1 194 | 0 |

- Git clogs between 75 and 100 agents: at 100 the retries are 13 times the entries and 35 ready
  changes never enter. Its best rate is about 9 000 changes an hour, the traffic of about 320
  agents that each deliver 28 changes an hour.
- TreeYard holds to 200 agents, the highest level run (median wait 0.06 s, no change left out).
- Up to 75 agents the two arms deliver the same; git's wait is under half a second.
- No change that entered is missing on main in any arm.

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
- A `sleep` never fails, so this table is the queue with no failing change. The next table adds them.

### The queue with failing changes

50 agents, changes entering main per hour within the window, and in brackets the ratio to
3600 / test length ([`results/queue-bad.json`](results/queue-bad.json)).

| Test | 3600 / test | Batch | 0 % bad | 5 % | 10 % | 20 % | 30 % |
|---|---|---|---|---|---|---|---|
| 30 s | 120 | 4 | 408 (3.4) | 408 (3.4) | 312 (2.6) | 228 (1.9) | 144 (1.2) |
| 30 s | 120 | 8 | 804 (6.7) | 408 (3.4) | 336 (2.8) | 336 (2.8) | 96 (0.8) |
| 30 s | 120 | 16 | 1 584 (13.2) | 600 (5.0) | 24 (0.2) | 36 (0.3) | 0 (0.0) |
| 60 s | 60 | 4 | 188 (3.1) | 202 (3.4) | 135 (2.2) | 142 (2.4) | 90 (1.5) |
| 60 s | 60 | 8 | 382 (6.4) | 255 (4.2) | **210 (3.5)** | 68 (1.1) | 60 (1.0) |
| 60 s | 60 | 16 | 742 (12.4) | 248 (4.1) | 15 (0.2) | 15 (0.2) | 0 (0.0) |
| 120 s | 30 | 4 | 94 (3.1) | 94 (3.1) | 71 (2.4) | 49 (1.6) | 45 (1.5) |
| 120 s | 30 | 8 | 191 (6.4) | 128 (4.3) | 105 (3.5) | 34 (1.1) | 30 (1.0) |
| 120 s | 30 | 16 | 364 (12.1) | 128 (4.3) | 64 (2.1) | 8 (0.3) | 0 (0.0) |

**The gain of the queue depends on the share of changes that fail the test, and at our agents
that share was 0 of 43.** Counted from stored runs with real agents: 30 changes in three runs with
the land test and the 13 states of main of a git run with 13 agents, the repository's suite each
time; 0 failed, and the 95 % interval reaches up to 8.2 % (the count and its limits are in
`bench/t2/dev/hard-runs/README.md` on the `connect` branch). So the measured share sits in the
columns 0 % to 5 %, and the data cannot rule out the 10 % column. The agents run the tests
themselves before they hand a change in; a suite that takes long enough to need a queue may be run
less often by them, and then more changes would fail at entry.

**Every number in the columns with bad changes of the table above comes from the first minutes of
an overloaded queue.** The longer run under it reads the two cells that matter.

- **The rule is met in this one run:** at 10 % bad changes and a 60 s test, batch 8 gives 210
  changes an hour, 3.5 times 3600 / test length (the rule asks for 2 times). That number is 28
  changes in 4 batches, so it is rough.
- **Failing changes cost most of the gain.** Batch 8 falls from 6.4 times with no bad change to
  3.4 to 4.3 at 5 %, 2.8 to 3.5 at 10 % and about 1 at 20 to 30 % (one cell of six gives 2.8), the
  level of a test per change.
- **A large batch breaks down.** Batch 16 gives 12 to 13 times with no bad change and next to
  nothing from 10 % bad on (one cell of nine gives 2.1): nearly every batch of 16 then holds a bad
  change, and while the queue looks for it nothing enters. The landing times show gaps of four
  test lengths after a failing batch of 8.
- **Not built: a batch size that follows the share of failing changes.** The table says a fixed
  batch is wrong at one end or the other: 16 is best with no bad change and useless from 10 % on,
  4 holds up best at 20 to 30 %. An idea for the queue: start large, halve the batch after a
  failing one, grow it again after clean ones. Noted here, not built and not measured.
- **No bad change reached main** in any of the 45 cells (297 bad changes made, 103 rejected by the
  queue, the others still waiting at the end).
- **No rejected change entered after its fix** within any cell: the fixed changes were still
  waiting at the end. The wait of a bad change is therefore not measured.
- The queue was overloaded in every cell with bad changes: 19 to 50 ready changes had not entered
  at the end, and the median wait of the good changes that entered was 1 to 12 test lengths.

### The queue with failing changes, longer window (2026-11-23)

Test 60 s, batch 8, 50 agents, window of 30 test lengths, three seeds per cell, a rejected change
fixed after 120 s ([`results/queue-steady.json`](results/queue-steady.json)).

| Bad share | Per hour: mean (lowest to highest of 3) | Ratio to 3600 / test | Fixed change: rejection to entry, median per run | Rejected changes that entered in the run | Good change: median wait | Bad on main |
|---|---|---|---|---|---|---|
| 5 % | 217 (212 to 226) | 3.6 (3.5 to 3.8) | 9, 13 and 15 min | 10 of 18 | 8 to 10 min | 0 |
| 10 % | 158 (146 to 166) | 2.6 (2.4 to 2.8) | 12, 12 and 13 min | 15 of 31 | 10 to 13 min | 0 |

- **The rule is met:** at 10 % bad changes the queue gives 158 changes an hour, 2.6 times
  3600 / test length, in each of the three runs above the 120 the rule asks for. The short window
  had said 3.5 times; the longer one is lower.
- With no bad change the same queue gave 6.4 times (short window), so 5 % bad costs about four
  tenths of the rate and 10 % about six tenths.
- A rejected change that is fixed enters 9 to 15 minutes after its rejection (2 of them are the
  fix), and it was rejected about 9 to 11 minutes after it was ready. About half of the rejected
  changes had not entered when the run ended.
- The rate moves within a run: by ten-minute thirds it runs from 96 to 336 an hour.
- The queue is overloaded throughout (the agents made 1.4 to 1.7 times the changes that entered,
  while most of them stood waiting; 24 to 42 ready changes had not entered at the end), so the waits are those of a full queue, not of a queue that
  keeps up.

### Limits

- The queue with failing changes: one run per cell, and a window of 8 to 10 test lengths, about as
  long as the search after one or two failing batches. The cells with bad changes show the first
  minutes of an overloaded queue, not a steady state; a single cell can be far off (see 20 % bad,
  batch 8, at 30 s against 60 s). The three test lengths use the same random draws, so they are
  close to one schedule at three speeds, not three samples. The fix of a rejected change is
  instant; a real agent takes minutes.
- Git as it comes, with automatic housekeeping, ran in earlier series, not alternating with
  TreeYard and under other load on the machine. At 50, 75 and 100 agents it gave 5 960, 5 120 and
  2 860 changes an hour, with 1 683 fetch errors at 75 agents, some naming "possible repository
  corruption on the remote side"; with housekeeping off those errors were 0. In the very first
  series 15 of 100 scripted git agents stopped on a fetch error; the script then made a failed git
  command a retry. These numbers are in `results/series1.json`, `series3.json` and
  `series4-nogc.json` and are not the comparison of this page.
- One machine plays the shared repository or server and every agent, and other work ran on it during
  parts of series 1, 2 and 1b (load average is in the result files). The clog point of git in series
  1 depends on this machine's cores; the pile-up of retries does not.
- Step 1b was cut by a restart of the machine after 58 of 84 levels; the other 26 ran afterwards on
  a quieter machine.
- Changes are 2-line inserts that always merge; real conflicts need an agent and are not in this test.
- In the land-test arm with tests of 30 s and more, the bridge process of 1 to 7 agents per level
  ended in the middle of a run. Cause, found afterwards: the script itself killed them. It waited
  only 60 s after the deadline for its agents and then closed every bridge, and a land with a 30 s
  test can run three rounds, longer than that. The kills came after the deadline, when nothing more
  is counted, so the rates and waits above stand; the script now waits long enough. (Looking for
  this showed a real fault that is fixed on the `queue` branch: a land test kept running after its
  bridge was killed.)
- 28 changes an hour per real agent comes from short tasks (about 2 minutes each); agents on long
  tasks deliver fewer, so the same ceilings serve more of them.

## Fair comparison without a test (settings written before the run)

The comparison for this page: git **without** automatic housekeeping (`--no-gc`) and TreeYard,
alternating level by level (git, then TreeYard, at 50, 75, 100 and 200 agents), same settings as
series 1, in one go on the same machine so both arms meet the same load. The numbers of git with
housekeeping stay in the limits only.

## The queue with failing changes (settings written before the run)

Arm `treeyard_queue` only, binary 0.2.0. A share of the changes is bad (`--bad`): the queue's test
is `sleep T; ! grep -rq 'def _bad_' src`, so a tree that holds a bad change fails. An agent whose
change comes back `rejected` fixes it at once (renames the function) and lands again.

- Bad share 0, 5, 10, 20, 30 %; test 30, 60, 120 s; batch 4, 8, 16: 45 cells. Server default for
  `parallel` (test runs side by side after a failing batch).
- 50 agents in every cell, the same pace and changes as before, so the agents offer more than the
  queue can take in most cells (at most 2 000, 1 200 and 670 changes an hour for the three tests).
- A cell runs 300, 480 and 960 s for the three tests, then twice the test plus 60 s. The three test
  lengths run at the same time (the test is a `sleep`).
- Reported per cell: changes entering main per hour (the ceiling when the queue is overloaded), the
  ratio to 3600 / test length, median wait of good and of bad changes, changes ready and not
  entered, bad changes that reached main (must be 0).
- **Rule: the queue makes sense when at 10 % bad changes and a 60 s test it gives at least 2 times
  3600 / test length, that is 120 changes an hour.** Read for batch 8 (the default); the other
  batches are reported beside it.
- The count starts with an empty queue, so the first test length of every cell lands nothing; the
  rates are a little under the steady state.

## The queue with failing changes, longer window (settings written before the run)

The cells above show the first minutes of an overloaded queue. This repeat reads the steady state.

- Arm `treeyard_queue`, binary 0.2.0, 50 agents, test 60 s, batch 8, bad share 5 % and 10 %.
- Window 1 800 s (30 test lengths), then 180 s. Three independent random seeds per cell (11, 12,
  13): six runs, one after the other, nothing else measured on the machine.
- An agent whose change is rejected takes 120 s to fix it (`--fix-s 120`), then lands again.
- Reported per cell: mean and range over the three seeds of changes entering main per hour and of
  the ratio to 3600 / test length; the time from rejection to entry of a fixed change (median per
  run); bad changes on main (must be 0).
- The rule is the same: at 10 % bad at least 120 changes an hour.
