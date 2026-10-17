# Checks run locally (no GitHub Actions on appnice repos).
# Rust comes from Homebrew rustup, which is not on the default PATH.
CARGO ?= $(shell command -v cargo 2>/dev/null || echo /opt/homebrew/opt/rustup/bin/cargo)
export PATH := $(dir $(CARGO)):$(PATH)

.PHONY: build test check fmt release bench bench-quick bench-test bench-workspaces bench-conflicts bench-queue bench-treeyard bench-history bench-cow test-linux

build:
	$(CARGO) build

test:
	$(CARGO) test

check: bench-test
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) test

fmt:
	$(CARGO) fmt

release:
	$(CARGO) build --release

# Phase-0 baseline (docs/benchmarks.md). Scratch, clones and results go to BENCH_DIR.
BENCH_DIR ?= /tmp/treeyard-bench
BENCH_REPOS ?= ripgrep requests astro
BENCH_N ?= 1,10,50
BENCH_PY = python3 bench/bench.py --dir $(BENCH_DIR)

bench-test:
	PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s bench

bench-workspaces:
	for r in $(BENCH_REPOS); do $(BENCH_PY) workspaces --repo $$r --n $(BENCH_N) > /dev/null || exit 1; done

bench-conflicts:
	for r in $(BENCH_REPOS); do $(BENCH_PY) conflicts --repo $$r --agents $(BENCH_N) > /dev/null || exit 1; done

bench-queue:
	for r in $(BENCH_REPOS); do $(BENCH_PY) queue --repo $$r --n $(BENCH_N) > /dev/null || exit 1; done

bench-history:
	for r in $(BENCH_REPOS); do $(BENCH_PY) history --repo $$r --n $(BENCH_N) > /dev/null || exit 1; done

bench-treeyard: release
	for r in $(BENCH_REPOS); do $(BENCH_PY) treeyard --repo $$r --n $(BENCH_N) --agents $(BENCH_N) > /dev/null || exit 1; done

# Phase-2 workspaces: TreeYard copy-on-write workspaces with a shared dependency layer
# (compare with bench-workspaces, same N and jobs).
bench-cow: release
	for r in $(BENCH_REPOS); do $(BENCH_PY) cow --repo $$r --n $(BENCH_N) > /dev/null || exit 1; done

# Linux code paths (reflink on XFS, overlayfs as root) in a privileged Docker container.
test-linux:
	sh bench/test-linux.sh

# Everything: about 3 hours on a 15-core CPU; about 70 GB of scratch at the peak (astro, 50 copies).
bench: bench-workspaces bench-conflicts bench-queue bench-history bench-treeyard
	$(BENCH_PY) report

# A few minutes: two repos, no dependency builds for 50 agents, no queue.
bench-quick: release
	$(BENCH_PY) workspaces --repo ripgrep --n 1,10 > /dev/null
	$(BENCH_PY) workspaces --repo requests --n 1,10,50 --setup checkout,uv > /dev/null
	$(BENCH_PY) conflicts --repo requests --commits 200 --agents 1,10,50 --seeds 1 > /dev/null
	$(BENCH_PY) treeyard --repo requests --commits 200 > /dev/null
	$(BENCH_PY) report
