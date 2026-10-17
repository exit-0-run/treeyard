# Contributing

Thank you for your help. This file tells you how to build, test and send a change.

## Build and test

1. Install Rust (edition 2024, rustc 1.85 or newer) and git 2.44 or newer.
2. Run `make check`. It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, the
   Python unit tests of `bench/`, and `cargo test`. It must pass before you send a change.
3. Run `make release` for `target/release/treeyard`.

Useful extra checks:

- `sh scripts/check-quickstart.sh docs/quickstart.md` runs the quickstart in an empty directory.
- `examples/swarm-demo.sh --agents 10` compares scripted agents on git worktrees and on the server.
- `TREEYARD_FUZZ_SEEDS=300 cargo test --release --test fuzz_history` runs more random histories.
- `make test-linux` runs the Linux clone paths in a privileged Docker container.

There is no hosted CI. Checks run on your machine.

## Rules for code

- Git is the oracle. A merge, an export, a blame or a diff must give git's answer. Add a test that
  compares with git (`git merge-tree`, `git blame --first-parent`, `git diff`), not only a fixed value.
- Keep commits deterministic: dates and author come from the changeset, never from the clock.
- All repo state goes through `Repo::with`. Keep slow work (tests, index builds, long polls) outside it.
- Add an operation in one place only: the table in `src/rpc.rs` (dispatch and `tools()`). Then run
  `cargo run -q -- api-doc > docs/api.md`, add the name to a group in `src/apidoc.rs`, and name it
  in `docs/spec/rpc.md`. `tests/docs.rs` checks all three.
- No new dependency without a reason in the pull request. Pin a version in `Cargo.toml`.

## Rules for docs

- Every number in a doc comes from a command you ran. Put the command, the machine and the date
  next to it. `docs/benchmarks.md` is the place for raw numbers.
- Write in plain, short sentences (we follow ASD-STE100 Simplified Technical English where we can):
  one idea per sentence, active voice, the same word for the same thing.
- A ```` ```sh ```` block in `docs/quickstart.md` must run as is. Use ```` ```text ```` for
  commands that need other tools or a placeholder.

## Send a change

1. Fork, make a branch, commit with a clear message.
2. Run `make check`.
3. Open a pull request. Say what you changed, why, and how you tested it.

By sending a contribution, you agree that it is licensed under Apache-2.0, the licence of this
project. Report security problems privately, as `SECURITY.md` says.
