#!/bin/sh
# Runs every ```sh block of a Markdown file, word for word, in a new empty directory.
#   sh scripts/check-quickstart.sh docs/quickstart.md            # print the transcript
#   sh scripts/check-quickstart.sh docs/quickstart.md --write    # also store it in the file
# Uses the `treeyard` on PATH (put a fresh install or target/release first). Stops at the first error.
set -eu
doc=$1
write=${2:-}
case $doc in /*) ;; *) doc="$PWD/$doc" ;; esac
dir=$(mktemp -d "${TMPDIR:-/tmp}/treeyard-quickstart.XXXXXX")
script="$dir.sh"
log="$dir.log"
python3 -I - "$doc" "$script" <<'EOF'
import re, sys
text = open(sys.argv[1]).read()
blocks = re.findall(r"^```sh\n(.*?)^```", text, flags=re.S | re.M)
out = ["set -e", "unset TREEYARD_URL TREEYARD_TOKEN TREEYARD_HOME TREEYARD_AGENT", "trap '[ -n \"${SERVER:-}\" ] && kill \"$SERVER\" 2>/dev/null; true' EXIT"]
for b in blocks:
    for line in b.rstrip("\n").split("\n"):
        if not line.strip():
            continue
        shown = line.replace("'", "'\\''")
        out.append(f"printf '%s\\n' '$ {shown}'")
        out.append(line)
open(sys.argv[2], "w").write("\n".join(out) + "\n")
EOF
echo "# $(treeyard --version) | $(uname -sm) | $(date -u +%Y-%m-%dT%H:%MZ) | fresh dir $(basename "$dir")" > "$log"
status=0
(cd "$dir" && sh "$script") >> "$log" 2>&1 || status=$?
cat "$log"
if [ "$status" -ne 0 ]; then
    echo "check-quickstart: FAILED (exit $status) in $dir" >&2
    exit "$status"
fi
if [ "$write" = "--write" ]; then
    python3 -I - "$doc" "$log" <<'EOF'
import sys
doc, log = sys.argv[1], sys.argv[2]
text = open(doc).read()
start, end = "<!-- transcript:start -->", "<!-- transcript:end -->"
a, b = text.index(start) + len(start), text.index(end)
body = "\n```text\n" + open(log).read() + "```\n"
open(doc, "w").write(text[:a] + body + text[b:])
EOF
    echo "check-quickstart: transcript written to $doc" >&2
fi
rm -rf "$dir" "$script" "$log"
echo "check-quickstart: ok" >&2
