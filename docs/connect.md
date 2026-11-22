# Connect: attach TreeYard to the agents you already use

`treeyard connect` attaches TreeYard to one git repository and to the agent tools on this machine:
Claude Code, Codex and OpenCode. Each agent then edits through TreeYard and learns that it is in
another agent's way while it works, not at merge time. `treeyard disconnect` undoes all of it.

Every block tagged `sh` on this page runs as is: `sh scripts/check-quickstart.sh docs/connect.md`
runs them in a new empty directory and stops at the first error. You need `git`, `python3` and a
`treeyard` binary on `PATH` ([install.md](install.md)).

## What connect does

1. It checks that the directory is in a git repository. With no TreeYard data yet, it imports the
   checked-out branch (as `treeyard init --from .` does).
2. It starts `treeyard serve` on `127.0.0.1` on a free port and records the pid and the port.
3. It writes one MCP entry per harness, in project scope only: `.mcp.json` (Claude Code),
   `.codex/config.toml` (Codex), `opencode.json` (OpenCode). Each entry runs `treeyard mcp` with
   `TREEYARD_URL`. It never edits `~/.claude.json`, `~/.claude/`, `~/.codex/` or `~/.config/opencode`.
4. It adds one marked block to `AGENTS.md` and `CLAUDE.md`: open a change with an intent, edit
   through `apply`, fix an overlap or a conflict at once, run the tests in the directory that
   `checkout` writes for the change ([mcp.md](mcp.md#test-a-change-checkout)), finish with `land`.
   The block also says that the files on disk do not change at land. Real agents followed an earlier
   version (tests on a hand-made copy) with no other hint: [trials/t1/README.md](trials/t1/README.md).
5. It turns the mirror to `origin` on only with `--mirror`, or after a yes at the prompt in a terminal.

The state lives outside the working tree, in `.git/treeyard/`: `data/`, `connect.json` (pid, port,
one undo record per touched file), `backup/` (the bytes of each file before connect) and `serve.log`.
Connect adds the files that it creates to `.git/info/exclude`, so `git status` stays clean.

| Flag | Effect |
|---|---|
| `--harness claude,codex,opencode` or `all` | Harnesses to attach. Default: the ones whose binary is on `PATH`. |
| `--agents N` | Agent names `<harness>-1` to `<harness>-N`. Each MCP bridge takes the first free name. |
| `--port P` | Port on `127.0.0.1`. Default: a free port, then the same port on the next connect. |
| `--mirror`, `--no-mirror` | Push landed changes to `origin` every 30 s, or never ask. |
| (no mode flag) | Native mode, the default since 0.2.0: the MCP entries run `treeyard mcp --tools native` (`start`, `sync`, `land`, `change`, `diff`) and the block is the native one (below). Each agent edits and tests a directory of its own with its normal tools ([mcp.md](mcp.md#native-mode-start-edit-with-your-own-tools-sync-land)). `--native` is still accepted and changes nothing. |
| `--api` | API mode (the default before 0.2.0): `treeyard mcp --tools agent`, the 12 operations a coding agent needs plus `checkout`, and the API block ([mcp.md](mcp.md#tool-sets)). A `connect` without `--api` switches back to native mode. |
| `--all-tools` | API mode with every operation listed. Refused with `--native`. |
| `--land-test CMD`, `--land-test-timeout S`, `--no-land-test` | The land test: `land` lands only a result that passed `CMD` on the change rebased onto the main it lands on ([mcp.md](mcp.md#the-land-test)). Stored as `[coordination] land_test` in the server's `treeyard.toml` (the server restarts to read it). |
| `--land-queue CMD`, `--land-queue-setup CMD`, `--land-batch N`, `--no-land-queue` | The land queue instead of the land test: the server tests batches of up to `N` changes (default 8) with `CMD` in its own test slots, after `--land-queue-setup` prepared each slot once; `land` waits for the verdict ([mcp.md](mcp.md#the-land-queue)). Stored as `[landing] mode = "queue"`, `test`, `test_setup`, `batch`; it replaces a land test, and `--land-test` replaces it. Refused together with `--land-test`. `--no-land-queue` and `disconnect` remove it. |

A second `connect` with the same flags prints `already connected` and writes nothing. When the
server stopped, connect starts it again on the recorded port.

## 1. A repository

```sh
mkdir my-repo
git -C my-repo init -q -b main
printf 'hello\n' > my-repo/greeting.txt
printf '# My project\n' > my-repo/AGENTS.md
git -C my-repo add greeting.txt AGENTS.md
git -C my-repo -c user.name=me -c user.email=me@example.com commit -q -m "first commit"
cd my-repo
```

## 2. Connect

```sh
treeyard connect --harness all --agents 2
git status --short
python3 -c 'import json; s=json.load(open(".mcp.json"))["mcpServers"]["treeyard"]; print(s["args"][:3])'
tail -n 3 AGENTS.md
```

`git status --short` prints nothing for the new files, and the changed `AGENTS.md` shows as ` M`.
Each harness asks once before it uses a project server:

- Claude Code asks you to approve the `treeyard` server of `.mcp.json` (or use `/mcp`).
- Codex reads `.codex/config.toml` only when you trust the project. `codex exec` has no one to ask,
  so it refuses each MCP call unless the server entry approves its tools. Connect writes
  `default_tools_approval_mode = "approve"` into its own `[mcp_servers.treeyard]` entry (key and value
  as codex-cli 0.167.0 reads them), and only there; `connect status` shows it as
  `tools approval approve`. Other servers in the file keep their settings.
- OpenCode reads `opencode.json` at the project root. `opencode mcp list` shows the entry.

## 3. Two agents on one file

Here two agents work through `treeyard call`, so this page needs no model. A real agent makes the
same calls through the MCP tools.

```sh
URL=$(treeyard connect status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["url"])')
A=$(treeyard call --url "$URL" open_change '{"agent": "claude-1", "intent": "shout the greeting", "paths": ["greeting.txt"]}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
B=$(treeyard call --url "$URL" open_change '{"agent": "codex-1", "intent": "translate the greeting", "paths": ["greeting.txt"]}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
treeyard call --url "$URL" apply "{\"change\": \"$A\", \"edits\": [{\"path\": \"greeting.txt\", \"replace\": {\"old\": \"hello\", \"new\": \"HELLO\"}}]}" > /dev/null
treeyard call --url "$URL" apply "{\"change\": \"$B\", \"edits\": [{\"path\": \"greeting.txt\", \"replace\": {\"old\": \"hello\", \"new\": \"hola\"}}]}" | python3 -c 'import json,sys; print("collision:", json.load(sys.stdin)["collisions"][0]["kind"])'
treeyard connect status
```

`connect status` prints the server address, the attached harnesses and their agent names, the open
changes (agent, intent, paths), and one warning per overlap or conflict. Codex-1 sees the conflict
in its `apply` reply, before either agent lands.

## 4. Land and take the work back into the checkout

```sh
treeyard call --url "$URL" land "{\"change\": \"$A\"}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])'
treeyard call --url "$URL" abandon "{\"change\": \"$B\"}" > /dev/null
git pull -q "$URL/git" main
cat greeting.txt
```

## 5. Disconnect

`disconnect` restores every file that connect touched to its bytes from before connect, deletes the
files that connect created, and stops the server and the mirror. It keeps `.git/treeyard/data`
(open changes included) unless you pass `--purge`; a land queue that connect set is taken out of its
`treeyard.toml`. A second `disconnect` prints `not connected`.

```sh
treeyard disconnect --purge
git status --short
cat AGENTS.md
treeyard disconnect
```

When you edited a connect file after connect, `disconnect` removes only the TreeYard entry or block
from it and prints a warning.

## Native mode

`treeyard connect` (native mode) writes the native block (`NATIVE_INSTRUCTIONS` in
`src/connect.rs`, the same text as `bench/t2/block-bn.md`): call `start` first; work only in its `dir`
with your normal tools and run the tests there; `sync` when you want to know whether you collide
(optional); on `undoes` keep the other agent's part; `land` when the tests pass; on a conflict fix the
marked files in `dir` and `land` again. The files in the repository directory do not change. The
API block and entries (`--api`) stay byte for byte what they were. `connect status` prints the mode
(`mode: native` or `mode: api`), the land test and the land queue.

## Agent names

With `--agents N`, the MCP entry runs `treeyard mcp --agent-pool <h>-1,...,<h>-N --pool-dir
.git/treeyard/agents` (an absolute path). Each bridge takes the first free name with a lock file
and frees it when it exits. A lock of a dead process is free again. When every name is taken, the
bridge uses `<h>-1-<pid>`.

## Limits

- One repository and one machine. Connect writes absolute paths and the port into the configs, so
  they are excluded from git and are not for a team to share.
- Agents edit through the API (MCP tools). They run the tests in the directory that the bridge's
  `checkout` tool writes for their change ([mcp.md](mcp.md#test-a-change-checkout)). For an agent that
  edits files on disk, give it a workspace directory: [workspaces.md](workspaces.md).
- Connect does not change the trust settings of a harness (user-level settings). The one approval it
  sets is for the tools of its own Codex server entry, in the project file that it writes and removes.

Tests: `tests/connect.rs` (init and idempotency, the three configs, the instruction block, byte
identity after disconnect, status with two agents on one file, the mirror flag, the Codex tool approval,
the land test and the land queue flags).
