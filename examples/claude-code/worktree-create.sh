#!/bin/sh
# Claude Code WorktreeCreate hook: a TreeYard workspace instead of `git worktree add`.
# Claude Code sends JSON on stdin ({"name": "bold-oak-a3f2", ...}) and reads the directory
# from the last line of stdout. Everything else goes to stderr.
# Env: TREEYARD_URL (server), TREEYARD_HOME (node home), optional TREEYARD_SETUP (a shell
# command run once in the new workspace, e.g. "cargo build -q"), TREEYARD_TOKEN.
set -eu
name=$(python3 -I -c 'import json,sys; print(json.load(sys.stdin)["name"])')
case $name in *[!A-Za-z0-9._-]*|"") echo "worktree-create: bad name $name" >&2; exit 1 ;; esac
if [ -n "${TREEYARD_SETUP:-}" ]; then
    treeyard workspace create "$name" --agent "claude-$name" --intent "Claude Code session $name" --git \
        --setup "$TREEYARD_SETUP" >&2
else
    treeyard workspace create "$name" --agent "claude-$name" --intent "Claude Code session $name" --git >&2
fi
treeyard workspace path "$name"
