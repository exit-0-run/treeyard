# Checks run locally (no GitHub Actions on appnice repos).
# Rust comes from Homebrew rustup, which is not on the default PATH.
CARGO ?= $(shell command -v cargo 2>/dev/null || echo /opt/homebrew/opt/rustup/bin/cargo)
export PATH := $(dir $(CARGO)):$(PATH)

.PHONY: build test check fmt release

build:
	$(CARGO) build

test:
	$(CARGO) test

check:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) test

fmt:
	$(CARGO) fmt

release:
	$(CARGO) build --release
