#!/bin/sh
# Landing load test against plain git and TreeYard, the shape of gitai's graft-bench
# (doc/graft/BENCH.md there): N agents land C changes each on one branch, a share of them conflict.
#   sh bench/landbench.sh [--agents 50 --changes 5 --conflict-rate 0.2 --seed 1] [--runs 2]
# Needs git and a release build (cargo build --release --bin treeyard --bin landbench).
# Plain git: one bare repository on local disk (no network), one clone per agent; a rejected push runs
# `git pull --rebase` and retries, up to 20 attempts. TreeYard: `treeyard serve`, landing mode direct.
set -eu
export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=maintenance.auto GIT_CONFIG_VALUE_0=false GIT_CONFIG_KEY_1=gc.auto GIT_CONFIG_VALUE_1=0
here=$(cd "$(dirname "$0")/.." && pwd)
bin="$here/target/release"
runs=1
args=""
while [ $# -gt 0 ]; do
  case $1 in
    --runs) runs=$2; shift 2 ;;
    *) args="$args $1"; shift ;;
  esac
done
port=$((7600 + $$ % 300))
root=$(mktemp -d "${TMPDIR:-/tmp}/landbench.XXXXXX")
trap 'kill "${SERVER:-}" 2>/dev/null || true; rm -rf "$root"' EXIT
r=1
while [ "$r" -le "$runs" ]; do
  seed="$root/seed-$r"
  git init -q -b main "$seed"
  printf 'shared file\n' > "$seed/shared.txt"
  git -C "$seed" add -A
  git -C "$seed" -c user.email=b@b -c user.name=b commit -qm seed
  git clone -q --bare "$seed" "$root/remote-$r.git"
  echo "== run $r: plain git"
  # shellcheck disable=SC2086
  "$bin/landbench" git --remote "$root/remote-$r.git" --work "$root/git-$r" $args
  "$bin/treeyard" init --data "$root/data-$r" --from "$seed" --branch main > /dev/null
  "$bin/treeyard" serve --data "$root/data-$r" --addr "127.0.0.1:$port" > "$root/serve-$r.log" 2>&1 &
  SERVER=$!
  i=0
  until "$bin/treeyard" call status '{}' --url "http://127.0.0.1:$port" > /dev/null 2>&1; do
    i=$((i + 1)); [ "$i" -lt 100 ] || { cat "$root/serve-$r.log"; exit 1; }; sleep 0.1
  done
  echo "== run $r: TreeYard"
  # shellcheck disable=SC2086
  "$bin/landbench" treeyard --url "http://127.0.0.1:$port" $args
  kill "$SERVER"; wait "$SERVER" 2>/dev/null || true; SERVER=
  r=$((r + 1))
done
