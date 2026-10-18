#!/bin/sh
# Claude Code UserPromptSubmit hook: one TreeYard checkpoint per agent turn. It syncs the
# workspace the session runs in, then marks the change's head (`checkpoint`), so the state
# after each turn can come back with `treeyard workspace restore NAME --to N`.
# Claude Code sends JSON on stdin ({"session_id": ..., "cwd": ..., "prompt": ...}); stdout
# of this hook would become context for the model, so everything goes to stderr. It never
# blocks the prompt: outside a workspace, or on any error, it exits 0.
set -u
input=$(python3 -I -c 'import json,sys
d=json.load(sys.stdin)
s="".join(c for c in str(d.get("session_id","")) if c.isalnum() or c in "-_")[:40]
print(d.get("cwd") or "."); print(s)') || exit 0
dir=$(printf '%s\n' "$input" | sed -n 1p)
session=$(printf '%s\n' "$input" | sed -n 2p)
while [ "$dir" != / ] && [ "$dir" != . ] && [ ! -f "$dir/.treeyard/env" ]; do dir=$(dirname "$dir"); done
[ -f "$dir/.treeyard/env" ] || exit 0
val() { sed -n "s/^$1=//p" "$dir/.treeyard/env" | head -n 1; }
ws=$(val TREEYARD_WORKSPACE)
change=$(val TREEYARD_CHANGE)
TREEYARD_URL=${TREEYARD_URL:-$(val TREEYARD_URL)}
export TREEYARD_URL
treeyard workspace sync "$ws" >&2 || exit 0
treeyard call checkpoint "{\"change\": \"$change\", \"label\": \"turn of session $session\"}" >&2 || true
exit 0
