# Live agents: git worktrees vs TreeYard with real Claude Code agents

8 Claude Code agents (`claude -p`, model Sonnet) fix 8 real bugs from the history of
[more-itertools](https://github.com/more-itertools/more-itertools), all from the same base commit
`2fe1b2e` (2026-09-01). The bug reports are the problem parts of the real commit messages; the agents
never see the fixes. Two runs, same model, tasks and task text; only the way work reaches main differs:

- `git`: one `git worktree` per agent; `land.py` is a serial merge queue (rebase on master, run the tests,
  fast-forward).
- `treeyard`: one TreeYard workspace per agent, the daemon syncs edits, `treeyard workspace land` lands
  with optimistic landing (tests on main after the land, bisect and revert on a failure) and waits for
  the verdict.

`probes.py` holds 8 hidden behaviour checks, one per bug, run on the final main. Each fails on the base
commit and passes after the real fixes.

```sh
sh setup.sh
python3 demo.py git
python3 demo.py treeyard
```

Results go to `$DEMO_DIR/results/<arm>.json` (default `~/.cache/treeyard-live-agents`). The agents get
an allow-list of tools (file edits, `git`, `python`, `treeyard`, `cat`, `ls`, `grep`, ...), no MCP servers.

`DEMO_N=5` runs only the first 5 tasks and `DEMO_POOL=4` keeps at most 4 `claude -p` running at once.
`cloud.sh TAG` builds treeyard, runs both arms and copies the results to `bench/results/live-agents/TAG/`
(made for a Claude Code cloud session: it stops first if the `claude` CLI is missing or `claude -p` fails).

`DEMO_SET=wide` is 50 tasks on base `4aeb433` (`tasks_wide.py`): 10 pairs that each add two options to one
function, and 30 single tasks on 30 other functions. `probes_wide.py` has 60 hidden checks (one per task,
one per pair using both options). `wide_ref.patch` is a reference solution: with it all 60 checks pass and
so does the repo's suite; on the base none pass. Note that more-itertools' `.gitattributes` sets
`more_itertools/more.py merge=union`, so the git arm's rebase never stops on a conflict in `more.py` (it keeps
both sides, and the queue's tests catch a broken result); TreeYard does not read `.gitattributes` yet and
reports those conflicts.
