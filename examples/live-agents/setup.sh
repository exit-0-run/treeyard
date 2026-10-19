#!/bin/sh
# Prepares the work dir for demo.py: a clone of more-itertools and a venv with pytest.
set -eu
D=${DEMO_DIR:-$HOME/.cache/treeyard-live-agents}
mkdir -p "$D"
[ -d "$D/src" ] || git clone -q https://github.com/more-itertools/more-itertools "$D/src"
git -C "$D/src" cat-file -e 2fe1b2e^{commit}
[ -x "$D/venv/bin/python" ] || python3 -m venv "$D/venv"
"$D/venv/bin/pip" -q install pytest
if [ "${DEMO_SET:-}" = rg ]; then  # ripgrep for DEMO_SET=rg (base 3fce3b5); dependencies fetched once
  [ -d "$D/src-rg" ] || git clone -q https://github.com/BurntSushi/ripgrep "$D/src-rg"
  git -C "$D/src-rg" cat-file -e 3fce3b5bb0236da2df6d99672afb8a719642eca7^{commit}
  (cd "$D/src-rg" && git checkout -q 3fce3b5bb0236da2df6d99672afb8a719642eca7 && cargo fetch -q && git checkout -q -)
fi
echo "ready: $D"
