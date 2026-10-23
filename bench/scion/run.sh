#!/bin/sh
# Landing load test against CarterT27/scion (https://github.com/CarterT27/scion), the shape of
# bench/landbench.sh: 50 agents land 5 changes each on main, 20 % append to one shared file.
#   sh bench/scion/run.sh [memory|external] [--agents 50 --changes 5 --conflict-rate 0.2 --seed 1]
# Builds scion at a pinned commit with landrace.rs added as an example of its scion-conformance crate
# (it uses that crate's TestCluster: an in-process server over loopback TLS/gRPC, no daemon).
# Needs rustup (scion pins Rust 1.98.0) and protoc. `external` needs CockroachDB on localhost:26257
# and an S3 endpoint on localhost:9000 with bucket scion-test (scion's docker/compose.test.yml);
# docs/benchmarks.md says how the published numbers ran them.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
rev=e0ffadf
dir=${SCION_DIR:-$HOME/.cache/ty-scion/scion}
backend=${1:-memory}
[ $# -gt 0 ] && shift
[ -d "$dir" ] || git clone -q https://github.com/CarterT27/scion "$dir"
git -C "$dir" checkout -q "$rev"
cp "$here/landrace.rs" "$dir/crates/scion-conformance/examples/landrace.rs"
(cd "$dir" && cargo build -q --release -p scion-conformance --example landrace)
export SCION_TEST_CRDB_URL="${SCION_TEST_CRDB_URL:-postgresql://root@localhost:26257/defaultdb?sslmode=disable}"
export SCION_TEST_S3_ENDPOINT="${SCION_TEST_S3_ENDPOINT:-http://localhost:9000}"
export SCION_TEST_S3_BUCKET="${SCION_TEST_S3_BUCKET:-scion-test}"
export SCION_TEST_S3_ACCESS_KEY="${SCION_TEST_S3_ACCESS_KEY:-minioadmin}"
export SCION_TEST_S3_SECRET_KEY="${SCION_TEST_S3_SECRET_KEY:-minioadmin}"
export SCION_TEST_S3_REGION="${SCION_TEST_S3_REGION:-us-east-1}"
"$dir/target/release/examples/landrace" --backend "$backend" "$@"
