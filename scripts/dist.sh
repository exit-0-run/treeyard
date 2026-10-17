#!/bin/sh
# Builds release archives into dist/ (gitignored) and writes dist/SHA256SUMS.
#   sh scripts/dist.sh                 # this machine's target, plus Linux when cargo-zigbuild is found
#   DIST_TARGETS="aarch64-apple-darwin" sh scripts/dist.sh
# Linux targets are static musl binaries built with cargo-zigbuild and zig (see docs/install.md).
# Nothing here uploads or publishes anything.
set -eu
cd "$(dirname "$0")/.."
CARGO=${CARGO:-$(command -v cargo || echo /opt/homebrew/opt/rustup/bin/cargo)}
PATH="$(dirname "$CARGO"):$PATH"
export PATH

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
host=$(rustc -vV | sed -n 's/^host: //p')
linux="x86_64-unknown-linux-musl aarch64-unknown-linux-musl"
if [ -z "${DIST_TARGETS:-}" ]; then
    DIST_TARGETS=$host
    if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
        DIST_TARGETS="$DIST_TARGETS $linux"
    else
        echo "dist: cargo-zigbuild or zig not on PATH, so no Linux archives (docs/install.md)" >&2
    fi
fi

mkdir -p dist
for t in $DIST_TARGETS; do
    if [ "$t" = "$host" ]; then
        "$CARGO" build --release --locked --target "$t"
    else
        rustup target add "$t" >/dev/null
        "$CARGO" zigbuild --release --locked --target "$t"
    fi
    name="treeyard-$version-$t"
    stage=$(mktemp -d)
    mkdir "$stage/$name"
    cp "target/$t/release/treeyard" LICENSE README.md CHANGELOG.md "$stage/$name/"
    tar -C "$stage" -czf "dist/$name.tar.gz" "$name"
    rm -rf "$stage"
    echo "dist: dist/$name.tar.gz"
done

(cd dist && rm -f SHA256SUMS && for f in treeyard-*.tar.gz; do
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$f"; else shasum -a 256 "$f"; fi
done > SHA256SUMS)
cp scripts/install.sh dist/install.sh
cat dist/SHA256SUMS
