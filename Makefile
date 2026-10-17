# Checks run locally (no GitHub Actions on appnice repos).
export PATH := /opt/homebrew/opt/rustup/bin:$(HOME)/.cargo/bin:$(PATH)

.PHONY: build test check fmt release

build:
	cargo build

test:
	cargo test

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test

fmt:
	cargo fmt

release:
	cargo build --release
