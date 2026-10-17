#!/bin/sh
# Claude Code WorktreeCreate hook: a Rootstock workspace instead of `git worktree add`.
# Claude Code sends JSON on stdin ({"name": "bold-oak-a3f2", ...}) and reads the directory
# from the last line of stdout. Everything else goes to stderr.
# Env: ROOTSTOCK_URL (server), ROOTSTOCK_HOME (node home), optional ROOTSTOCK_SETUP (a shell
# command run once in the new workspace, e.g. "cargo build -q"), ROOTSTOCK_TOKEN.
set -eu
name=$(python3 -I -c 'import json,sys; print(json.load(sys.stdin)["name"])')
case $name in *[!A-Za-z0-9._-]*|"") echo "worktree-create: bad name $name" >&2; exit 1 ;; esac
if [ -n "${ROOTSTOCK_SETUP:-}" ]; then
    rootstock workspace create "$name" --agent "claude-$name" --intent "Claude Code session $name" --git \
        --setup "$ROOTSTOCK_SETUP" >&2
else
    rootstock workspace create "$name" --agent "claude-$name" --intent "Claude Code session $name" --git >&2
fi
rootstock workspace path "$name"
