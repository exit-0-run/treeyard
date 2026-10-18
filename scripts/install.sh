#!/bin/sh
# Installs the treeyard binary from a release directory or URL, after a SHA-256 check.
#   curl -fsSL https://github.com/exit-0-run/treeyard/releases/latest/download/install.sh | sh
#   sh install.sh --from ./dist                       # a directory made by scripts/dist.sh
#   sh install.sh --from https://example.org/v0.1.0   # any URL that serves the archives + SHA256SUMS
#   options: --prefix DIR (default ~/.local), --version V (default: the newest archive in SHA256SUMS)
# Needs: sh, tar, and sha256sum or shasum; curl or wget for a URL. No root rights.
set -eu
from=${TREEYARD_DIST:-}
prefix=${PREFIX:-$HOME/.local}
version=
while [ $# -gt 0 ]; do
    case $1 in
        --from) from=$2; shift 2 ;;
        --prefix) prefix=$2; shift 2 ;;
        --version) version=$2; shift 2 ;;
        -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
        *) echo "install: unknown option $1" >&2; exit 2 ;;
    esac
done
if [ -z "$from" ]; then
    here=$(cd "$(dirname "$0")" && pwd)
    for d in "$here" "$here/../dist"; do
        if [ -f "$d/SHA256SUMS" ]; then from=$d; break; fi
    done
fi
# Default: the newest release on GitHub (each asset redirects from /latest/download/).
[ -n "$from" ] || from=https://github.com/exit-0-run/treeyard/releases/latest/download

case $(uname -s) in
    Darwin) os=apple-darwin ;;
    Linux) os=unknown-linux-musl ;;
    *) echo "install: no build for $(uname -s); build from source: cargo build --release" >&2; exit 1 ;;
esac
case $(uname -m) in
    arm64|aarch64) arch=aarch64 ;;
    x86_64|amd64) arch=x86_64 ;;
    *) echo "install: no build for $(uname -m); build from source: cargo build --release" >&2; exit 1 ;;
esac
target=$arch-$os

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fetch() { # fetch NAME: copy or download one file of the release into $tmp
    case $from in
        http://*|https://*)
            if command -v curl >/dev/null 2>&1; then curl -fsSL -o "$tmp/$1" "$from/$1"
            else wget -q -O "$tmp/$1" "$from/$1"; fi ;;
        *) cp "$from/$1" "$tmp/$1" ;;
    esac
}
fetch SHA256SUMS
if [ -z "$version" ]; then
    version=$(sed -n "s/.*treeyard-\(.*\)-$target\.tar\.gz$/\1/p" "$tmp/SHA256SUMS" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
fi
name=treeyard-$version-$target
grep -q " $name.tar.gz\$" "$tmp/SHA256SUMS" || { echo "install: $from has no $name.tar.gz" >&2; exit 1; }
fetch "$name.tar.gz"
(cd "$tmp" && grep " $name.tar.gz\$" SHA256SUMS > want &&
    if command -v sha256sum >/dev/null 2>&1; then sha256sum -c want >/dev/null; else shasum -a 256 -c want >/dev/null; fi) ||
    { echo "install: checksum mismatch for $name.tar.gz" >&2; exit 1; }
tar -C "$tmp" -xzf "$tmp/$name.tar.gz"
mkdir -p "$prefix/bin"
cp "$tmp/$name/treeyard" "$prefix/bin/treeyard.new"
chmod 755 "$prefix/bin/treeyard.new"
mv "$prefix/bin/treeyard.new" "$prefix/bin/treeyard"
echo "installed $("$prefix/bin/treeyard" --version) to $prefix/bin/treeyard (checksum ok)"
case :$PATH: in *":$prefix/bin:"*) ;; *) echo "add $prefix/bin to PATH" ;; esac
