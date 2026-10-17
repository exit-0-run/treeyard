#!/bin/sh
# Runs the Linux code paths of the workspace layer in a privileged Docker container:
#   1. all tests on the container's own file system (overlayfs root: no reflink, so `auto` copies),
#   2. the cow and workspace tests with TMPDIR on a loop-mounted XFS with reflink=1
#      (FICLONE reflinks, and the overlayfs workspace test, which needs root).
# Needs a running Docker (for example colima). Downloads the rust image once; builds in a volume.
set -eu
IMAGE=${IMAGE:-rust:1}
cd "$(dirname "$0")/.."
tar --exclude=./target --exclude=./.git -cf - . | docker run --rm -i --privileged \
  -v rootstock-linux-target:/target -v rootstock-linux-cargo:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/target "$IMAGE" sh -euc '
    mkdir /work && tar -C /work -xf - && cd /work
    run() { if "$@" >/tmp/log 2>&1; then r=ok; else r=FAILED; fi
            grep -E "^test |test result|skip:|panicked" /tmp/log || tail -20 /tmp/log; echo "-> $r"; [ $r = ok ]; }
    (apt-get update -qq && apt-get install -y -qq xfsprogs) >/dev/null 2>&1
    uname -sr; cargo --version
    echo "== 1. container file system: $(stat -f -c %T /tmp)"
    run cargo test
    truncate -s 4G /xfs.img && mkfs.xfs -q -m reflink=1 /xfs.img && mkdir -p /mnt/xfs && mount -o loop /xfs.img /mnt/xfs
    echo "== 2. TMPDIR on $(stat -f -c %T /mnt/xfs) with reflink=1, as $(id -un)"
    export TMPDIR=/mnt/xfs ROOTSTOCK_EXPECT_REFLINK=1 ROOTSTOCK_TEST_OVERLAY=1
    run cargo test --lib cow -- --nocapture
    run cargo test --test workspace -- --nocapture
  '
