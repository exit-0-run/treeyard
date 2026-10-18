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
