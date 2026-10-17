#!/bin/sh
# Claude Code WorktreeRemove hook: send the last edits to the server, then delete the directory.
# The change stays open on the server, so nothing is lost: land it later with
# `rootstock call land '{"change":"ch_..."}'` or drop it with `rootstock call abandon ...`.
set -eu
dir=$(python3 -I -c 'import json,sys; print(json.load(sys.stdin)["worktree_path"])')
name=$(basename "$dir")
rootstock workspace sync "$name" >&2
rootstock workspace remove "$name" >&2
