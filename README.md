<p align="center">
  <img src="assets/wordmark.svg" alt="TreeYard" width="300">
</p>

<p align="center">
  <img alt="Licence: Apache-2.0" src="https://img.shields.io/badge/licence-Apache--2.0-2f6b45">
  <img alt="Version 0.1.0" src="https://img.shields.io/badge/version-0.1.0-2f6b45">
  <img alt="Rust 2024" src="https://img.shields.io/badge/rust-2024_edition-b7410e?logo=rust&logoColor=white">
  <img alt="Platforms: macOS, Linux" src="https://img.shields.io/badge/platform-macOS_%7C_Linux-4a5560">
  <img alt="MCP server" src="https://img.shields.io/badge/MCP-server-5b4fc4">
  <img alt="Git compatible" src="https://img.shields.io/badge/git-compatible-f05032?logo=git&logoColor=white">
  <img alt="Single binary" src="https://img.shields.io/badge/single-binary-4a5560">
  <img alt="Agents: 10-200" src="https://img.shields.io/badge/agents-10%E2%80%93200-d99a1e">
  <img alt="Self-hosted" src="https://img.shields.io/badge/self--hosted-yes-2f6b45">
</p>

<h3 align="center">Git for agent swarms. One server for every agent. No merge hell.</h3>

<p align="center">
  <a href="#benchmarks"><img src="assets/results.svg" alt="TreeYard vs plain git worktrees: 57x faster workspaces, 161x less disk, 13x faster on astro, 43x faster blame, 111 to 0 thrown-away changes, no queue wait" width="900"></a>
</p>

<p align="center"><b>Also:</b> 100% of conflicts reported before land, not at merge time · 8× faster grep on a 15 311-file repo · 1 binary, git underneath</p>

Your agents are fast. Git makes them wait. TreeYard is a version control server for 10 to 200 AI coding
agents on one repository. Agents get a workspace in about a second, see each other's edits while they work,
and land in milliseconds. Git stays the source of truth, so people keep `git clone`, `git pull`, `git push`
and GitHub.

## The problem

When 10 or more agents work on one repository with `git worktree`:

- Each agent gets a full checkout. Each agent must build the code or install the dependencies again.
- Agents do not see the edits of other agents. Conflicts show only at merge time, and agents lose work.
- Changes land one at a time through a merge queue that runs CI for each change.

## What TreeYard does

- Agents read, search and edit the repository through one API: HTTP, MCP or the CLI.
- An agent opens a *change*, applies edits and lands the change. The server does the rebase and the merge.
- A conflict comes back as a task: the base, the two sides, and the agent that changed the path.
- The server compares each edit with the other open changes. It tells the agent about an overlap before
  the change lands.
- Each landed change becomes a normal git commit. People use `git clone`, `git pull` and `git push`.
- An agent that must build gets a copy-on-write workspace. The workspace shares build output and
  dependencies with the base, so the agent does not build again.
- The server answers history questions: `blame`, `log`, indexed `grep`, `symbols` and `bisect`.

Status: v0.1. One node, one binary. There is no TLS and no token expiry yet. Threat model, controls and
limits: [SECURITY.md](SECURITY.md).

## Quick start

```sh
make release
treeyard init --data /tmp/rs --from /path/to/repo --branch main
treeyard serve --data /tmp/rs                      # http://127.0.0.1:7420

treeyard call open_change '{"agent": "a1", "intent": "fix a typo"}'
treeyard call apply '{"change": "ch_…", "edits": [{"path": "README.md", "replace": {"old": "teh", "new": "the"}}]}'
treeyard call land '{"change": "ch_…"}'

git clone http://127.0.0.1:7420/git repo            # the landed commit is on main
```

Connect an agent through MCP:

```sh
claude mcp add treeyard -- treeyard mcp --url http://127.0.0.1:7420 --agent claude-1
```

Give an agent a workspace that is ready to build:

```sh
treeyard daemon --spares 2 &
treeyard workspace create fix-1 --agent claude-1 --git --setup "cargo build -q"
cd "$(treeyard workspace path fix-1)"              # use any editor or tool, run the tests
treeyard workspace land fix-1
```

All operations, configuration and the data model: [docs/reference.md](docs/reference.md).

## Benchmarks

> **Measured results, not estimates.** One machine: 15-core ARM64 CPU (5 performance + 10 efficiency cores), 48 GB RAM, 1 TB NVMe SSD, APFS, macOS 26.6; no GPU use, 2026-10-17, one run per
> configuration (seed 1). Repositories: ripgrep `3fce3b5`, requests `611c6162`, astro (pnpm monorepo),
> go `dfc6f7db9bd7`, linux `58161ead29e5`. Commands, method and raw data: [docs/benchmarks.md](docs/benchmarks.md).

| Measurement (50 agents) | Plain git | TreeYard | Gain |
|---|---|---|---|
| 50 workspaces ready to build, ripgrep | 69.66 s, 7 789 MB | 1.22 s, 48.3 MB | **57× faster, 161× less disk** |
| 50 workspaces ready to build, astro | 352.53 s, 5 859 MB | 85.30 s, 2 197 MB | **4.1× faster, 2.7× less disk** |
| Same, astro, from the warm pool | 352.53 s, 5 859 MB | 26.19 s, 1 506 MB | **13× faster, 3.9× less disk** |
| Wait of the last change in a serial queue with the repository's CI (ripgrep / astro / requests) | 248 s / 1 414 s / 3 614 s | land p95 3.4 / 43 / 2.3 ms | **no queue wait** (CI runs after land, see limits) |
| Changes that conflict at merge time (500 real commits, stale bases) | 26 % to 37 % | the same conflicts | **all reported before land** |
| Changes thrown away (replay, ripgrep, 16 agents) | 111 | 0 with declared intents | **111 to 0** (upper bound) |
| `blame`, ripgrep, p50 | 24.76 ms | 15.85 ms cold, 0.58 ms warm | **1.6× faster cold, 43× warm**, same lines |
| `grep`, go (15 311 files) | 482 to 664 ms | 5.15 to 393.92 ms | **8× faster (median)**, same matches |
| Land on linux (96 080 files), p50, 50 in a row | 183.7 to 201.14 ms (commit + merge-tree) | 0.91 ms | **about 200× faster** |

Limits of these numbers:

- TreeYard land time does not include CI. To run tests, set `landing.mode` to `queue` or `optimistic`.
- The replay declares the exact paths of each commit. Real agents guess, so 0 thrown-away changes is an
  upper bound.
- Overlap warnings have a precision of 0.40 to 0.54. About half the warnings are for changes that merge
  without a conflict.
- With 50 agents on stale bases and no intents, the final tree is different from the history. A merge
  cannot see that a fix landed before the bug it fixes.

## Not yet

Two-way workspace sync, a virtual file system, persistent events, blame of moved lines, symbol references,
token expiry, TLS, Postgres and S3 storage.

## Licence

Apache-2.0.
