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
   through `apply`, fix an overlap or a conflict at once, finish with `land`.
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
- Codex reads `.codex/config.toml` only when you trust the project.
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
(open changes included) unless you pass `--purge`. A second `disconnect` prints `not connected`.

```sh
treeyard disconnect --purge
git status --short
cat AGENTS.md
treeyard disconnect
```

When you edited a connect file after connect, `disconnect` removes only the TreeYard entry or block
from it and prints a warning.

## Agent names

With `--agents N`, the MCP entry runs `treeyard mcp --agent-pool <h>-1,...,<h>-N --pool-dir
.git/treeyard/agents` (an absolute path). Each bridge takes the first free name with a lock file
and frees it when it exits. A lock of a dead process is free again. When every name is taken, the
bridge uses `<h>-1-<pid>`.

## Limits

- One repository and one machine. Connect writes absolute paths and the port into the configs, so
  they are excluded from git and are not for a team to share.
- Agents edit through the API (MCP tools). For an agent that builds and runs tests, give it a
  workspace directory: [workspaces.md](workspaces.md), and the Claude Code hooks in [mcp.md](mcp.md).
- Connect does not change trust or approval settings of a harness: these are user-level settings.

Tests: `tests/connect.rs` (init and idempotency, the three configs, the instruction block, byte
identity after disconnect, status with two agents on one file, the mirror flag).
