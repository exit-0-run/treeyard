#!/bin/sh
# Runs both arms of demo.py inside a Claude Code cloud session (or any Linux box with the `claude` CLI):
# builds treeyard from this checkout, prepares more-itertools, runs git then treeyard, and copies the results
# to bench/results/live-agents/<tag>/ in this checkout (with PUSH_BRANCH=B: committed and pushed to B after each arm).
#   DEMO_N=5 DEMO_POOL=5 sh examples/live-agents/cloud.sh pilot
#   DEMO_SET=wide DEMO_POOL=10 ARMS="git treeyard" sh examples/live-agents/cloud.sh wide50
set -eu
TAG=${1:-run}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export DEMO_DIR=${DEMO_DIR:-$HOME/.cache/treeyard-live-agents}
export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=maintenance.auto GIT_CONFIG_VALUE_0=false \
  GIT_CONFIG_KEY_1=gc.auto GIT_CONFIG_VALUE_1=0
OUT="$ROOT/bench/results/live-agents/$TAG"
mkdir -p "$OUT"
# A reused VM keeps DEMO_DIR: never copy an earlier run's results into this one.
rm -rf "$DEMO_DIR/results" "$DEMO_DIR/agents"
{
  echo "nproc: $(nproc 2>/dev/null || sysctl -n hw.ncpu)"
  free -m 2>/dev/null | sed -n 2p || true
  echo "claude: $(command -v claude || echo missing) $(claude --version 2>/dev/null || true)"
} | tee "$OUT/env.txt"
command -v claude >/dev/null || { echo "no claude CLI: stop" | tee -a "$OUT/env.txt"; exit 2; }
claude -p "Reply with the single word ok." --output-format json --max-turns 1 > "$OUT/probe.json" 2>&1 ||
  { echo "claude -p failed: stop" | tee -a "$OUT/env.txt"; cat "$OUT/probe.json"; exit 3; }
(cd "$ROOT" && cargo build --release -q)
export TREEYARD_BIN="$ROOT/target/release/treeyard"
sh "$ROOT/examples/live-agents/setup.sh"
for arm in ${ARMS:-git treeyard}; do
  python3 "$ROOT/examples/live-agents/demo.py" "$arm" 2>&1 | tee "$OUT/$arm.log"
  cp "$DEMO_DIR"/results/*.json "$OUT/"
  cp "$DEMO_DIR"/results/*.log "$OUT/" 2>/dev/null || true
  mkdir -p "$OUT/agents" && cp "$DEMO_DIR"/agents/*.json "$OUT/agents/" 2>/dev/null || true
  if [ -n "${PUSH_BRANCH:-}" ]; then  # push after each arm, so a reclaimed VM keeps the finished arm
    git -C "$ROOT" checkout -q -B "$PUSH_BRANCH"
    git -C "$ROOT" add "bench/results/live-agents/$TAG"
    git -C "$ROOT" commit -q -m "Live agents $TAG: $arm arm" && git -C "$ROOT" push -q -f origin "$PUSH_BRANCH"
  fi
done
echo "results: $OUT"
