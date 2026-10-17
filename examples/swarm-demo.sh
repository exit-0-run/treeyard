#!/bin/sh
# N scripted agents against plain git worktrees and against TreeYard (binary treeyard); prints a table.
#   examples/swarm-demo.sh                      # 10 agents, 2000 files, 2 same-line pairs
#   examples/swarm-demo.sh --agents 50 --pairs 5 --json /tmp/demo.json
# Needs git, python3 (standard library) and a treeyard binary: on PATH, TREEYARD_BIN, --bin,
# or target/release/treeyard (`make release`). Everything runs in a temporary directory.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
if [ -z "${TREEYARD_BIN:-}" ]; then
    if [ -x "$here/../target/release/treeyard" ]; then
        TREEYARD_BIN="$here/../target/release/treeyard"
    else
        TREEYARD_BIN=treeyard
    fi
fi
export TREEYARD_BIN
exec python3 -I "$here/swarm_demo.py" "$@"
