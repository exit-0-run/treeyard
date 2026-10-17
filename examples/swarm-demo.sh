#!/bin/sh
# N scripted agents against plain git worktrees and against TreeYard (binary rootstock); prints a table.
#   examples/swarm-demo.sh                      # 10 agents, 2000 files, 2 same-line pairs
#   examples/swarm-demo.sh --agents 50 --pairs 5 --json /tmp/demo.json
# Needs git, python3 (standard library) and a rootstock binary: on PATH, ROOTSTOCK_BIN, --bin,
# or target/release/rootstock (`make release`). Everything runs in a temporary directory.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
if [ -z "${ROOTSTOCK_BIN:-}" ]; then
    if [ -x "$here/../target/release/rootstock" ]; then
        ROOTSTOCK_BIN="$here/../target/release/rootstock"
    else
        ROOTSTOCK_BIN=rootstock
    fi
fi
export ROOTSTOCK_BIN
exec python3 -I "$here/swarm_demo.py" "$@"
