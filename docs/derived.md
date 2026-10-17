# Derived data: history, blame, search, symbols, bisect

Agents and CI ask history questions all day: who wrote this line, what changed under this
directory, where is this function, which change broke the tests. TreeYard answers them on the
server, without a checkout, from derived data that it computes from the changesets and keeps.

## Operations

| Operation | What | Derived data |
|---|---|---|
| `log` | history of main (or `at`), newest first; filters `path` (file or directory), `paths` (globs), `agent`, `since`, `until` | mainline heights and a changed-path index (`touch`: each changed file and each parent `dir/`) |
| `blame` | per line: the changeset (agent, model, session, intent) or, before the import, the git commit; `start`/`end` for a range | blame per (changeset, path), computed from the parent's blame with one line diff |
| `grep` | regex per line, same results and order as the scan; `index: false` scans | trigram set per blob; an in-memory posting index of main |
| `symbols`, `outline` | definitions by name or prefix (`kind`, `path` filters); every definition of a file | tree-sitter symbols per (blob, language) |
| `bisect` | first bad changeset between `good` and `bad` (default main) | none: temporary checkouts |

Every derived row is keyed by content (a blob id, or a changeset id and a path), so a row never goes
stale. A new revision only adds rows for its new blobs and paths. The rows live in the SQLite file
of the data directory and survive a restart.

`[index]` in `treeyard.toml` picks when the search and symbol rows are made: `lazy` (default: on
the first `grep` or `symbols` call), `eager` (a background thread follows main), or `off` (grep
scans). The history index follows main on every query. Blame rows are made on the first `blame` of
a file version. Heavy work (reading blobs aside) runs outside the repo lock, in chunks.

## Import with history

`treeyard init --data D --from REPO --history N` imports the last N first-parent commits as
changesets. Each one maps to its own git commit, so `log`, `blame` and `bisect` reach into real
history and commit ids stay the same. Lines older than the oldest imported commit are blamed by
`git blame --first-parent` on that commit: blame continues into everything git holds.

## Blame equals `git blame --first-parent`

The line diff is a port of git's xdiff Myers diff (`src/xdiff.rs`): trim of the common tail,
removal of unmatched and too-common lines, `xdl_split` with git's cost limits, and
`xdl_change_compact` with the indent heuristic. A file that a changeset adds follows a rename the
way `git blame` does: the candidate sources are the files that changeset deleted, and the only
destination is the blamed path. Two differences from `git blame` remain by design: blame follows
first parents only, and it does not look for moved or copied lines (`-M`, `-C`).

Checks (commands run 2026-10-17, 15-core CPU, 48 GB RAM, macOS, release build):

| Repo, revision | Imported commits | Files checked | Lines equal to git | `blame` cold p50 / p95 | `blame` warm p50 | `git blame --first-parent` p50 / p95 |
|---|---|---|---|---|---|---|
| go `dfc6f7db9bd7` (16 108 files) | 20 000 | 400 of 400 | 102 565 of 102 565 | 35.06 / 463.16 ms | 0.72 ms | 249.3 / 742.02 ms |
| ripgrep `3fce3b5bb023` | 2 000 | 226 of 226 | 73 691 of 73 691 | 15.85 / 46.57 ms | 0.58 ms | 24.76 / 33.96 ms |
| requests `611c6162cbc4` | 2 000 | 127 of 127 | 30 631 of 30 631 | 4.94 / 52.28 ms | 0.63 ms | 22.59 / 41.71 ms |

Command: `python3 bench/derived.py --repo <clone> --dir <scratch> --history <N> --files 400`
(files = every file at HEAD that the imported history touched, sampled with seed 1, at most 400).
"Cold" is the first `blame` of a file over HTTP; "warm" is the second. `git blame` runs on the
same clone. Hunk check of the diff alone: `cargo test --release --test xdiff_git -- --ignored`
printed "1106 file pairs, 0 differ" (ripgrep, last 500 commits) and "1538 file pairs, 0 differ"
(go, `TREEYARD_REAL_REPO=<go clone> TREEYARD_COMMITS=400`): every hunk equals `git diff -U0`.

What the checks found on the way: with imara-diff 0.2 (the first choice) only 90 of 100 ripgrep
files had the same blame as git. The port without git's `trim_common_tail` still differed from
`git diff -U0` on 3 of 1106 file pairs. Rename detection through `git diff-tree -M` over the whole
tree paired a source with another destination (go `src/simd/archsimd/cpu_arm64.go`). `git log
--follow` made it worse (348 of 400 go files), because `--follow` also finds copies from unchanged
files.

## History index

Same command, `log` of 12 sampled directories, limit 50, over HTTP:

| Repo | `log` with the index | the same walk without it (`scan: true`) | `git log --first-parent -n50 -- <dir>` |
|---|---|---|---|
| go, 20 000 commits | 2.7 to 8.48 ms | 8.93 to 124.64 ms | 29.72 to 671.01 ms |
| ripgrep, 2 000 | 0.82 to 1.63 ms | 1.9 to 8.45 ms | 19.02 to 31.41 ms |
| requests, 2 000 | 0.87 to 3.95 ms | 1.43 to 8.08 ms | 18.87 to 37.45 ms |

The index and the scan return the same entries in every case, and they equal `git log`'s commits
(the import changeset, which stands for all older history, left out). A directory that almost every
commit touches gains nothing (requests `src`: 1.6 ms with the index, 1.54 ms without). Tests:
`log_index_equals_scan_and_git_log` in `tests/derived.rs`.

## Trigram search

The plan for a regex is an AND/OR of trigrams from its literals (case-insensitive letters expand
up to 64 variants; `.*`, `\w+` and other wide classes give no trigrams, so such a pattern scans).
Candidates come from the posting lists of main; a revision other than main uses main's index plus
its changed files (up to 4096, else a scan). The regex then runs line by line on the candidates
only, so results equal the scan.

go `dfc6f7db9bd7`, 15 311 text files, `grep` with `max` 100 000 over HTTP (index: best of two
calls; scan: `index: false`; git: `git grep -n -E` on the checkout, all cores):

| Pattern | Matches | Candidates | Index | Scan | `git grep` |
|---|---|---|---|---|---|
| `func \(s \*Server\)` | 68 | 27 | 5.41 ms | 656.21 ms | 521.79 ms |
| `import \"net/http\"` | 5 | 23 | 5.15 ms | 711.25 ms | 497.14 ms |
| `errors\.New\(\"[a-z]+ not found` | 4 | 398 | 17.01 ms | 389.04 ms | 548.48 ms |
| `[A-Z][a-z]+Handler` | 781 | 402 | 57.04 ms | 548.11 ms | 634.63 ms |
| `(?i)deprecated` | 1 975 | 663 | 79.1 ms | 691.46 ms | 481.64 ms (`-i`) |
| `panic\(` | 7 950 | 2 365 | 98.24 ms | 717.32 ms | 517.09 ms |
| `\bunsafe\b` | 19 078 | 1 735 | 100.0 ms | 396.8 ms | no match (ERE has no `\b`) |
| `TODO\|FIXME` | 4 510 | 1 445 | 137.1 ms | 739.05 ms | 663.92 ms |
| `fn\s+main` | 0 | 4 914 | 216.86 ms | 838.54 ms | 615.46 ms |
| `Copyright 20[0-9][0-9]` | 11 932 | 11 857 | 393.92 ms | 920.53 ms | 510.52 ms |

Median speed-up over the scan: 8.0 times (go), 10.8 (ripgrep), 15.6 (requests). Every pattern gave
the same matches with and without the index. The first `grep` on go built the index in 1.987 s
(trigram rows for every blob, on all cores, then the posting lists: 106 MB in memory). A pattern
that most files contain (`Copyright`) gains little: reading the candidate blobs is the cost.

## Symbols

Tree-sitter grammars with their tags queries: Rust, Python, Go, JavaScript, TypeScript, TSX (plus
constants and type aliases for Rust and Go). Kinds: `function`, `method`, `class` (Rust structs and
enums too), `interface` (Rust traits too), `module`, `type`, `macro`, `constant`. Intents with
`path#name` use the same definitions when the language is known (else the old regex).

go: the first `symbols` call parsed every source blob in 2.76 s; then lookups by name took p50
3.22 ms, max 7.85 ms (8 names, up to 2 159 hits for `main`). Tests:
`symbols_are_found_at_main_and_in_an_open_change`, `symbols::tests::every_language_finds_its_definitions`.

## Bisect

`bisect {good, bad?, command?, verify?}` walks the first-parent chain from `bad` back to `good`,
tests both ends (unless `verify: false`), and halves the range. One temporary checkout moves from
step to step: only changed files are written, files the test changed are restored, and other files
the test made (build output) stay, as in `git bisect`. A rewritten file gets an mtime at least one
second newer, because same-size edits in the same second looked unchanged to Python's `.pyc` check
(the first real run blamed the wrong change). Commands come only from `treeyard.toml`
(`[bisect] test`, named `[bisect.commands]`, else `landing.test`): agents pick one by name and never
send a command of their own. Then `revert` removes the culprit.

requests `611c6162cbc4`: 40 agents landed through the API, agent 27 swapped user and password in
`get_auth_from_url`; `bisect` with the repo's own `tests/test_utils.py` found agent 27 in 7 tests
(3.24 s), `revert` landed, and a second `bisect` answered "the test passes on bad". Command:
`python3 bench/derived.py --repo <requests clone> --dir <scratch> --history 2000 --only bisect
--bisect-test "PYTHONPATH=src <venv>/bin/python -m pytest -q -x -p no:cacheprovider tests/test_utils.py"`
(the venv holds pytest 9.1.1 and the requests dependencies). Test with no Python:
`bisect_finds_a_planted_bad_change_and_revert_removes_it` (40 changes, at most 8 tests).

## Limits

- Blame follows first parents and whole-file renames only. A blame of a huge file far from its
  last cached version diffs every step once (go max 3.1 s cold, then cached).
- The posting index lives in memory for main (106 MB for go). After a restart it is rebuilt from
  the SQLite rows, without blob reads (time not measured).
- Trigram rows are per blob for every revision ever indexed; nothing removes old rows yet.
- Symbols cover definitions, not references. Languages beyond the six above give no symbols.
