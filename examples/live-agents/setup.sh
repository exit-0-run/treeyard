#!/bin/sh
# Prepares the work dir for demo.py: a clone of more-itertools and a venv with pytest.
set -eu
D=${DEMO_DIR:-$HOME/.cache/treeyard-live-agents}
mkdir -p "$D"
[ -d "$D/src" ] || git clone -q https://github.com/more-itertools/more-itertools "$D/src"
git -C "$D/src" cat-file -e 2fe1b2e^{commit}
[ -x "$D/venv/bin/python" ] || python3 -m venv "$D/venv"
"$D/venv/bin/pip" -q install pytest
echo "ready: $D"
