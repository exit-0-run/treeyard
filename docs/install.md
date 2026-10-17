# Install

`rootstock` is one binary (static on Linux). It needs `git` 2.44 or newer on the server and on
nodes: the server reads objects with `git cat-file` and merges lines with
`git merge-file --diff-algorithm`, which git 2.44 added
([release notes](https://raw.githubusercontent.com/git/git/master/Documentation/RelNotes/2.44.0.adoc),
opened 2026-10-18).

## From a release directory

A release is a directory (or URL) with one archive per platform, `SHA256SUMS` and `install.sh`.
`install.sh` picks the archive for this machine, checks its SHA-256, and copies the binary to
`~/.local/bin` (change it with `--prefix`). It needs no root rights.

```text
sh dist/install.sh --from dist                 # a local release directory
sh install.sh --from https://HOST/PATH/v0.1.0  # any URL that serves the same files
```

| Archive | Platform | Built with |
|---|---|---|
| `rootstock-0.1.0-aarch64-apple-darwin.tar.gz` | macOS on Apple Silicon | `cargo build --release` |
| `rootstock-0.1.0-x86_64-unknown-linux-musl.tar.gz` | Linux x86_64, static (musl) | `cargo zigbuild` |
| `rootstock-0.1.0-aarch64-unknown-linux-musl.tar.gz` | Linux arm64, static (musl) | `cargo zigbuild` |

No release is published yet. Build one with the next section.

## Build the release directory

```text
sh scripts/dist.sh
```

It builds this machine's target and, when `cargo-zigbuild` and `zig` are on `PATH`, the two Linux
musl targets. It writes `dist/rootstock-<version>-<target>.tar.gz`, `dist/SHA256SUMS` and
`dist/install.sh`. `dist/` is gitignored. The script uploads nothing.

Cross-build tools, kept outside the repo (no global install needed):

```text
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo install --locked --root "$TOOLS" cargo-zigbuild      # 0.23.4 worked on 2026-10-18
# zig 0.15.2 from https://ziglang.org/download/ (check the SHA-256 in index.json), unpacked in $TOOLS
PATH="$TOOLS/bin:$TOOLS/zig-aarch64-macos-0.15.2:$PATH" sh scripts/dist.sh
```

An Intel Mac (`x86_64-apple-darwin`) build is not in the list. `DIST_TARGETS="x86_64-apple-darwin"
sh scripts/dist.sh` should work after `rustup target add x86_64-apple-darwin`, but no one ran it.

## From source

```text
make release                     # target/release/rootstock
cp target/release/rootstock ~/.local/bin/
```

Rust 2024 edition (rustc 1.85 or newer; this repo builds with 1.99.1). `make check` runs format,
clippy and all tests.

## Checked on 2026-10-18

- `sh scripts/dist.sh` on the macOS arm64 machine built all three archives (about 50 s with warm
  caches) and wrote `SHA256SUMS`. The Linux binaries are `ELF 64-bit LSB executable ... statically
  linked` (`file`).
- `sh dist/install.sh --from dist --prefix <empty dir>` printed `installed rootstock 0.1.0 ...
  (checksum ok)`. With one byte appended to the archive it printed `install: checksum mismatch` and
  exited 1 without installing.
- In `alpine:3` containers (Docker on the same machine), `install.sh` installed the arm64 archive
  (native) and the x86_64 archive (emulated). Then `scripts/check-quickstart.sh docs/quickstart.md`
  passed on both, with git 2.54.0 from Alpine.
