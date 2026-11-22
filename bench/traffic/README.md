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
