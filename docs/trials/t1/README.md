# T1: real agents follow the connect instruction block

This trial checks one claim: a real agent uses the TreeYard tools when it reads only the block that
`treeyard connect` writes. The task prompt does not name TreeYard.

## Result

3 of 3 harnesses completed the task through `open_change`, `apply` and `land`. The server audit log
shows the sequence, and the deciding test is green on a clone of main. This is true with block v1
and with block v2. No harness was blocked by spend or by a login.

| Harness | Version | Model | Runs | Passed runs |
|---|---|---|---|---|
| Claude Code | claude 2.1.330 | default of the subscription (`claude-sonnet-5-5` in the init event) | claude-1 (v1), claude-2 (v2) | 2 of 2 |
| Codex | codex-cli 0.167.0 | default of the existing login | codex-1, codex-2 (v1), codex-3 (v2) | 2 of 3 |
| OpenCode | opencode 2.2.6 | `opencode/deepseek-v4.1-flash-free` (free, no login) | opencode-1 to opencode-3 (v1), opencode-4 (v2) | 2 of 4 |

Per run ([summary.txt](summary.txt), output of `python3 docs/trials/t1/run.py summary`):

| run | block | model | exit | killed | wall s | audit ops | via MCP tools | test on main | text.py on disk | pass |
|---|---|---|---|---|---|---|---|---|---|---|
| claude-1 | v1 | default | 0 | no | 14.6 | open_change:ok -> apply:ok -> land:landed | yes | green | unchanged | yes |
| claude-2 | v2 | default | 0 | no | 11.4 | open_change:ok -> apply:ok -> land:landed | yes | green | unchanged | yes |
| codex-1 | v1 | default | 0 | no | 20.7 | (none) | no | red | unchanged | no |
| codex-2 | v1 | default | 0 | no | 28.7 | open_change:ok -> apply:ok -> land:landed | yes | green | unchanged | yes |
| codex-3 | v2 | default | 0 | no | 25.6 | open_change:ok -> apply:ok -> land:landed | yes | green | unchanged | yes |
| opencode-1 | v1 | opencode/glm-5.3-flash-free | 1 | no | 0.8 | (none) | no | red | unchanged | no |
| opencode-2 | v1 | opencode/deepseek-v4.1-flash-free | 0 | no | 100.0 | open_change:ok -> apply:ok -> land:landed | no | green | unchanged | no |
| opencode-3 | v1 | opencode/deepseek-v4.1-flash-free | 0 | no | 73.6 | open_change:ok -> apply:ok -> land:landed | yes | green | unchanged | yes |
| opencode-4 | v2 | opencode/deepseek-v4.1-flash-free | 0 | no | 66.6 | open_change:ok -> apply:ok -> land:landed | yes | green | unchanged | yes |

The 3 failed runs are setup errors of the driver or the provider, not agents that bypassed the tools:

- codex-1: `codex exec` refused each MCP call. The error in the transcript is
  `MCP tool call requires approval, but approval policy is never`. The agent tried `open_change`
  twice and then stopped. Fix in the driver: `-c mcp_servers.treeyard.default_tools_approval_mode="approve"`.
- opencode-1: the free model failed before any tool call:
  `{"type":"server_error","message":"Upstream request failed: Model is unavailable."}` (status 400).
  Fix: a different free model.
- opencode-2: OpenCode takes its project directory from `$PWD`, not from the process directory. The
  driver started it with the `PWD` of its own shell (the worktree), so OpenCode did not load the
  `opencode.json` of the trial repo. The agent found the server in the driver and used
  `treeyard call` from a shell. The audit log shows the full protocol, but not through MCP, so this
  run does not count. Fix in the driver: `PWD` = the trial repo.

No agent wrote `text.py` on disk in any run (column "text.py on disk"). [stub-3](stub-3/) is the
free check of the driver: a script makes the 3 calls, with no model.

## What the block changed (v1 to v2)

The tool order of each run is in [sequences.txt](sequences.txt) (`python3 docs/trials/t1/run.py sequences`).

- With v1, 3 of 3 passing agents looked for the landed fix in the checkout after `land`
  (`git pull`, or `python3 -m unittest` in the repo, which was still red). claude-1 and codex-2 ran
  the tests only after land. opencode-3 checked the fix logic in an inline script before land.
  After land it read `.git/treeyard/serve.log`, `ps` output and the trial driver.
- v2 adds step 4 (test on a copy before land) and step 6 (the files on disk do not change at land).
- With v2, 3 of 3 agents ran the tests in a temporary copy before land. claude-2 and codex-3 stopped
  after land. opencode-4 still ran `git log` and the tests in the checkout after land.
- The `open_change` tool now says that the MCP bridge sets the agent name. In codex-2 the agent sent
  `"agent": "codex"`, so the audit log shows `codex`. In codex-3 it shows `codex-1`. One run each
  does not prove that the description caused this.

Block texts and the outcome of each run: [blocks/v1.md](blocks/v1.md), [blocks/v2.md](blocks/v2.md) (final).

## How the trial runs

`run.py` (python3, standard library only) runs one harness on one new repository:

1. It makes `<scratch>/t1/runs/<harness>-<n>/repo`: `text.py` with a bug in `slugify()`,
   `test_text.py` (4 unittest cases) and one commit. The deciding test is `python3 -m unittest`.
2. It runs `treeyard connect --harness <h> --agents 1 --no-mirror` there. Connect picks a free
   loopback port. The driver stops when the port is 7420.
3. It runs the agent with this prompt and refuses a prompt that contains "treeyard":
   `Fix the bug in slugify in text.py so that `python3 -m unittest` passes. Finish the work completely.`
4. It saves `treeyard call audit` and `events`, clones `<url>/git` main, runs the test in the clone,
   and records `git status` of the trial repo.
5. It runs `treeyard disconnect --purge`, removes host paths and `ps` output from the files, checks
   them for names and token patterns, and copies them to `docs/trials/t1/<harness>-<n>/`.

A run passes when the audit rows show `open_change`, then at least 1 `apply`, then `land` with status
`landed`. The transcript must show these calls as MCP tool calls. The test must be green on main, and
`text.py` on disk must not hold an edit.

Commands (from the worktree root, after `make build`):

```
T1_FORBIDDEN=<comma list of words> python3 docs/trials/t1/run.py run claude 2 <scratch> --block v2
T1_FORBIDDEN=<...> python3 docs/trials/t1/run.py run codex 3 <scratch> --block v2
T1_FORBIDDEN=<...> python3 docs/trials/t1/run.py run opencode 4 <scratch> --block v2 --model opencode/deepseek-v4.1-flash-free
python3 docs/trials/t1/run.py summary
T1_SCRATCH=<scratch> python3 docs/trials/t1/run.py snapshot docs/trials/t1/userconfig-after.txt
```

The agent commands (the driver writes the exact list into `result.json` of each run):

- Claude Code: `claude -p <prompt> --mcp-config .mcp.json --strict-mcp-config --max-turns 30
  --output-format stream-json --verbose --permission-mode acceptEdits --allowedTools
  "mcp__treeyard__*,Bash,Read,Edit,Write"`. `CLAUDE_CONFIG_DIR` points to the run dir. The driver
  reads the OAuth access token of the installed Claude Code from the Keychain into the
  `CLAUDE_CODE_OAUTH_TOKEN` variable of the child process only.
- Codex: `codex exec --json --ephemeral --skip-git-repo-check --ignore-user-config -s workspace-write
  -C <repo> -c approval_policy="never"` and `-c mcp_servers.treeyard.*` overrides from the
  `.codex/config.toml` that connect wrote. Auth stays in the existing Codex home.
- OpenCode: `opencode run --standalone --format json --auto --model <free model> <prompt>`, with
  `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME` in the run dir.
  `--standalone` is necessary: the background service of the user already holds port 49374.

## Limits on the real-agent runs

- Each run had a wall-clock limit of 600 s. On timeout the driver kills the process group. No run hit it.
- Turn limits: Claude `--max-turns 30`; Codex: the driver kills it after 40 tool-call events; OpenCode:
  agent `build` with `steps` = 30 in the run-dir config. No run hit a turn limit.
- The driver runs one agent at a time. 9 real-agent runs in total (limit 12). Spend: 0 USD (existing
  subscription and login, free OpenCode models). Claude reports `total_cost_usd` 0.079 for claude-1:
  this is a list-price figure, not a charge.

## User configuration

[userconfig-before.txt](userconfig-before.txt) and [userconfig-after.txt](userconfig-after.txt) hold
the sha256 of `~/.claude.json`, `~/.claude/settings*.json`, `~/.claude/CLAUDE.md`,
`~/.codex/config.toml` and each file in `~/.config/opencode/`. Each run dir has the same pair.

- `~/.claude/settings.json` and `~/.config/opencode/service.json`: equal before and after.
- `~/.claude.json`: the hash changes, because other sessions on this machine write it. Its top-level
  keys are equal, it has 4 `projects` entries before and after, and 0 entries under the scratch dir.
- `~/.codex/config.toml`: **changed**. Each `codex exec` run added one entry
  `[projects."<scratch>/t1/runs/codex-<n>/repo"] trust_level = "trusted"` (codex-1, codex-2, codex-3).
  The driver did not write it; `--ignore-user-config` and `--ephemeral` do not stop it. When the 3
  entries are removed in memory, the hash equals the hash from before
  ([userconfig-codex-proof.txt](userconfig-codex-proof.txt)). So these 3 entries are the only change.
  The file is not edited back here, because this trial must not edit user configuration. The owner
  can remove the 3 entries.

## Not covered

- One agent per repository, so no overlap or conflict happened. 3 to 5 agents at once is the next trial.
- Only API mode (no workspace directory). The agents ran the tests in temporary copies that they built from `read`.
- A trial repo with 3 files. For a large repository a copy from `read` is not practical: use workspace mode.
