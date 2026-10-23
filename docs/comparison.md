# How TreeYard compares

TreeYard (binary `treeyard`) is a server under your agent orchestrator, not one more
orchestrator. This page compares it with the tools teams use today. The facts about other tools
come from research done on 2026-10-17 (each page was opened on that date, links below). Numbers about TreeYard come from
[benchmarks.md](benchmarks.md). Where another tool is the better choice, this page says so.

## In one table

| | git worktrees (+ an orchestrator) | GitButler | Jujutsu (jj) | Merge queue (GitHub, Mergify, Graphite, Aviator) | TreeYard |
|---|---|---|---|---|---|
| Where it runs | each agent's machine | desktop app, one directory | local client | SaaS on the forge | one self-hosted server for all machines |
| Agent's workspace | full checkout per agent | virtual branches in one shared directory | one working copy per workspace | not its job | API without checkout, or a copy-on-write directory |
| Sees other agents' edits before merge | no | inside one directory only | no | no | yes: intents, trial-merge collisions, events |
| Conflict | at merge or rebase, as markers | in the app | recorded in the commit, rebase never fails | the PR leaves the queue | typed task with base/ours/theirs and who changed it |
| Landing | push + PR, or local merge | the app commits | local | serial or batched, CI per entry | server-side rebase + merge, no global lock; queue or optimistic test modes |
| Git for people | yes | yes | git-compatible | yes | yes: `git clone/pull/push` on `/git`, mirror to GitHub |
| Licence | GPL-2.0 (git) | FSL-1.1-MIT (not OSI) | Apache-2.0 | closed SaaS | Apache-2.0 |
| Price | free | free client | free | GitHub: private repos need Enterprise Cloud; Mergify Max $21/seat/mo, Graphite Team $40/user/mo, Aviator Team $20/dev/mo | free, self-hosted |

## git worktrees with an orchestrator

Claude Squad, Conductor, Emdash, Vibe Kanban, Superset and Claude Code's own `--worktree` all give
each agent a `git worktree`. That is simple and every tool supports it.

What hurts at 10 or more agents (sources in the research file):

- Late conflicts: 27.67 % of 107K+ agent pull requests have a merge conflict (arXiv 2604.03551,
  2026-04-04); cross-agent pairs conflict at 41.7 % (arXiv 2607.04697, 2026-07-06). Both are
  preprints, not peer reviewed.
- Lost work: Claude Code issues #46444 (worktree cleanup deleted 10 days of uncommitted work),
  #75490, #60604, #102465 (one agent ran `git stash` over another's work).
- Setup and disk per worktree: a Cursor forum user saw 9.82 GB for 2 worktrees of a 2 GB codebase.

Measured on one machine (15-core CPU, 48 GB RAM, APFS, 2026-10-17, [benchmarks.md](benchmarks.md)):
50 ripgrep worktrees ready to build took 69.66 s and 7 789 MB; 50 TreeYard workspaces took 1.22 s
and 48.3 MB. On replayed history, 26 % to 37 % of changes conflicted at merge time.

Choose plain worktrees when you run fewer than about 5 agents, on one machine, and conflicts are
rare. They need no server.

## GitButler

GitButler puts several virtual branches in one working directory and has an agents tab and Claude
Code hooks (blog.gitbutler.com/agents-tab). Trigger.dev moved from worktrees to GitButler for
parallel Claude Code, and also wrote that "overlapping areas can definitely lead to problems
related to races and interference" (trigger.dev/blog/parallel-agents-gitbutler, 2026-04-16).

GitButler is a strong choice for one person with a few agents on one machine who wants a good UI.
TreeYard is for many agents on many machines: it keeps every agent's change on a server, merges
there, and tells each agent about collisions. GitButler's licence is FSL-1.1-MIT (MIT after two
years); TreeYard is Apache-2.0.

## Jujutsu (jj)

jj records conflicts in commits, so a rebase never fails, and it snapshots the working copy when a
jj command runs (docs.jj-vcs.dev/latest/conflicts). Some users run agents with jj to avoid lost
work (panozzaj.com, 2025-11-22). Codex users ask for jj workspace support (openai/codex #26648).

jj is a client. It has no server that sees all agents, no agent API, and no landing service.
TreeYard borrows two of its ideas: changes with stable ids instead of branches, and conflicts as
data. A jj backend for TreeYard is an open question (see the design doc).

Choose jj when people want a better local VCS. Use TreeYard under the agents.

## Merge queues

GitHub merge queue groups pull requests and runs CI per group. It is available "in any public
repository owned by an organization, or in private repositories owned by organizations using GitHub
Enterprise Cloud" (github/docs, merge-queue reusable). Mergify, Graphite, Aviator and Trunk sell
queues per seat. A local FIFO queue with a 3 to 4 minute check caps at "well under 20
landings/hour" (claude-code-merge-queue README).

Measured with each repo's own CI, a serial queue at 50 agents makes the last change wait 248 s
(ripgrep), 1 414 s (astro) and 3 614 s (requests). TreeYard lands in 2.3 ms to 43 ms p95, but
**without CI on the land path**. To test, use `landing.mode = "queue"` (batch test and bisect, like
a merge queue) or `"optimistic"` (land at once, test main after, bisect and revert). Anthropic
reports the same choice for its own agents: post-merge tests instead of a global lock
(anthropic.com/engineering/redesigning-source-control, 2026-10-13).

Choose a hosted merge queue when every change must pass full CI before it reaches main, and you
pay for the forge plan. TreeYard's queue mode does the same batch idea on your server.

## Early-warning tools

Clash (in-memory `merge-tree` over local worktrees), git-kura (path seals), Foremerge (semantic
intents) and git-stint show the demand for early conflict data. Each works on one machine;
Foremerge says "Coordination across machines is out of scope". TreeYard does the same on a
server for every machine, and measures its own precision: trial-merge warnings 0.395 precision at
0.975 recall (ripgrep, 16 agents, [benchmarks.md](benchmarks.md#early-warnings-precision-next-to-recall)).

## Scion and its open-source versions

Anthropic described Scion, its internal version control system, on 2026-10-13 ("Redesigning source
control"). The server does history, blame, merge and rebase. Clients fetch only the paths they need.
The unit of work is a changeset, and a ref move is a compare-and-swap. There is no global merge
queue: tests after the merge find conflicts, and Claude fixes them. Scion is not public, and the post
says it is not a general replacement for git.

Two open-source projects follow that design: CarterT27/scion (Rust, Apache-2.0, gRPC over TLS,
CockroachDB and S3-compatible storage) and OpenScion (Rust, GPL-3.0). They aim at what Scion aims at:
checkout and storage for very large monorepos with many CI jobs. CarterT27/scion also lands through an
API: it rebases a stack onto the ref and moves the ref with a compare-and-swap, with retries. It lists
git protocol compatibility and two-way git sync as out of scope. At small size the extra tiers cost
time: on a real repository with 28,068 files, CarterT27/scion reports 14.5 s for a full checkout and
2.5 to 3 s for git (its README, measured 2026-10-14).

TreeYard shares the model: changesets and rebase on the server, agents as the main clients, `land`
as one call. It aims at another problem: many agents that write to one branch at the same time. It
adds the parts that Scion leaves to tests after the merge: trial-merge conflict warnings while agents
work, and queue or optimistic test modes at land. It is one binary with SQLite and a normal git
repository, so people keep `git clone/pull/push`. It has no sparse or virtual checkout and no storage
tier for a 100 GB monorepo. At that size, choose the Scion design.

## Sapling and Mononoke

Mononoke is Meta's server with server-side rebase; the Sapling docs say the "server and virtual
filesystem are not yet available publicly". Like Scion, it proves the model of a server-first VCS.

## What TreeYard does not do yet

- One node only: no virtual file system, no cache between nodes, no Postgres or S3 storage.
- No TLS (put it behind a TLS proxy) and no token expiry.
- Two-way workspace sync is not built: a workspace sends edits to the server, the server does not
  push other agents' edits into it.
- Real-agent runs used Claude Code only ([benchmarks.md](benchmarks.md)). Codex and OpenCode have not
  run against it in a measured test.

## Sources

All opened 2026-10-17. arXiv 2604.03551 (https://arxiv.org/abs/2604.03551) · arXiv 2607.04697
(https://arxiv.org/abs/2607.04697) · Claude Code issues #46444, #75490, #60604, #102465
(https://github.com/anthropics/claude-code/issues) · Cursor forum
(https://forum.cursor.com/t/windows-request-to-disable-automatic-worktree-creation-critical-disk-space-issue/146189)
· GitButler (https://blog.gitbutler.com/agents-tab) · Trigger.dev
(https://trigger.dev/blog/parallel-agents-gitbutler) · jj (https://docs.jj-vcs.dev/latest/conflicts/) ·
panozzaj (https://www.panozzaj.com/blog/2025/11/22/avoid-losing-work-with-jujutsu-jj-for-ai-coding-agents/)
· openai/codex #26648 (https://github.com/openai/codex/issues/26648) · GitHub merge queue
(https://github.com/github/docs/blob/main/data/reusables/gated-features/merge-queue.md) · Mergify
(https://mergify.com/pricing) · Graphite (https://graphite.com/pricing) · Aviator
(https://www.aviator.co/pricing) · claude-code-merge-queue (https://github.com/funador/claude-code-merge-queue)
· Clash (https://github.com/clash-sh/clash) · Foremerge (https://github.com/naw103/foremerge) ·
Sapling (https://sapling-scm.com/docs/introduction/) · Anthropic
(https://anthropic.com/engineering/redesigning-source-control). Opened 2026-10-23: CarterT27/scion
(https://github.com/CarterT27/scion) · OpenScion (https://github.com/tinyhumansai/openscion).
