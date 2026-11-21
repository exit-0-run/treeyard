# Orchestrators: workspace hooks

An orchestrator runs many coding agents, each in its own git worktree. This page tells how to attach
TreeYard to three of them. It has two parts:

1. `treeyard workspace-hook create|remove`: a command for any tool that runs a command when it makes
   and removes a workspace.
2. One recipe per orchestrator (Orca, Paradev, Agent Orchestrator). Each recipe names the hook point,
   the source file that we opened and read, its commit and the date we read it. We did not install or run
   these tools. A recipe is a config snippet, not a tested integration.

## `treeyard workspace-hook`

```sh
dir=$(treeyard workspace-hook create --name fix-login --intent "fix the login form")
cd "$dir"                 # a TreeYard workspace bound to one open change
# ... the agent works here ...
treeyard workspace-hook remove fix-login   # sends the last edits, deletes the directory
```

- **Inputs.** Only arguments and environment variables. Every flag has a variable:
  `--name` (`TREEYARD_HOOK_NAME`), `--from-dir` (`TREEYARD_HOOK_DIR`), `--agent` (`TREEYARD_AGENT`),
  `--intent` (`TREEYARD_INTENT`), `--setup` (`TREEYARD_SETUP`), `--url` (`TREEYARD_URL`), `--home`
  (`TREEYARD_HOME`). A token comes only from `TREEYARD_TOKEN`, so it never shows in a process list.
- **Output.** stdout gets one line: the absolute workspace path. All other text goes to stderr.
- **Server address.** `--url` / `TREEYARD_URL` first. Else the hook reads the `treeyard connect` state
  of the repository at the working directory (`.git/treeyard/connect.json`). A linked worktree finds the
  state too, because the hook reads the common git directory. There is no fixed default port.
- **Exit codes.** 0 done; 1 error; 2 bad name, or no server address; 3 the server does not answer.

`create`:

- Opens a change (agent `hook-<name>` unless `--agent`) and makes its workspace, with a `.git` unless
  `--no-git`. The name defaults to the base name of `--from-dir`, else of the working directory.
- `--from-dir PATH` seeds the change with the uncommitted edits of the checkout at PATH. It also copies
  the gitignored files that `.worktreeinclude` or `copy` in `.treeyard/workspace.toml` name.
- A second `create` with the same name returns the same path and opens no second change. A workspace that
  failed before is removed (its empty change is abandoned) and made again.
- When the server does not answer, `create` exits 3 and writes nothing to stdout or to the node home.

`remove [NAME | PATH] [--from-dir PATH] [--abandon] [--force]`:

- Sends the edits that no sync sent yet, then deletes the directory. The change stays open; `--abandon`
  drops it. With no NAME, it takes the name that `create` gets from `--from-dir` or the working directory.
- A workspace that is not there gives "not found" on stderr and exit 0, so a repeated call is safe.
- When the server does not answer, `remove` exits 3 and deletes nothing, unless `--force`.

Tests: `tests/workspace_hook.rs` (create, repeated create, remove, repeated remove, no server, no
address, connect state from a linked worktree).

## What the three orchestrators allow

All three run a setup command inside a git worktree that they make themselves. None of the three reads a
directory path back from that command. So the agent keeps working in the orchestrator's worktree, not in
the TreeYard workspace. The hook then gives you two things only:

- each agent's work starts as an open TreeYard change with a declared intent (seeded with the worktree's
  uncommitted edits), so other agents see it in `collisions` and `connect status`;
- a removal step that sends the edits made in the TreeYard workspace directory.

Edits that the agent makes later in the orchestrator's worktree do not reach that change. For warnings
while the agent works, use API-only mode: run `treeyard connect` in the repository
([connect.md](connect.md)). The agent then opens its change and edits through MCP (`open_change`,
`apply`, `land`), whatever directory the orchestrator gives it. The recipes below add the hook on top of
that; you can also leave them out.

## Orca

- **Hook point:** `scripts.setup` and `scripts.archive` in `orca.yaml` at the repository root. Orca runs
  `setup` in the new worktree and `archive` in the worktree before removal. Orca reads `setup` from the
  new worktree's `orca.yaml` and `archive` from the primary checkout's `orca.yaml`. The CLI runs archive
  hooks only with `--run-hooks`. A failed archive hook blocks the removal.
- **Environment:** `ORCA_ROOT_PATH` (primary checkout), `ORCA_WORKTREE_PATH` (new worktree),
  `ORCA_WORKSPACE_NAME` (worktree directory base name).
- **Result:** in `runHook`, Orca keeps the exit status and the output text of a hook, and nothing else.
  It does not take a path. The default hook time limit there is 120 s (`HOOK_TIMEOUT`).
- **Read on 2026-11-21, commit `0c5714bd4e672956258b156fea419e1e8536b5f0` of stablyai/orca:**
  - [docs/site/content/docs/model/orca-yaml.mdx](https://github.com/stablyai/orca/blob/0c5714bd4e672956258b156fea419e1e8536b5f0/docs/site/content/docs/model/orca-yaml.mdx)
  - [src/main/workspace-hooks/setup-hook-env-vars.ts](https://github.com/stablyai/orca/blob/0c5714bd4e672956258b156fea419e1e8536b5f0/src/main/workspace-hooks/setup-hook-env-vars.ts)
  - [src/main/workspace-hooks/hooks.ts](https://github.com/stablyai/orca/blob/0c5714bd4e672956258b156fea419e1e8536b5f0/src/main/workspace-hooks/hooks.ts) (`runHook`, `classifyHookProcessResult`)
  - [src/main/runtime/workspaces/removal/runtime-registered-local-worktree-removal.ts](https://github.com/stablyai/orca/blob/0c5714bd4e672956258b156fea419e1e8536b5f0/src/main/runtime/workspaces/removal/runtime-registered-local-worktree-removal.ts)

```yaml
# orca.yaml
scripts:
  setup: |
    treeyard workspace-hook create --from-dir "$ORCA_WORKTREE_PATH" --intent "Orca workspace $ORCA_WORKSPACE_NAME"
  archive: |
    treeyard workspace-hook remove --from-dir "$ORCA_WORKTREE_PATH"
```

## Paradev (formerly Superset)

- **Hook point:** `setup` and `teardown` (lists of commands) in `.paradev/config.json`. A repository
  set up before the rename keeps `.superset/config.json`, and Paradev still reads it. Without commands,
  Paradev runs `.paradev/setup.sh` / `teardown.sh`. A per-machine override lives under
  `~/.paradev/projects/<repo-path>/config.json`, and `config.local.json` adds `before` / `after` steps.
  The docs say setup and teardown run in the workspace directory. A teardown failure in the CLI, SDK or
  MCP does not block the delete.
- **Environment (docs):** `SUPERSET_ROOT_PATH`, `SUPERSET_WORKSPACE_NAME`, `SUPERSET_WORKSPACE_PATH`.
  The host-service terminal code sets `SUPERSET_WORKSPACE_PATH` and `SUPERSET_ROOT_PATH`. We did not find
  `SUPERSET_WORKSPACE_NAME` in that file, so the recipe uses `$PWD`.
- **Newer format:** Paradev's own repository also has `.paradev/config.toml` with `[setup] run` and
  `[archive] run`. The teardown code calls a v1 project's `teardown` its local `[archive]` command.
- **Result:** setup runs as the first command of a terminal. Paradev does not take a path from it.
- **Read on 2026-11-21, commit `f9e295ff90d7c1d3ac99d1e02c1f982ac004563b` of ParadevAI/Paradev:**
  - [apps/docs/content/docs/setup-teardown-scripts.mdx](https://github.com/ParadevAI/Paradev/blob/f9e295ff90d7c1d3ac99d1e02c1f982ac004563b/apps/docs/content/docs/setup-teardown-scripts.mdx)
  - [packages/host-service/src/runtime/setup/config.ts](https://github.com/ParadevAI/Paradev/blob/f9e295ff90d7c1d3ac99d1e02c1f982ac004563b/packages/host-service/src/runtime/setup/config.ts) (`SetupConfig`)
  - [packages/host-service/src/terminal/env.ts](https://github.com/ParadevAI/Paradev/blob/f9e295ff90d7c1d3ac99d1e02c1f982ac004563b/packages/host-service/src/terminal/env.ts)
  - [packages/host-service/src/runtime/teardown/teardown.ts](https://github.com/ParadevAI/Paradev/blob/f9e295ff90d7c1d3ac99d1e02c1f982ac004563b/packages/host-service/src/runtime/teardown/teardown.ts) (`resolveTeardownCommand`)
  - [packages/host-service/src/trpc/router/workspace-creation/shared/setup-terminal.ts](https://github.com/ParadevAI/Paradev/blob/f9e295ff90d7c1d3ac99d1e02c1f982ac004563b/packages/host-service/src/trpc/router/workspace-creation/shared/setup-terminal.ts) (`resolveInitialCommand`)
  - [.paradev/config.toml](https://github.com/ParadevAI/Paradev/blob/f9e295ff90d7c1d3ac99d1e02c1f982ac004563b/.paradev/config.toml)

```json
{
  "setup": ["treeyard workspace-hook create --from-dir \"$PWD\""],
  "teardown": ["treeyard workspace-hook remove --from-dir \"$PWD\""]
}
```

Add these lines to the existing `setup` and `teardown` lists. Do not replace them.

## Agent Orchestrator

The repository ComposioHQ/agent-orchestrator now redirects to OrchestratorInc/agent-orchestrator.

- **Hook point:** the project settings `postCreate` (commands after the worktree exists) and `preRemove`
  (commands before a managed worktree is removed). The daemon keeps project settings as JSON in SQLite.
  The docs say: "There is no current `agent-orchestrator.yaml` project registry." Edit them in
  **Project Settings → Scripts**, or with `ao project set-config <id> --post-create CMD --pre-remove CMD`
  (both repeatable).
- **Environment:** each step runs through `sh` with the worktree as its working directory, and gets
  `AO_WORKTREE_PATH`, `AO_SOURCE_TREE_PATH` and the project `env`. A failed `postCreate` step stops the
  agent start. A failed `preRemove` step keeps the worktree. `preRemove` runs only for worktrees under the
  daemon's own `worktrees` directory.
- **Workspace slot:** the docs say that AO "does not currently offer a full-clone workspace selector". A
  Git project always gets a git worktree. We found no setting that puts another directory in its place.
- **Result:** AO keeps only the last 8 KiB of a step's output, for its error message. It does not take a path.
- **Read on 2026-11-21, commit `080a1a2a5830673caa7b4f6571172bbea9f6e114` of OrchestratorInc/agent-orchestrator:**
  - [frontend/src/docs/content/configuration/projects.mdx](https://github.com/OrchestratorInc/agent-orchestrator/blob/080a1a2a5830673caa7b4f6571172bbea9f6e114/frontend/src/docs/content/configuration/projects.mdx)
  - [frontend/src/docs/content/plugins/workspaces/index.mdx](https://github.com/OrchestratorInc/agent-orchestrator/blob/080a1a2a5830673caa7b4f6571172bbea9f6e114/frontend/src/docs/content/plugins/workspaces/index.mdx)
  - [frontend/src/docs/content/plugins/workspaces/worktree.mdx](https://github.com/OrchestratorInc/agent-orchestrator/blob/080a1a2a5830673caa7b4f6571172bbea9f6e114/frontend/src/docs/content/plugins/workspaces/worktree.mdx)
  - [backend/internal/session_manager/manager.go](https://github.com/OrchestratorInc/agent-orchestrator/blob/080a1a2a5830673caa7b4f6571172bbea9f6e114/backend/internal/session_manager/manager.go) (`provisionWorkspace`, `runPostCreate`)
  - [backend/internal/session_manager/workspace_cleanup.go](https://github.com/OrchestratorInc/agent-orchestrator/blob/080a1a2a5830673caa7b4f6571172bbea9f6e114/backend/internal/session_manager/workspace_cleanup.go) (`runPreRemoveContext`)
  - [backend/internal/cli/project.go](https://github.com/OrchestratorInc/agent-orchestrator/blob/080a1a2a5830673caa7b4f6571172bbea9f6e114/backend/internal/cli/project.go) (`--post-create`, `--pre-remove`)

```sh
ao project set-config <project-id> \
  --post-create 'treeyard workspace-hook create --from-dir "$AO_WORKTREE_PATH"' \
  --pre-remove 'treeyard workspace-hook remove --from-dir "$AO_WORKTREE_PATH"'
```

`set-config --post-create` replaces the whole `postCreate` list (`cfg.PostCreate = opts.postCreate` in
`project.go`). Repeat the flag for every step that you keep, or add the lines in Project Settings.

## Claude Code

Claude Code reads the workspace path from the stdout of its `WorktreeCreate` hook, so there the agent does
work in the TreeYard workspace. The hooks in [examples/claude-code](../examples/claude-code) do this with
`treeyard workspace create` and `treeyard workspace path`.
