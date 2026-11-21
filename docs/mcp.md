# Connect agents: Claude Code, Codex, OpenCode

Every agent reaches the server in one of three ways. All three use the one operation table
([api.md](api.md)).

One command does the MCP setup below for one repository, in project scope, and undoes it again:
`treeyard connect` and `treeyard disconnect` ([connect.md](connect.md)).

| Way | When | How |
|---|---|---|
| MCP tools | the agent edits through the API, no checkout | `treeyard mcp --url URL --agent NAME` (stdio), or `URL/mcp` (HTTP) |
| A workspace directory | the agent must build or run tests | `treeyard workspace create NAME` + `treeyard daemon` |
| CLI | scripts and orchestrators | `treeyard call <op> '<json>'` |

`treeyard mcp` reads `TREEYARD_URL` (default `http://127.0.0.1:7420`) and `TREEYARD_TOKEN`. With a
token, the server names the agent from the token. Without one, `--agent` (or `TREEYARD_AGENT`)
fills `agent` in `open_change` and `revert`. The server sends short instructions in its MCP
`initialize` reply: read with `read`/`list`/`grep`, then `open_change`, `apply`, `land`, and how to
handle a conflict.

The command lines below follow each tool's own docs, opened on 2026-10-17:
[Claude Code MCP](https://code.claude.com/docs/en/mcp), [Claude Code hooks](https://code.claude.com/docs/en/hooks),
[Codex MCP](https://developers.openai.com/codex/mcp), [OpenCode MCP servers](https://opencode.ai/docs/mcp-servers/).
The MCP bridge itself is tested with raw JSON-RPC on stdio (`tests/git_bridge.rs`, and step 7 of
[quickstart.md](quickstart.md)). No agent ran in these tests: the harness that built this has no
budget for model calls.

## Claude Code

Add the server for one project (local scope) or for all projects (`--scope user`):

```text
claude mcp add --env TREEYARD_URL=http://127.0.0.1:7420 --transport stdio treeyard -- treeyard mcp --agent claude-1
claude mcp get treeyard
```

To share it with a team, put it in `.mcp.json` at the project root:

```json
{
  "mcpServers": {
    "treeyard": {
      "command": "treeyard",
      "args": ["mcp", "--agent", "claude"],
      "env": { "TREEYARD_URL": "http://127.0.0.1:7420" }
    }
  }
}
```

### Claude Code worktrees become TreeYard workspaces

Claude Code runs a `WorktreeCreate` hook instead of `git worktree add` for `claude --worktree`,
subagents with `isolation: "worktree"`, workflows and background sessions. The hook prints the
directory as its last stdout line. `examples/claude-code/` has both hooks:

- `worktree-create.sh` reads `name` from stdin, runs `treeyard workspace create NAME --git`
  (set `TREEYARD_SETUP="cargo build -q"` to build once), and prints the workspace path.
- `worktree-remove.sh` reads `worktree_path`, syncs the last edits to the server, then deletes the
  directory. The change stays open on the server, so a removed worktree loses nothing that synced.

Add them to `.claude/settings.json` (absolute paths; see `examples/claude-code/settings.json`), and
run `treeyard daemon &` on the machine so edits sync while the session works:

```json
{
  "hooks": {
    "WorktreeCreate": [{ "hooks": [{ "type": "command", "command": "sh /path/to/examples/claude-code/worktree-create.sh" }] }],
    "WorktreeRemove": [{ "hooks": [{ "type": "command", "command": "sh /path/to/examples/claude-code/worktree-remove.sh" }] }]
  }
}
```

Test: `tests/claude_hooks.rs` feeds both scripts the hook JSON from the Claude Code docs, edits a
file in the workspace, removes it, and reads the edit back from the open change.

Limits that the Claude Code docs state: the hook does not run when Claude types `git worktree add`
in Bash; `.worktreeinclude` is not processed (copy `.env` files in your setup command). When a
subagent finishes, Claude Code keeps the worktree and does not run the remove hook: land or remove
those workspaces yourself (`treeyard workspace list`).

### One checkpoint per turn

`examples/claude-code/checkpoint.sh` is a `UserPromptSubmit` hook. Before each prompt it finds the
workspace of the session (the `.treeyard/env` above `cwd`), syncs it, and calls `checkpoint` with the
label `turn of session <id>`. A turn without edits adds no checkpoint. It prints nothing on stdout (that
would become model context) and never blocks the prompt. Undo a turn with
`treeyard workspace restore NAME --to N`, or ask the agent to call `restore_change`; `checkpoints` lists
them with the paths each turn changed.

```json
{ "hooks": { "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "sh /path/to/examples/claude-code/checkpoint.sh" }] }] } }
```

Test: `tests/claude_hooks.rs` (two turns give two checkpoints; outside a workspace the hook does nothing).

## Codex

```text
codex mcp add treeyard --env TREEYARD_URL=http://127.0.0.1:7420 -- treeyard mcp --agent codex-1
codex mcp list
```

Or in `~/.codex/config.toml`:

```toml
[mcp_servers.treeyard]
command = "treeyard"
args = ["mcp", "--agent", "codex-1"]
default_tools_approval_mode = "approve"   # else `codex exec` refuses every tool call

[mcp_servers.treeyard.env]
TREEYARD_URL = "http://127.0.0.1:7420"
```

Codex has no worktree hook (openai/codex issue #26648 asks for a custom worktree backend). For a
directory, create a workspace first and start Codex in it:
`cd "$(treeyard workspace path fix-1)" && codex`.

## OpenCode

In `opencode.json` (project) or the global OpenCode config:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "treeyard": {
      "type": "local",
      "command": ["treeyard", "mcp", "--agent", "opencode-1"],
      "enabled": true,
      "environment": { "TREEYARD_URL": "http://127.0.0.1:7420" }
    }
  }
}
```

Check with `opencode mcp list`. For a directory, start OpenCode inside a workspace as for Codex.

## Test a change: `checkout`

An agent that edits through `apply` has no files on disk, so it cannot run the tests. The stdio
bridge `treeyard mcp` has one tool of its own for that, `checkout` (it is not in the server's
operation table: only the bridge runs on the agent's machine):

```text
checkout {"change": "ch_..."}   ->   {"dir": "/tmp/.../treeyard-checkouts-501/ch_...", "files": 120, "written": 120, ...}
```

- `dir` holds exactly the tree of the change as the server has it: its base plus its edits, exec
  bits and symlinks, without the files it deleted. The agent runs the repository's own tests there
  (`cd DIR && make test`). No file is retyped.
- The default directory is one per change under the system temp directory (`$TMPDIR`, else `/tmp`).
  Both Claude Code and Codex under `sandbox_mode = "workspace-write"` let shell commands write there,
  and nothing is written into the repository checkout or a workspace that another agent or the daemon
  syncs. `into` names another directory; it must be new, empty or an earlier checkout
  (`.treeyard/checkout.json` marks one), so a checkout never overwrites a working tree.
- After the first `checkout`, every `apply`, `resolve`, `rebase`, `restore_change` and `land` of that
  change through the same bridge rewrites the directory, and the reply has a `checkout` field. Only
  files that differ are written, so build output (`target/`, `__pycache__/`) and mtimes stay.
- Edits made in the directory never reach the change: the next rewrite puts the change's content back
  and lists the paths in `reverted`. Files that the change does not have stay; those that no
  `.gitignore` excludes are listed in `extra` (they would not land).
- `main_moved: true`: work landed after the change's base, so the directory lacks it. `rebase`, then
  `checkout` again, to test what `land` will put on main. After a `land`, the directory holds the
  landed tree.

`treeyard checkout CHANGE [--into DIR]` does the same from a script. Test: `tests/checkout.rs` drives the
bridge over stdio against a real server: the fixture's test script fails in the checkout of the
repository and passes in the directory of the change, local edits are reverted, a landing by another
agent sets `main_moved`, `rebase` brings it in, and after `land` every file of main equals the file in
the directory. A second case runs the bridge inside a `workspace-hook` workspace: the workspace and its
change stay untouched, and the `apply` reply still has the `conflict` collision.

Why not the other two ways. (a) The bridge could keep the agent's working directory in step with its
change. But that directory is often shared: after `treeyard connect` every agent of the repository runs
in the one checkout, and in a `workspace-hook` workspace the daemon would sync the written files into the
workspace's own change, a second change of the same agent that then collides with the first. (b) Agents
could edit on disk and land with `treeyard workspace land`. Then the collision warnings would come only
at sync time, not in each `apply` reply. A separate directory per change has neither problem and costs
one call.

## Many agents, one machine or many

- One server serves all agents on all machines. Each agent needs only the URL (and a token when
  the server is not on loopback: [access.md](access.md)).
- Give each agent its own `--agent` name, or its own token. Blame, log and audit show that name.
- Agents learn about each other through `apply` replies (`collisions`, and `undoes` when an edit
  removes text another agent wrote; `resolve` and `land` say it too) and the `events` tool.
  Tell your agents to call `events` with `wait_ms` before a long task, or let the orchestrator
  watch `GET /api/events`.

## MCP over HTTP: `POST /mcp`

The server also speaks MCP itself, over Streamable HTTP at `/mcp`, with the same tools and the same
token rules as `/api`. Use it when the client cannot start a process: a remote agent, or an MCP
gateway that serves a company's own MCP servers to its members.

```text
claude mcp add --transport http treeyard https://HOST/mcp --header "Authorization: Bearer $TREEYARD_TOKEN"
```

- Stateless: every POST is answered with JSON; there is no session id and no server-sent stream
  (`GET` and `DELETE` are `405`). Notifications alone get `202`. A batch gets an array.
- No or unknown token: `401` with `WWW-Authenticate: Bearer`, already on `initialize`, so a
  gateway's connection check sees at once that each member needs a key.
- `Authorization` may hold `Bearer <token>`, HTTP Basic, or the bare token: a gateway that
  forwards a member's key verbatim needs no prefix.
- A non-admin token acts as its own agent, as on `/api`. Its write globs hold for `apply`.

Tested in `tests/mcp_http.rs`. An MCP gateway that stores one key per member and sends it in
`Authorization` reached it end to end on 2026-10-21: each member's agent acted as that member,
with no bridge in between.

## A raw MCP session

```text
$ printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"status","arguments":{}}}' \
  | treeyard mcp --agent a1
```

The first reply has `serverInfo.name` `treeyard` and the instructions. The second has the status
JSON as one text item.
