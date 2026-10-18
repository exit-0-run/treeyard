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
  <a href="#benchmarks"><img src="assets/hero/workspaces.svg" alt="57x faster: 50 workspaces ready to build on ripgrep, git 69.7 s, TreeYard 1.2 s" width="640"></a>
  <a href="#benchmarks"><img src="assets/hero/disk.svg" alt="161x less disk: 50 workspaces on ripgrep, git 7.8 GB, TreeYard 48 MB" width="640"></a>
  <a href="#benchmarks"><img src="assets/hero/blame.svg" alt="43x faster blame on ripgrep, git 24.8 ms, TreeYard 0.6 ms" width="640"></a>
  <a href="#benchmarks"><img src="assets/hero/astro.svg" alt="13x faster: 50 workspaces with node_modules on astro, git 353 s, TreeYard 26 s" width="640"></a>
  <a href="#benchmarks"><img src="assets/hero/thrown-away.svg" alt="Changes thrown away with 16 agents: git 111, TreeYard 0" width="640"></a>
</p>

<p align="center"><b>Also:</b> land a change in 3 ms with no merge queue* · 99–100% of conflicts reported before land · 8× faster grep · 1 binary, git underneath</p>

<p align="center"><sub>Measured on 15-core CPU, 48 GB RAM, one run per configuration. * Land time excludes CI: tests run on main
after land, or in a tested queue. 111 → 0 is an upper bound: the replay declares the exact paths of each change.</sub></p>

Your agents are fast. Git makes them wait. TreeYard is a self-hosted version control server for 10
to 200 AI coding agents on one repository. Agents get a workspace in about a second, see each
other's edits while they work, and land in milliseconds. Git stays the source of truth, so people
keep `git clone`, `git pull`, `git push` and GitHub.

## The pains

What hurts when many agents work on one repository with `git worktree` (sources:
[docs/comparison.md](docs/comparison.md); our numbers: [docs/benchmarks.md](docs/benchmarks.md)):

1. **Conflicts show up too late.** 27.67 % of 107K+ agent pull requests have a merge conflict
   (arXiv 2604.03551). On 500 replayed commits, 26 % to 37 % of changes conflicted at merge time.
2. **Work gets lost.** Worktree cleanup and one agent's `git stash` or `git clean` delete another's
   work (Claude Code issues #46444, #75490, #102465).
3. **Every agent sets up a full checkout.** 50 ripgrep worktrees ready to build: 69.66 s and 7 789 MB.
4. **Landing is serial.** With each repo's own CI, a queue makes the 50th change wait 248 s to
   3 614 s. Hosted merge queues on private repos need a paid plan.

## What TreeYard does about them

- **See conflicts while you work.** Each `apply` lands every pair of open changes on today's main, in
  both orders, without storing anything, and reports `conflict` (with the reason) or `overlap` with line ranges. Agents declare intents (paths,
  symbols) and get overlaps at `open_change`. Events stream to every agent.
- **Lose nothing, undo anything.** A change keeps every revision on the server. A workspace syncs to
  its change about 300 ms after each edit, and `remove` sends the last edits before it deletes anything.
  `checkpoint` marks a point (a Claude Code hook marks one per turn); `restore_change` goes back to it
  and can itself be undone.
- **Many agents in one checkout.** Agents that share one directory still get one change each: every hunk
  has a stable id and one owner (`treeyard share`).
- **Start in about a second.** Agents that only edit need no checkout: they use the API. Agents that
  build get a copy-on-write clone of a built base, with `target/` or `node_modules` shared as a layer.
- **Land without a lock.** The server rebases and merges on `land`. A conflict comes back as a task
  with the base, both sides and who changed the path. Optional modes test before (`queue`) or after
  (`optimistic`) the land, with bisect and revert.
- **Workspaces that clean up after themselves.** A repo file `.treeyard/workspace.toml` sets setup,
  teardown, layers, `.env` files to copy and ports. `workspace gc` keeps count and disk under a limit and
  never deletes unsynced work. Sparse workspaces (`--paths`) and a mirror into one fixed directory.
- **Best of N, notes that travel.** Run several agents on one task (`group`) and land one; attach the
  agent's transcript to its change, and it becomes a git note on the landed commit.
- **Keep git.** Every land is one deterministic git commit. People clone, pull and push over HTTP;
  `treeyard mirror` pushes `main` to GitHub.
- **Ask the server.** `blame` (equal to `git blame --first-parent`), `log`, indexed `grep`,
  tree-sitter `symbols`, server-side `bisect`.

## Live run: 8 Claude Code agents fix 8 real bugs

8 Claude Code agents fix 8 real bugs from the history of more-itertools at once, from the same base
commit. Hidden checks, which the agents never see, test every fix on the final main.

| | git worktrees + merge queue | TreeYard |
|---|---|---|
| All 8 fixes landed after | 74.3 s | **48.2 s** |
| Fixes that work on main (hidden checks) | 8 / 8 | 8 / 8 |
| A change that failed a test | caught by the queue before the merge | reverted about 1 s after the land; the agent got the verdict and landed its fix |
| Agent turns, API-equivalent cost | 66, $0.45 | 66, $0.46 |

The earlier runs of this demo found three bugs in TreeYard; all are fixed and tested. All runs, and how
to repeat them: [docs/benchmarks.md](docs/benchmarks.md#live-agents-8-claude-code-agents-fix-8-real-bugs-2026-10-18),
`examples/live-agents/`.

## See it: 50 scripted agents, git worktrees vs TreeYard

`examples/swarm-demo.sh` gives N scripted agents the same work twice. Each agent adds a file and
edits its own line of a shared file; 5 pairs edit the same line (a real conflict). First every
agent gets a `git worktree`, commits, and a serial queue lands each branch (`git rebase` +
`git merge --ff-only`, no CI). Then every agent opens a change on TreeYard, applies the same edits,
and calls `land` at the same time. A conflicted agent redoes its edit on the new main on both sides.
Run on 15-core CPU, 48 GB RAM: `examples/swarm-demo.sh --agents 50 --pairs 5` (50 agents, 2 000 files,
5 same-line pairs).

| Step                                      | git worktrees       | TreeYard           |
|-------------------------------------------|---------------------|--------------------|
| Give 50 agents a place to work (s)        | 8.06                | 1.22               |
| Extra disk for 50 agents (MB)             | 391.2               | 6.8                |
| Edit and commit, all agents at once (s)   | 2.85                | 2.07               |
| Land every change (s)                     | 3.71 (serial queue) | 0.88 (all at once) |
| Change is on main after, p50 / p95 (ms)   | 2021 / 3562         | 500 / 834          |
| Same-line conflicts warned before land    | 0 of 5              | 5 of 5             |
| Conflicts at land (agent redoes the edit) | 5                   | 5                  |
| Edits missing from main                   | 0 of 50             | 0 of 50            |

Read it fairly: the same 5 conflicts happen on both sides. The difference is that TreeYard warns
about all 5 before anyone lands, and nobody waits for a queue. The TreeYard agents have no
directory (API only); for agents that need one, see the workspace numbers below. Four more runs and
the method: [docs/benchmarks.md](docs/benchmarks.md#swarm-demo-scripted-agents-git-worktrees-vs-treeyard-task-015-2026-10-18).

## Benchmarks

> **Measured results, not estimates.** 15-core CPU, 48 GB RAM, one run per
> configuration (seed 1). Repositories: ripgrep `3fce3b5`, requests `611c6162`, astro (pnpm monorepo),
> go `dfc6f7db9bd7`, linux `58161ead29e5`. Commands, method and raw data: [docs/benchmarks.md](docs/benchmarks.md).

| Measurement (50 agents) | Plain git | TreeYard | Gain |
|---|---|---|---|
| 50 workspaces ready to build, ripgrep | 69.66 s, 7 789 MB | 1.22 s, 48.3 MB | **57× faster, 161× less disk** |
| 50 workspaces ready to build, astro | 352.53 s, 5 859 MB | 85.30 s, 2 197 MB | **4.1× faster, 2.7× less disk** |
| Same, astro, from the warm pool | 352.53 s, 5 859 MB | 26.19 s, 1 506 MB | **13× faster, 3.9× less disk** |
| Wait of the last change in a serial queue with the repository's CI (ripgrep / astro / requests) | 248 s / 1 414 s / 3 614 s | land p95 3.4 / 43 / 2.3 ms | **no queue wait** (CI runs after land, see limits) |
| Changes that conflict at merge time (500 real commits, stale bases) | 26 % to 37 % | the same conflicts | **98 % to 100 % reported before land** |
| Changes thrown away (replay, ripgrep, 16 agents) | 111 | 0 with declared intents | **111 to 0** (upper bound) |
| `blame`, ripgrep, p50 | 24.76 ms | 15.85 ms cold, 0.58 ms warm | **1.6× faster cold, 43× warm**, same lines |
| `grep`, go (15 311 files) | 482 to 664 ms | 5.15 to 393.92 ms | **8× faster (median)**, same matches |
| Land on linux (96 080 files), p50, 50 in a row | 183.7 to 201.14 ms (commit + merge-tree) | 0.91 ms | **about 200× faster** |

Correctness: 300 random histories gave 2 200 clean server merges, all equal to `git merge-tree`, and
0 server conflicts that git merged cleanly (`TREEYARD_FUZZ_SEEDS=300 cargo test --release --test
fuzz_history`, [docs/benchmarks.md](docs/benchmarks.md#hardening-checks-task-013-2026-10-17)).

Limits of these numbers:

- TreeYard land time does not include CI. To run tests, set `landing.mode` to `queue` or `optimistic`.
- The replay declares the exact paths of each commit. Real agents guess, so 0 thrown-away changes is an
  upper bound.
- Conflict warnings catch 99 % to 100 % of real conflicts, and 72 % to 90 % of warned pairs really
  conflict. The other warnings are false alarms.
- With 50 agents on stale bases and no intents, the final tree is different from the history. A merge
  cannot see that a fix landed before the bug it fixes.
- All agents in these measurements are scripts. No Claude Code, Codex or OpenCode session ran.

## Quick start

Install a release ([docs/install.md](docs/install.md)) or build with `make release`, then run this in
an empty directory. `sh scripts/check-quickstart.sh README.md` runs these blocks word for word; the
longer version with conflicts, workspaces and MCP is [docs/quickstart.md](docs/quickstart.md).

```sh
mkdir demo-repo
git -C demo-repo init -q -b main
printf 'hello wrold\n' > demo-repo/README.md
git -C demo-repo add README.md
git -C demo-repo -c user.name=me -c user.email=me@example.com commit -q -m "first commit"
treeyard init --data rs-data --from demo-repo
treeyard serve --data rs-data > serve.log 2>&1 &
SERVER=$!
sleep 1
```

An agent opens a change, edits without a checkout, and lands:

```sh
CH=$(treeyard call open_change '{"agent": "a1", "intent": "fix a typo"}' | sed -n 's/^  "id": "\(ch_[0-9a-f]*\)",$/\1/p')
treeyard call apply "{\"change\": \"$CH\", \"edits\": [{\"path\": \"README.md\", \"replace\": {\"old\": \"wrold\", \"new\": \"world\"}}]}" > /dev/null
treeyard call land "{\"change\": \"$CH\"}" | grep '"status"'
```

People see a normal git commit:

```sh
git clone -q http://127.0.0.1:7420/git people-clone
git -C people-clone log --format='%s | %an' -n 1
kill "$SERVER"
```

## Connect your agents

```text
claude mcp add --transport stdio treeyard -- treeyard mcp --agent claude-1
codex mcp add treeyard -- treeyard mcp --agent codex-1
```

OpenCode, `.mcp.json`, and Claude Code worktree hooks that make `claude --worktree` and subagents
use TreeYard workspaces: [docs/mcp.md](docs/mcp.md).

## Docs

| Doc | What |
|---|---|
| [docs/quickstart.md](docs/quickstart.md) | ten minutes, checked word for word, with its transcript |
| [docs/concepts.md](docs/concepts.md) | changes, landing, conflicts as tasks, workspaces, git |
| [docs/api.md](docs/api.md) | every operation, generated from the operation table |
| [docs/spec/rpc.md](docs/spec/rpc.md) | Agent Repo RPC v0: an open protocol for repo servers for agents |
| [docs/mcp.md](docs/mcp.md) | Claude Code, Codex, OpenCode |
| [docs/comparison.md](docs/comparison.md) | vs worktrees, GitButler, jj, merge queues |
| [docs/install.md](docs/install.md) | release archives, checksums, cross builds |
| [docs/workspaces.md](docs/workspaces.md), [docs/coordination.md](docs/coordination.md), [docs/git-http.md](docs/git-http.md), [docs/derived.md](docs/derived.md), [docs/access.md](docs/access.md) | the parts in detail |
| [docs/reference.md](docs/reference.md), [docs/benchmarks.md](docs/benchmarks.md), [SECURITY.md](SECURITY.md) | configuration, numbers, threat model |
| [CHANGELOG.md](CHANGELOG.md), [CONTRIBUTING.md](CONTRIBUTING.md) | history, how to help |

## Status and not yet

v0.1: one node, one binary, macOS and Linux. There is no TLS and no token expiry yet: keep the
server on loopback or behind a TLS proxy with tokens ([SECURITY.md](SECURITY.md)).

Not built yet:
- continuous two-way workspace sync: a workspace gets the server's version on request
  (`workspace update`, and on its own after a land that hits a conflict), not live;
- a virtual file system (a workspace holds its whole tree, or its `--paths` cone);
- persistent events (the event log is in memory);
- blame of moved or copied lines, and symbol references (only definitions);
- Postgres and S3 storage, and more than one node.

## Licence

Apache-2.0.
