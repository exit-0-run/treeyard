#!/bin/sh
# Claude Code WorktreeRemove hook: send the last edits to the server, then delete the directory.
# The change stays open on the server, so nothing is lost: land it later with
# `treeyard call land '{"change":"ch_..."}'` or drop it with `treeyard call abandon ...`.
set -eu
dir=$(python3 -I -c 'import json,sys; print(json.load(sys.stdin)["worktree_path"])')
name=$(basename "$dir")
treeyard workspace sync "$name" >&2
treeyard workspace remove "$name" >&2
