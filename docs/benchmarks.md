# Benchmarks: phase-0 baseline (plain git worktrees vs TreeYard)

Measured 2026-10-17 by `bench/bench.py` (stdlib Python, driven by `make bench`). Every number below comes
from the result JSON of the runs listed in [How to reproduce](#how-to-reproduce). The tables in
[Raw numbers](#raw-numbers) are the unchanged output of `python3 bench/bench.py --dir $BENCH_DIR report`.

## Result

At 50 agents, the serial merge queue costs the most wall time on all three repos. Late conflicts cost the
most work. The two pains add up, because a change that conflicts goes back to its agent and through the
queue again. Workspace setup is third. History queries are last.

| Pain (RFC phase 0) | 50 agents, plain git | TreeYard today |
|---|---|---|
| Serial merge queue wait | last change waits 248 s (ripgrep), 1414 s (astro), 3614 s (requests); 705, 125 and 49 landings per hour | land p95 3.4 ms (ripgrep), 2.3 ms (requests), 43 ms (astro); no CI on the land path |
| Late conflicts | 26 % to 37 % of changes conflict at merge time; 98 % to 100 % of them shared a path with another open change before the merge | the same conflicts, but `overlaps` names 139 of 139 (astro), 188 of 188 (requests), 156 of 156 (ripgrep) before landing |
| Workspace time and disk | 50 ready workspaces: 127 s and 14 079 MB (ripgrep, cold build), 525 s and 65 141 MB (astro, copied `node_modules`), 427 s and 5 812 MB (astro, pnpm store with APFS clones); `df` deltas | 50 API agents ready (open a change, read, grep) in 0.27 s to 1.5 s wall; `df` delta 0.44 MB or less |
| History queries | `git blame` p50 0.03 s to 0.15 s, max 2.5 s; 50 at once: 0.2 s to 3.5 s wall | phase 4 (2026-10-17): `blame` equal to `git blame --first-parent`, cold p50 15.85 ms (ripgrep) and 35.06 ms (go), warm 0.58 / 0.72 ms; see [derived.md](derived.md) |

What this means for the plan:

1. Landing comes first. A serial queue with the repo's own CI caps throughput at 3600 / CI seconds. A
   50-change wave waits 4 min to 60 min for the last change. TreeYard lands in milliseconds, but only
   because it runs no CI. The RFC's optimistic mode (tests after the merge) is the missing half.
2. Early conflict data comes second, and the server already has it. Almost every conflict that git
   found at merge time was visible earlier as a shared path between open changes (bench: 98 % to
   100 %; TreeYard `flagged_early`: 100 %, except 76 of 77 for requests and 57 of 58 for ripgrep at 10
   agents). Live collision events would move nearly all of them out of the merge step.
3. Stale bases also break clean merges. In astro, wave 1 at 10 agents already gives a tree that is not
   the history: `[ci] release` lands first and deletes the `.changeset/*.md` files, then three PRs land
   cleanly and add their changeset files back; `pnpm-lock.yaml` differs by 100 lines. No conflict is
   reported. Post-merge checks are needed, not only conflict detection.
4. Workspaces matter for JS monorepos: 1.2 GB of `node_modules` per astro workspace, 68 s to 85 s each
   when 8 install at once. The known fix (pnpm store, APFS clones) cuts disk 11 times (65 141 MB to 5 812 MB)
   but not time (525 s to 427 s for 50). For ripgrep and requests a warm cache (uv, a cloned `target/`)
   is enough.

## Phase 2: TreeYard workspaces vs plain worktrees (2026-10-17)

Measured on the same machine (15-core CPU, 48 GB RAM), same pinned revisions (ripgrep `3fce3b5`,
astro `c75e897`), same warm caches (cargo registry, pnpm store), at most 8 workspaces made at once,
in one session. TreeYard: `make release` on the task-#007 working tree. Other sessions used the Mac
during the runs. Commands:

```sh
export BENCH_DIR=/private/tmp/rootstock-harness/007 BENCH_OSS=/private/tmp/rootstock-harness/oss
python3 bench/bench.py --dir $BENCH_DIR --oss $BENCH_OSS workspaces --repo ripgrep --n 1,10,50 --setup checkout,cargo-warm-clone,cargo-cold   # 6 min
python3 bench/bench.py --dir $BENCH_DIR --oss $BENCH_OSS workspaces --repo astro --n 1,10,50 --setup checkout,pnpm-store                     # 25 min
python3 bench/bench.py --dir $BENCH_DIR --oss $BENCH_OSS cow --repo ripgrep --n 1,10,50    # 31 s
python3 bench/bench.py --dir $BENCH_DIR --oss $BENCH_OSS cow --repo astro --n 1,10,50      # 27 min, mostly deleting 300 astro workspaces
python3 bench/bench.py --dir $BENCH_DIR report
```

`make bench-cow` runs the `cow` part for `BENCH_REPOS`.

### Result: 50 extra agents, each ready to build and test

| Repo | Plain git worktrees | TreeYard workspaces | TreeYard from the warm pool |
|---|---|---|---|
| ripgrep (237 files, `target/` 346 MB) | `worktree add` + clone of a warm `target/` + `cargo build`: 69.66 s, 7 789 MB (cold build: 180.38 s, 14 204 MB) | 1.22 s, 48.3 MB (1.0 MB per agent); per workspace p50 0.16 s | 0.86 s, 5.1 MB; p50 0.12 s |
| astro (7 033 files, `node_modules` 1.2 GB) | `worktree add` + `pnpm install --offline` from the store: 352.53 s, 5 859 MB (117 MB per agent); p50 56.6 s | 85.30 s, 2 197 MB (43.9 MB per agent); p50 12.7 s | 26.19 s, 1 506 MB (30.1 MB per agent); p50 3.5 s |

Ready to edit (files plus the dependency layer, no setup command): ripgrep 50 in 1.03 s (50.4 MB), astro
50 in 52.03 s (1 443 MB), and from the warm pool astro 50 in 4.08 s (81.8 MB, p50 0.019 s per workspace).
One workspace alone: ripgrep 0.03 s, astro 1.00 s (0.01 s from the pool).

What the numbers say:

1. **Time.** The RFC target is a new workspace below 1 s. ripgrep meets it in every mode (p50 0.16 s
   with 8 at once). astro meets it only from the warm pool: cloning 562 `node_modules` directories takes
   0.93 s alone and about 7 s each when 8 run at once (APFS clones are metadata work per inode, and they
   queue). `treeyard daemon --spares N` moves that cost out of the request.
2. **Disk.** Per extra agent, ripgrep costs about 1 MB instead of 156 MB (warm clone) or 284 MB (cold).
   astro costs 29 MB to 44 MB instead of 117 MB with pnpm's own store and 1 303 MB with copied
   `node_modules` (phase-0 run). The 29 MB is APFS metadata for the cloned inodes; the spare runs show
   1.6 MB per agent because the pool was filled before the measurement started.
3. **Builds.** Clones keep mtimes, so cargo treats the cloned `target/` as fresh: `cargo build` takes
   0.05 s to 0.13 s per new ripgrep workspace (a worktree with a cloned `target/`: 10.4 s p50, because checkout gives new mtimes).
   `pnpm install --offline` on a restored astro layer still takes 1.5 s alone and 3.5 s to 3.8 s with 8 at once.
4. **Not free.** The first workspace per lockfile builds the layer (ripgrep 5.3 s, astro 6.7 s; not
   counted, like the baseline seed). Filling the pool takes about 1 s per astro spare. Deleting 50 astro
   workspaces takes 142.6 s to 147.3 s; 50 ripgrep workspaces 2.8 s to 3.1 s.

How it is measured (`cmd_cow` in `bench/bench.py`): `treeyard init` and `serve` on the pinned
revision; one `first` workspace with `--setup` builds the layer. Then N workspaces at once (at most 8)
with `treeyard workspace create NAME --layer DIR=LOCKFILE [--setup CMD]` through the CLI. Each one then
appends a line to the README and runs `treeyard workspace sync`, and the bench checks that the server
got exactly that path. Per-workspace times are the `create` call alone; the run's wall time also covers
the edit and the sync, so it is larger than the baseline's worktree-plus-setup time would be for the same
work. `spare-*` modes first run `treeyard workspace spare --count N` (the "pool fill" column, not counted).
`df delta` is the statvfs change of the volume over the batch; `du` counts each clone in full.

## Phase 3: coordination replay

Measured 2026-10-17 on the same machine (15-core CPU, 48 GB RAM), with `treeyard replay` on the last
500 first-parent commits of ripgrep (`3fce3b5`) and requests (`611c6162`), seed 1, at 16 and 50 agents.
Binary: `make release` of the working tree at the end of task #009 (copy
`/private/tmp/rootstock-harness/009/treeyard-009`, sha256 prefix `8d9a7e115aa85591`). Baseline: the
binary of main `2d90daf` plus the #003 to #007 working tree, copied before any #009 change
(`/private/tmp/rootstock-harness/009/treeyard-base`). Other sessions ran on the Mac at the same time,
so the land times are noisy (see below).

Command for one run (`rp.sh TAG REPO N CONFIG [flags]` in `/private/tmp/rootstock-harness/009/`):

```sh
treeyard replay --src oss/ripgrep --from HEAD~500 --to HEAD --agents 16 --data D \
  [--config full.toml] [--intents] [--wave-base]
```

Configs: `drivers.toml` = cargo-lock for `Cargo.lock`, union for `CHANGELOG.md`, `HISTORY.md`,
`AUTHORS.rst`, `syntax = "mergiraf"` (mergiraf 0.20.0); `cont.toml` = `continuous_rebase = true`;
`full.toml` = both. `--intents`: each agent declares its commit's paths at `open_change`; an agent
whose intent overlaps an open change, or a path of an agent that waits, starts in a later round (the
"defer" policy of an orchestrator). The default model gives each agent its commit's true parent as the
base (a stale base, as in phase 0). `--wave-base` gives each agent the main of its round and applies its
commit as a 3-way edit; an agent whose edit does not apply there is "blocked on base" and retries in the
next round.

### Result: thrown-away work and forced resolutions

| Repo | Agents | Baseline: thrown away / forced / waves equal to history | Intents (+ drivers + continuous rebase) | Rounds per wave with intents (stacked: N) |
|---|---|---|---|---|
| ripgrep | 16 | 111 / 7 / 21 of 32 | 0 / 0 / 32 of 32 | 6.7 (2.34 changes land per round) |
| ripgrep | 50 | 247 / 8 / 1 of 10 | 0 / 0 / 10 of 10 | 17.0 (2.93) |
| requests | 16 | 138 / 3 / 30 of 32 | 0 / 0 / 32 of 32 | 5.7 (2.75) |
| requests | 50 | 239 / 3 / 0 of 10 | 0 / 0 / 10 of 10 | 14.9 (3.36) |

"Thrown away" = land attempts that came back with a conflict, after which the replay agent redoes the
commit. "Forced" = commits that only landed after `resolve take=ours`. With intents, no work is thrown
away and every wave equals the history, also at 50 agents, where the baseline final tree is wrong. The
price is order: the overlapping agents of a wave start one after another, 5.7 to 17 rounds per wave
instead of 1, but 2.3 to 3.4 changes still land per round, against 1 for `--stacked`. The replay
declares exact paths, so these intent numbers are an upper bound for agents that guess their paths.

What the other features do in this replay:

- Drivers and continuous rebase change nothing on the stale-parent model (same 111 / 247 / 138 / 239).
  This is by design: both now run only when the change's base is an ancestor of main. On a stale parent,
  main seems to delete the work that the base has, and the first versions of both features applied those
  deletions. Drivers: 111 conflicts fell to 49, but 0 of 32 waves matched history (run `t-drivers`).
  Continuous rebase: 111 rose to 137 (run `old/m-ripgrep-16-p-cont`).
- On the wave-base model, conflicts at land are already rare (1 or 2 per run), because an agent whose
  edit does not apply is blocked first (186 to 883 times). There, drivers resolved 1 Cargo.lock conflict
  per ripgrep run, and continuous rebase found the remaining conflict before its owner landed in 3 runs.
  The replay has no agent time between steps, so it cannot show how much earlier that is.
- Every clean merge still gives the same tree as `git merge-tree`: "server clean, git conflict" is 0 in
  all 32 runs. The only extra cases are 1 Cargo.lock merge per ripgrep wave-base run that a driver
  resolved where git conflicts.

### Early warnings: precision next to recall

KB 003 asked for precision. On the stale-parent model (no config):

| Repo | Agents | Shared path: flagged / conflicted / precision | Trial merge: flagged / conflicted / precision / recall |
|---|---|---|---|
| ripgrep | 16 | 401 / 80 / 0.20 | 200 / 79 / 0.395 / 0.975 |
| ripgrep | 50 | 446 / 156 / 0.35 | 303 / 152 / 0.502 / 0.974 |
| requests | 16 | 365 / 111 / 0.304 | 254 / 110 / 0.433 / 0.991 |
| requests | 50 | 443 / 188 / 0.424 | 349 / 188 / 0.539 / 1.0 |

A shared path is a weak signal: 58 % to 80 % of the flagged changes landed clean. The trial merge of
`collisions` (the same 3-way merge that `land` runs, against each other open change) cuts the false
alarms by 37 % to 62 % (ripgrep 16: 321 to 121) and keeps 97 % to 100 % recall. Neither number is 1, because the merge at land time is with
main as it is then, not with one other change.

### Semantic guard

No config, stale parents. A review task `absorbed_edit` is opened when a rebase makes one of the
change's edits vanish:

| Repo | Agents | absorbed_edit tasks | on a path that is wrong at wave end | waves not equal to history (not explained by forced) |
|---|---|---|---|---|
| ripgrep | 16 | 1 | 1 | 11 (7) |
| ripgrep | 50 | 6 | 5 | 9 (6) |
| requests | 16 | 1 | 1 | 2 (0) |
| requests | 50 | 2 | 2 | 10 (8) |

The `dbg!` case is caught: at 16 agents the only task is commit `e92e2ef8137a` "cli: remove stray
`dbg!`", path `crates/cli/src/lib.rs`, whole file absorbed, path wrong at wave end. Other tasks at 50
agents: `d47663b1b454` "searcher: fix regression with `--line-buffered` flag" (3 paths), `9c8457522913`
"printer: drop dependency on serde_derive" (`Cargo.lock`), `79d393a302f1` "release: remove riscv64 and
powerpc64 artifacts" (`CHANGELOG.md`). 9 of 10 tasks sit on a path that is wrong at wave end. The guard
does not see every wrong wave: a clean merge that brings back deleted content (like the astro
`.changeset` case of phase 0) has no vanished edit.

### Land time and replay cost

Land p50 / p95 with no config: 2.16 / 8.37 ms (ripgrep 16) against 1.08 / 3.67 ms for the baseline binary
on a quieter machine. The same `--no-git-check` replay of ripgrep at 16 agents, run back to back under the
same load: baseline binary p50 2.33 / p95 10.12 ms, #009 binary p50 2.58 / p95 10.98 ms. So most of the
difference is machine load, and the guard and event code add about 10 %. With intents, land p95 is 1.59 to
3.33 ms (fewer changes are open at once). A replay with continuous rebase on the wave-base model takes 2 to
5 times longer than without (122 s to 260 s against 41 s to 64 s), because every rebase of every open change is also checked against git.

Raw table (`python3 /private/tmp/rootstock-harness/009/table.py`, all columns, 36 rows) is in
`/private/tmp/rootstock-harness/009/final-table.md`; each run's JSON is in
`/private/tmp/rootstock-harness/009/results/`.

## Scale: linux, 96 080 files (task 013, 2026-10-17)

Question: does one server stay fast on a big repo with long history? Repo: the Linux kernel,
https://github.com/torvalds/linux at `58161ead29e50d70a317ea26b472e0c7508e69da` (`master`, 2026-10-17),
96 080 files at the tip. Clone: `git clone --depth 1`, then four times `git fetch --deepen=500 origin
master`. The result is a shallow clone that reaches 1 461 641 commits (59 279 first-parent), 5.8 GB in `.git`.

Commands (scratch `/private/tmp/rootstock-harness/013`, JSON in `scale/results/`):

```sh
D=/private/tmp/rootstock-harness/013; L=/private/tmp/rootstock-harness/oss/linux
git archive 2d90daf | tar -x -C $D/before && (cd $D/before && cargo build --release)   # the "before" binary
python3 bench/scale.py --repo $L --dir $D/scale --tag linux-before --binary $D/before/target/release/treeyard --git
python3 bench/scale.py --repo $L --dir $D/scale --tag linux-after --git                 # working tree at task start
python3 bench/scale.py --repo $L --dir $D/scale --tag linux-after2                      # after the two fixes below
python3 bench/scale.py --repo $L --dir $D/scale --tag linux-history10000 --history 10000 --no-grep   # before fix 2
python3 bench/scale.py --repo $L --dir $D/scale --tag linux-history10000-walk --history 10000 --no-grep
```

"before" is the last commit on main (`2d90daf`). "after" is the working tree, which holds the hardening
work: incremental export (`write_tree_changed`), a lazy object cache, a bounded manifest cache, a
copy-on-write import of a shallow source, and the trigram index. One run per row. Other agent sessions
used the Mac at the same time: load average 6 to 17 on 15 cores (the `load` field of each JSON).

| Measurement (linux, history 0) | before `2d90daf` | after (task start) | after (fixes 1 and 2) |
|---|---|---|---|
| `init` wall time | 315.13 s | 1.23 s | 1.69 s |
| `init` peak RSS | 8 930.6 MB | 282.1 MB | 282.2 MB |
| `serve` start until `/health` answers | 6.07 s | 0.017 s | 0.014 s |
| Restart (second start) | 5.94 s | 0.033 s | 0.024 s |
| Server RSS after start / after 200 reads | 1 801.2 / 1 852.2 MB | 10.9 / 101.2 MB | 10.8 / 102.5 MB |
| Server RSS after grep / after 130 lands | 1 854.2 / 1 021.9 MB | 860.2 / 873.7 MB | 772.3 / 782.6 MB |
| `read`, 200 files, p50 / p95 / max | 0.25 / 0.43 / 55.0 ms | 0.44 / 0.78 / 56.27 ms | 0.5 / 0.86 / 58.49 ms |
| `list` of the root | 5.17 ms | 5.72 ms | 5.32 ms |
| `tree` of main (96 080 files) | no such op | 45.89 ms | 42.87 ms |
| `grep EXPORT_SYMBOL_GPL\(`, max 100, first / repeat | 680.15 / 634.16 ms (scan) | 17 635.41 / 11.12 ms | 18 827.15 / 8.08 ms |
| `grep struct file_operations [a-z_]+ = \{`, first / repeat | 945.43 / 900.82 ms | 20.56 / 19.75 ms | 15.96 / 15.82 ms |
| `grep TODO\|FIXME`, first / repeat | 236.95 / 239.52 ms | 14.19 / 10.12 ms | 7.9 / 7.96 ms |
| `land`, 50 one after another, p50 / p95 / max | 69.29 / 73.52 / 112.56 ms | 1.28 / 2.45 / 26.49 ms | 0.91 / 1.96 / 20.4 ms |
| `land`, 16 agents x 5 at once, p50 / p95 | 695.23 / 1 092.82 ms | 29.14 / 64.1 ms | 17.64 / 30.27 ms |
| Lands per second, 16 agents | 7.2 | 196.2 | 252.9 |
| `git fsck --connectivity-only` on the export repo | clean | clean | clean |

The data directory is 5 197.6 MB (before) and 5 944.9 MB (after) in `du -sk`. `du` counts APFS clones in
full, and the after import clones `objects/` copy-on-write, so the physical cost is lower (not measured).

Plain git baseline on the same clone (`--git`; worktree add, then per land: append a line, `git commit -a`,
`git merge-tree --write-tree`, `git update-ref`; no CI). Two runs, in the before and the after-task-start runs:

| Plain git, linux | run 1 | run 2 |
|---|---|---|
| `git worktree add` (checkout 1.7 GB) | 8.437 s | 7.684 s |
| `git status --porcelain` | 0.534 s | 0.315 s |
| `git grep -E -c`, the three patterns | 3 172.65 / 2 830.09 / 3 544.17 ms | 2 818.28 / 2 756.93 / 3 844.61 ms |
| edit + commit + merge-tree + update-ref, 50 lands, p50 / p95 | 183.7 / 196.07 ms | 201.14 / 237.77 ms |

So on linux a TreeYard land (p50 0.91 ms) is about 200 times faster than a local git commit plus merge
(p50 183.7 to 201.14 ms). The first indexed grep is slower than a scan, because it builds the trigram
index (255 MB of postings); later greps take 8 to 16 ms against 2.8 to 3.8 s for `git grep`. Set
`[index] search = "eager"` to build the index when the server starts, or send `"index": false` to scan.

### Hotspots found and fixed in this task

1. **The first grep stopped every land for seconds.** The posting index was built under the repo lock.
   Probe: land in a loop while the first grep runs (`python3 bench/grep_stall.py BINARY $L $D/probe/data`).
   The first rows ran the same loop from a scratch copy of the script; the last run is `bench/grep_stall.py`.

   | Build | grep wall | Lands during grep | Slowest land round trip | Land p50 |
   |---|---|---|---|---|
   | index built under the lock | 18.15 s | 378 | 4.567 s | 4.9 ms |
   | `search::build_index` outside the lock, 16 MiB prepare chunks | 17.37 s | 1 112 | 0.314 s | 4.1 ms |
   | same, 4 MiB prepare chunks (three runs) | 20.69 / 20.6 / 21.43 s | 1 365 / 1 366 / 1 383 | 0.191 / 0.205 / 0.218 s | 4.4 / 4.4 / 4.7 ms |

   Test: `index_built_outside_the_lock_follows_later_lands` (tests/derived.rs).
2. **A cold read after a long history import replayed every changeset.** With `--history 10000`, the
   first `read` rebuilt main's manifest from the root changeset and cached each step. Now `manifest` reads
   the git tree (`git ls-tree -r`, about 0.09 s on linux) after `MANIFEST_WALK` = 64 replayed steps.

   | linux, `--history 10000` | before fix 2 | after fix 2 |
   |---|---|---|
   | `init` wall / peak RSS | 6.334 s / 1 051.3 MB | 6.432 s / 1 051.5 MB |
   | `read`, 200 files, p50 / max (the first read is the max) | 0.45 / 752.7 ms | 0.42 / 114.74 ms |
   | Server RSS after reads | 756.3 MB | 97.1 MB |
   | `land`, 50 one after another, p50 / p95 | 1.3 / 3.06 ms | 1.21 / 2.9 ms |
   | `land`, 16 agents x 5, p50 / p95 | 22.88 / 171.62 ms | 27.79 / 152.63 ms |

   Test: `cold_manifest_from_git_equals_a_full_replay` (tests/hardening.rs) compares 231 changesets with a
   full replay. A mutation (drop one tree entry) makes it fail.

Open: with 10 000 imported changesets, parallel land p95 is 152.63 ms against 30.27 ms with none (not
investigated). `init --history 10000` peaks at 1 051.5 MB RSS. The index holds 255 MB of postings for linux.

## Hardening checks (task 013, 2026-10-17)

| Command | Result |
|---|---|
| `TREEYARD_FUZZ_SEEDS=300 cargo test --release --test fuzz_history -- --nocapture` | 300 seeds, 7718 lands, 5111 merges: 2200 clean and equal to git merge-tree, 2911 server conflicts (0 of them git merged cleanly); 7718 exports equal to the manifest, 7718 reads equal to git; 448.84 s |
| `cargo test --test hardening` (part of `make check`) | 12 passed: kill at 6 failpoints and restart, duplicate land race, lost land reply, client retry, 503 busy, data-dir lock, bind refusal, hostile paths, sizes, repo content and push, shallow import, cold manifests |

## Machine and sources

| Item | Value |
|---|---|
| Machine | 15-core CPU, 48 GB RAM, macOS, APFS |
| Tools | git 2.54.0 (Apple Git-157), Python 3.15.0, rustc and cargo 1.99.1, node v26.11.1, pnpm 11.28.4 (astro's `packageManager`), uv 0.13.0, pip 26.2.1 |
| ripgrep | https://github.com/BurntSushi/ripgrep at `3fce3b5bb0236da2df6d99672afb8a719642eca7` (2026-08-04), 237 files, 2215 first-parent commits |
| requests | https://github.com/psf/requests at `611c6162cbc4ac2020a2f91c7cfa4f3abf9bbb60` (2026-09-21), 130 files, 2669 first-parent commits |
| astro | https://github.com/withastro/astro at `c75e89711a4935a0befea6f768842f6d6a9de2bb` (2026-10-16), 7033 files, 11736 first-parent commits; checkout 71 MB, with `node_modules` 1.2 GB per workspace |
| TreeYard | this repo, `make release`, working tree on top of `2d90daf` plus the bench and the `flagged_early` counter |

Other agent sessions used the same Mac during the runs. The `df` deltas and the timings carry that
noise. Two runs of the conflict replay (`log-conflicts.txt` and `log-conflicts2.txt`) gave the same
counts, because the replay is deterministic for a seed.

## How to reproduce

```sh
export BENCH_DIR=/private/tmp/rootstock-harness/003     # scratch: clones, seeds, workspaces, results
export BENCH_OSS=/private/tmp/rootstock-harness/oss     # optional: existing clones (else cloned from the URLs)
make bench-conflicts                                    # 16 min: 3 repos x 500 commits x N=1,10,50 x seeds 1,2,3
make bench-treeyard                                    # 7 min: init, 1/10/50 API agents, `treeyard replay` N=1,10,50
make bench-workspaces                                   # 67 min: 3 repos x setups x N=1,10,50, 8 at once
make bench-queue BENCH_REPOS=requests                   # 63 min (73 s CI x 51)
make bench-queue BENCH_REPOS="ripgrep astro"            # 29 min
make bench-history                                      # 47 s
python3 bench/bench.py --dir $BENCH_DIR report          # the tables below
```

`make bench` runs all of it (about 3 h here). `make bench-quick` is a subset: 4 min 31 s here
(`time make bench-quick`, log `log-quick.txt`). The wall times above
come from `time make ...` in the log files in `$BENCH_DIR`. Pinned revisions and setup commands live in
`REPOS` in `bench/bench.py`. `make bench-test` (part of `make check`) tests every subcommand on a
synthetic 10-commit repo.

## Method

**Workspaces.** `git worktree add --detach` from a local clone, then the setup commands, for N
workspaces with at most 8 made at the same time. One seed workspace installs everything first, so every
setup starts with warm caches (cargo registry, pip/uv cache, pnpm store); the seed time is not counted.

| Repo | Setup | Commands |
|---|---|---|
| ripgrep | `cargo-cold` | `cargo build --offline -q` (own `target/`) |
| ripgrep | `cargo-warm-clone` | `cp -c -R -p <seed>/target target` (APFS clonefile), then `cargo build --offline -q` |
| requests | `pip` | `python3 -m venv .venv`, `.venv/bin/pip install -q -r requirements-dev.txt` |
| requests | `uv` | `uv venv -q .venv`, `uv pip install -q --python .venv/bin/python -r requirements-dev.txt` |
| astro | `pnpm-copy` | `pnpm install --offline --frozen-lockfile --config.package-import-method=copy` (what a sandbox without a shared store does) |
| astro | `pnpm-store` | `pnpm install --offline --frozen-lockfile` (pnpm's default: clones from the global store) |

`du` counts APFS clones in full, so `du` is the cost on a file system without copy-on-write (for
example ext4 in a container). `df delta` (statvfs used bytes before and after) is the real cost on this
Mac. TreeYard: `treeyard init`, then `serve`; N threads each call `open_change`, then `read` of the
README and `grep TODO` at that change over HTTP.

**Late conflicts.** The last 500 first-parent commits are cut into waves of N agents. Each agent works
on its commit's true parent and does not see the others (the worktree model). The wave lands in a random
order (seeds 1, 2, 3) with `git merge-tree --write-tree --merge-base=<parent> main <commit>` and
`merge.renames=false`. A conflict is seen only at merge time: the change is thrown away and redone on the
newer main. If nothing lands in a redo round, one change is forced with `-Xtheirs`. "Shared a path
before merge" counts conflicts whose paths overlap another change of the same wave, which a server that
sees all open changes knows before the merge. "Lines thrown away" is the added plus deleted lines of the
conflicted changes, over all lines. A wave is faithful when main equals the tree of the wave's last
original commit. `treeyard replay` runs the same model inside the server (its own RNG, so the landing
order differs) and checks every merge against `git merge-tree`.

**Serial merge queue.** N changes enter the queue at t = 0. The queue lands them one at a time in
history order: check out main plus the change (that is the original commit), run the repo's CI, land.
One queue of 50 runs; N = 1 and N = 10 are its first 1 and 10 changes. Wait = time before the change's
own CI starts.

| Repo | CI command (per change) |
|---|---|
| ripgrep | `cargo build -q && cargo test -q` |
| requests | `.venv/bin/python -m pytest -q -x -p no:cacheprovider tests` (619 passed, 15 skipped; about 72 s, mostly timeouts in `TestTimeout`, 1 % CPU) |
| astro | `pnpm install --frozen-lockfile --prefer-offline && pnpm run build:ci:no-cache && pnpm -C packages/astro run test:unit` (3737 unit tests) |

**History queries.** `git log --format=%H <rev> -- <file>` and `git blame --porcelain <rev> -- <file>` on
the 10 files that the last 500 commits changed most, then N blames at once.

## Raw numbers

### Workspaces (git worktree add + setup)

| repo | setup | N | jobs | wall s | per ws p50 s | per ws p95 s | git p50 s | du per ws MB | du sum MB | du dedup MB | df delta MB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| astro | checkout | 1 | 1 | 0.54 | 0.5416 | 0.5416 | 0.5416 | 71.3 | 71.3 | 71.3 | 75.9 |
| astro | checkout | 10 | 8 | 4.25 | 3.4797 | 3.4939 | 3.4797 | 71.3 | 713.0 | 713.0 | 749.7 |
| astro | checkout | 50 | 8 | 21.34 | 3.3943 | 3.6034 | 3.3943 | 71.3 | 3565.2 | 3565.2 | 3840.7 |
| astro | pnpm-copy | 1 | 1 | 7.84 | 7.8383 | 7.8383 | 0.5539 | 1313.4 | 1313.4 | 1313.4 | 1355.9 |
| astro | pnpm-copy | 10 | 8 | 89.16 | 76.4464 | 76.4842 | 3.652 | 1236.3 | 12363.4 | 12359.6 | 12922.1 |
| astro | pnpm-copy | 50 | 8 | 524.85 | 84.8177 | 88.8214 | 3.8433 | 1231.6 | 61577.5 | 61577.5 | 65141.0 |
| astro | pnpm-store | 1 | 1 | 11.88 | 11.8804 | 11.8804 | 1.2074 | 1231.4 | 1231.4 | 1231.4 | 119.8 |
| astro | pnpm-store | 10 | 8 | 87.17 | 68.7523 | 68.7898 | 4.9935 | 1231.4 | 12313.7 | 12313.7 | 1158.2 |
| astro | pnpm-store | 50 | 8 | 427.42 | 68.2406 | 70.543 | 4.573 | 1231.4 | 61568.6 | 61568.6 | 5811.7 |
| requests | checkout | 1 | 1 | 0.05 | 0.0434 | 0.0434 | 0.0434 | 4.6 | 4.6 | 4.6 | 4.7 |
| requests | checkout | 10 | 8 | 0.12 | 0.0847 | 0.0953 | 0.0847 | 4.6 | 45.7 | 45.7 | 46.8 |
| requests | checkout | 50 | 8 | 0.6 | 0.089 | 0.1221 | 0.089 | 4.6 | 228.7 | 228.7 | 234.1 |
| requests | pip | 1 | 1 | 5.37 | 5.3665 | 5.3665 | 0.0283 | 61.7 | 61.7 | 61.7 | 63.5 |
| requests | pip | 10 | 8 | 9.56 | 5.7891 | 7.5474 | 0.0866 | 61.7 | 617.5 | 617.5 | 635.4 |
| requests | pip | 50 | 8 | 35.57 | 5.1594 | 7.013 | 0.0506 | 61.7 | 3087.3 | 3087.3 | 3183.0 |
| requests | uv | 1 | 1 | 0.82 | 0.8183 | 0.8183 | 0.0289 | 32.5 | 32.5 | 32.5 | 6.8 |
| requests | uv | 10 | 8 | 1.55 | 0.9551 | 0.964 | 0.0875 | 32.5 | 324.7 | 324.7 | 56.1 |
| requests | uv | 50 | 8 | 6.03 | 0.9324 | 0.9814 | 0.0821 | 32.5 | 1623.4 | 1623.4 | 278.2 |
| ripgrep | checkout | 1 | 1 | 0.04 | 0.0363 | 0.0363 | 0.0363 | 3.7 | 3.7 | 3.7 | 3.9 |
| ripgrep | checkout | 10 | 8 | 0.17 | 0.127 | 0.1372 | 0.127 | 3.7 | 37.2 | 37.2 | 39.1 |
| ripgrep | checkout | 50 | 8 | 0.94 | 0.1389 | 0.1763 | 0.1389 | 3.7 | 185.9 | 185.9 | 195.1 |
| ripgrep | cargo-cold | 1 | 1 | 3.96 | 3.9641 | 3.9641 | 0.0346 | 302.0 | 302.0 | 302.0 | 283.6 |
| ripgrep | cargo-cold | 10 | 8 | 27.85 | 22.4263 | 23.5163 | 0.1302 | 302.9 | 3028.8 | 3028.8 | 2837.6 |
| ripgrep | cargo-cold | 50 | 8 | 126.68 | 19.292 | 24.2163 | 0.0653 | 302.7 | 15134.5 | 15134.5 | 14079.0 |
| ripgrep | cargo-warm-clone | 1 | 1 | 2.45 | 2.4533 | 2.4533 | 0.0343 | 451.6 | 451.6 | 451.6 | 154.8 |
| ripgrep | cargo-warm-clone | 10 | 8 | 13.02 | 10.6142 | 10.9683 | 0.1295 | 454.9 | 4548.7 | 4548.7 | 1684.9 |
| ripgrep | cargo-warm-clone | 50 | 8 | 53.38 | 8.8575 | 11.5311 | 0.1312 | 453.6 | 22678.5 | 22678.5 | 7799.2 |

### Late conflicts (git merge-tree, one value per seed)

| repo | commits | agents | seeds | changes | late conflicts | shared a path before merge | redone | forced | unresolved | lines thrown away share | lockfile hits | faithful waves | first divergent wave | final tree equal |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| astro | 500 | 1 | 3 | 500/500/500 | 0/0/0 | 0/0/0 | 0/0/0 | 0/0/0 | 0/0/0 | 0.0/0.0/0.0 | 0/0/0 | 500/500/500 of 500 | None/None/None | True/True/True |
| astro | 500 | 10 | 3 | 500/500/500 | 58/58/54 | 58/58/54 | 62/67/64 | 2/4/5 | 0/0/0 | 0.1219/0.1599/0.0768 | 34/34/27 | 0/0/0 of 50 | 1/1/1 | False/False/False |
| astro | 500 | 50 | 3 | 500/500/500 | 129/129/128 | 129/129/128 | 224/205/223 | 9/8/9 | 0/0/0 | 0.3344/0.3776/0.3296 | 79/75/77 | 0/0/0 of 10 | 1/1/1 | False/False/False |
| requests | 500 | 1 | 3 | 500/500/500 | 0/0/0 | 0/0/0 | 0/0/0 | 0/0/0 | 0/0/0 | 0.0/0.0/0.0 | 0/0/0 | 500/500/500 of 500 | None/None/None | True/True/True |
| requests | 500 | 10 | 3 | 500/500/500 | 73/73/82 | 72/73/82 | 76/76/85 | 2/1/1 | 0/0/0 | 0.0548/0.3027/0.2758 | 0/0/0 | 48/49/49 of 50 | 4/4/4 | True/True/True |
| requests | 500 | 50 | 3 | 500/500/500 | 181/172/185 | 181/172/185 | 192/186/199 | 1/2/2 | 0/0/0 | 0.7806/0.781/0.7892 | 0/0/0 | 1/9/1 of 10 | 2/2/2 | False/True/False |
| ripgrep | 500 | 1 | 3 | 498/498/498 | 0/0/0 | 0/0/0 | 0/0/0 | 0/0/0 | 0/0/0 | 0.0/0.0/0.0 | 0/0/0 | 500/500/500 of 500 | None/None/None | True/True/True |
| ripgrep | 500 | 10 | 3 | 498/498/498 | 54/46/51 | 53/46/51 | 54/46/51 | 1/0/0 | 0/0/0 | 0.1281/0.1097/0.1241 | 14/9/12 | 47/49/49 of 50 | 21/50/50 | False/False/False |
| ripgrep | 500 | 50 | 3 | 498/498/498 | 141/133/144 | 141/133/144 | 142/136/147 | 1/1/1 | 0/0/0 | 0.6543/0.5947/0.6538 | 36/35/30 | 10/4/4 of 10 | None/5/5 | True/False/False |

### Serial merge queue (real CI per change)

| repo | N | CI p50 s | wait mean s | wait p95 s | wait max s | drain s | landings/h | CI failed |
|---|---|---|---|---|---|---|---|---|
| astro | 1 | 28.14 | 0.0 | 0.0 | 0.0 | 28.86 | 124.8 | 0 |
| astro | 10 | 28.14 | 128.4333 | 257.0729 | 257.0729 | 289.88 | 124.2 | 0 |
| astro | 50 | 28.14 | 718.3025 | 1366.7477 | 1414.3101 | 1440.78 | 124.9 | 0 |
| requests | 1 | 72.8419 | 0.0 | 0.0 | 0.0 | 72.88 | 49.4 | 0 |
| requests | 10 | 72.4693 | 326.644 | 652.808 | 652.808 | 725.21 | 49.6 | 0 |
| requests | 50 | 72.9924 | 1797.5585 | 3461.2573 | 3614.3189 | 3687.45 | 48.8 | 0 |
| ripgrep | 1 | 3.6158 | 0.0 | 0.0 | 0.0 | 3.67 | 979.7 | 0 |
| ripgrep | 10 | 4.0133 | 17.9808 | 36.3493 | 36.3493 | 40.46 | 889.7 | 0 |
| ripgrep | 50 | 4.1748 | 118.804 | 226.4997 | 248.2778 | 255.45 | 704.6 | 0 |

### TreeYard API agents (no workspace)

| repo | init s | data MB | files | N | wall s | open_change p50 s | ready p50 s | ready p95 s | df delta MB | server RSS MB |
|---|---|---|---|---|---|---|---|---|---|---|
| astro | 7.42 | 761.1 | 7033 | 1 | 0.048 | 0.0005 | 0.0465 | 0.0465 | 0.01 | 63.6 |
| astro | 7.42 | 761.1 | 7033 | 10 | 0.303 | 0.0016 | 0.154 | 0.3012 | 0.08 | 64.5 |
| astro | 7.42 | 761.1 | 7033 | 50 | 1.502 | 0.1549 | 0.7545 | 1.4322 | -3.14 | 67.1 |
| requests | 3.32 | 65.2 | 130 | 1 | 0.013 | 0.0005 | 0.0113 | 0.0113 | 0.01 | 19.3 |
| requests | 3.32 | 65.2 | 130 | 10 | 0.057 | 0.0016 | 0.0305 | 0.0544 | 0.08 | 20.4 |
| requests | 3.32 | 65.2 | 130 | 50 | 0.267 | 0.0268 | 0.1334 | 0.2505 | 0.44 | 24.0 |
| ripgrep | 4.62 | 77.4 | 237 | 1 | 0.015 | 0.0005 | 0.0124 | 0.0124 | 0.02 | 11.2 |
| ripgrep | 4.62 | 77.4 | 237 | 10 | 0.078 | 0.0011 | 0.0404 | 0.0755 | 0.08 | 12.3 |
| ripgrep | 4.62 | 77.4 | 237 | 50 | 0.374 | 0.0585 | 0.185 | 0.3512 | 0.44 | 15.0 |

### TreeYard replay (seed 1)

| repo | commits | agents | first conflicts | flagged by overlaps | redone | forced | faithful waves | final tree equal | land p50 ms | land p95 ms | land max ms | merges same tree as git | server clean, git conflict | server conflict, git clean |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| astro | 500 | 1 | 0 | 0 | 0 | 0 | 500 of 500 | True | 11.57 | 12.32 | 28.32 | 499 | 0 | 0 |
| astro | 500 | 10 | 63 | 63 | 73 | 7 | 0 of 50 | False | 12.73 | 32.0 | 57.14 | 492 | 0 | 0 |
| astro | 500 | 50 | 139 | 139 | 255 | 10 | 0 of 10 | False | 14.78 | 43.17 | 133.02 | 490 | 0 | 0 |
| requests | 500 | 1 | 0 | 0 | 0 | 0 | 500 of 500 | True | 0.48 | 0.6 | 1.83 | 499 | 0 | 0 |
| requests | 500 | 10 | 77 | 76 | 85 | 3 | 46 of 50 | True | 0.63 | 1.48 | 5.8 | 496 | 0 | 0 |
| requests | 500 | 50 | 188 | 188 | 233 | 3 | 0 of 10 | False | 0.76 | 2.32 | 14.72 | 497 | 0 | 1 |
| ripgrep | 500 | 1 | 0 | 0 | 0 | 0 | 500 of 500 | True | 0.62 | 0.74 | 1.72 | 497 | 0 | 0 |
| ripgrep | 500 | 10 | 58 | 57 | 58 | 1 | 47 of 50 | False | 0.89 | 2.41 | 6.9 | 496 | 0 | 0 |
| ripgrep | 500 | 50 | 156 | 156 | 231 | 8 | 1 of 10 | False | 1.17 | 3.36 | 14.52 | 490 | 0 | 0 |

### History queries in a checkout (git log / blame on the most-changed files)

| repo | commits in history | files | log p50 s | log max s | blame p50 s | blame max s | N=1 wall / p95 s | N=10 wall / p95 s | N=50 wall / p95 s |
|---|---|---|---|---|---|---|---|---|---|
| astro | 15292 | 10 | 0.1544 | 0.2381 | 0.148 | 2.5407 | 2.501 / 2.4984 | 2.507 / 2.5065 | 3.519 / 3.4432 |
| requests | 6495 | 10 | 0.0475 | 0.0573 | 0.0277 | 0.0864 | 0.03 / 0.0274 | 0.098 / 0.0951 | 0.252 / 0.1384 |
| ripgrep | 2287 | 10 | 0.0367 | 0.0452 | 0.0453 | 0.062 | 0.071 / 0.0657 | 0.087 / 0.0829 | 0.215 / 0.1761 |

### Phase 2 raw numbers (`report` on `/private/tmp/rootstock-harness/007`)

#### Workspaces (git worktree add + setup)

| repo | setup | N | jobs | wall s | per ws p50 s | per ws p95 s | git p50 s | du per ws MB | du sum MB | du dedup MB | df delta MB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| astro | checkout | 1 | 1 | 0.57 | 0.5711 | 0.5711 | 0.5711 | 71.3 | 71.3 | 71.3 | 75.5 |
| astro | checkout | 10 | 8 | 4.34 | 3.5311 | 3.5693 | 3.5311 | 71.3 | 713.0 | 713.0 | 764.5 |
| astro | checkout | 50 | 8 | 21.96 | 3.5046 | 3.6322 | 3.5046 | 71.3 | 3565.2 | 3565.2 | 3841.7 |
| astro | pnpm-store | 1 | 1 | 7.21 | 7.2083 | 7.2083 | 0.558 | 1231.4 | 1231.4 | 1231.4 | 119.4 |
| astro | pnpm-store | 10 | 8 | 70.04 | 56.8645 | 56.9256 | 3.5403 | 1231.4 | 12313.7 | 12313.7 | 1226.6 |
| astro | pnpm-store | 50 | 8 | 352.53 | 56.5926 | 57.2773 | 3.8042 | 1231.4 | 61568.6 | 61568.6 | 5859.4 |
| ripgrep | checkout | 1 | 1 | 0.08 | 0.0788 | 0.0788 | 0.0788 | 3.7 | 3.7 | 3.7 | 3.9 |
| ripgrep | checkout | 10 | 8 | 0.3 | 0.207 | 0.244 | 0.207 | 3.7 | 37.2 | 37.2 | 39.2 |
| ripgrep | checkout | 50 | 8 | 1.3 | 0.1943 | 0.2604 | 0.1943 | 3.7 | 185.9 | 185.9 | 195.3 |
| ripgrep | cargo-warm-clone | 1 | 1 | 3.98 | 3.98 | 3.98 | 0.0723 | 452.3 | 452.3 | 452.3 | 154.7 |
| ripgrep | cargo-warm-clone | 10 | 8 | 19.55 | 14.0198 | 15.8168 | 0.172 | 455.4 | 4554.3 | 4554.3 | 1561.5 |
| ripgrep | cargo-warm-clone | 50 | 8 | 69.66 | 10.4121 | 15.2919 | 0.1322 | 453.9 | 22696.9 | 22696.9 | 7789.2 |
| ripgrep | cargo-cold | 1 | 1 | 6.09 | 6.0854 | 6.0854 | 0.0705 | 302.2 | 302.2 | 302.2 | 283.8 |
| ripgrep | cargo-cold | 10 | 8 | 41.81 | 31.1136 | 33.7835 | 0.1716 | 302.9 | 3029.2 | 3029.2 | 2843.0 |
| ripgrep | cargo-cold | 50 | 8 | 180.38 | 27.5405 | 41.2923 | 0.0776 | 303.1 | 15154.4 | 15154.4 | 14203.9 |

#### TreeYard workspaces (copy-on-write clone + dependency layer)

| repo | mode | N | jobs | wall s | per ws p50 s | per ws p95 s | clone p50 s | layer p50 s | setup p50 s | sync p50 s | layer reused | du per ws MB | df delta MB | df per ws MB | pool fill s (not counted) | remove all s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| astro | files | 1 | 1 | 1.12 | 1.0042 | 1.0042 | 0.0666 | 0.9302 | 0.0 | 0.116 | 1 | 1231.4 | 29.6 | 29.6 |  | 3.78 |
| astro | files | 10 | 8 | 10.19 | 7.4575 | 7.4731 | 0.2037 | 7.0661 | 0.0 | 0.6655 | 10 | 1231.4 | 272.0 | 27.2 |  | 28.03 |
| astro | files | 50 | 8 | 52.03 | 7.5597 | 8.2497 | 0.2748 | 7.209 | 0.0 | 0.6651 | 50 | 1231.4 | 1443.2 | 28.9 |  | 142.55 |
| astro | build | 1 | 1 | 3.57 | 3.4418 | 3.4418 | 0.088 | 1.7783 | 1.5674 | 0.1287 | 1 | 1231.4 | 73.6 | 73.6 |  | 3.77 |
| astro | build | 10 | 8 | 16.48 | 11.1736 | 11.2132 | 0.2728 | 6.9752 | 3.7797 | 0.5801 | 10 | 1231.4 | 341.4 | 34.1 |  | 28.8 |
| astro | build | 50 | 8 | 85.3 | 12.7286 | 13.2566 | 0.2783 | 8.1691 | 3.6609 | 0.6605 | 50 | 1231.4 | 2197.3 | 43.9 |  | 143.09 |
| astro | spare-files | 1 | 1 | 0.13 | 0.01 | 0.01 | 0.0018 | 0.0012 | 0.0 | 0.1196 | 1 | 1231.4 | 2.2 | 2.2 | 2.14 | 4.09 |
| astro | spare-files | 10 | 8 | 0.8 | 0.0291 | 0.034 | 0.0089 | 0.0073 | 0.0 | 0.5869 | 10 | 1231.4 | 12.1 | 1.2 | 9.82 | 28.19 |
| astro | spare-files | 50 | 8 | 4.08 | 0.0192 | 0.0431 | 0.0046 | 0.0042 | 0.0 | 0.6252 | 50 | 1231.4 | 81.8 | 1.6 | 51.17 | 147.34 |
| astro | spare-build | 1 | 1 | 1.61 | 1.5022 | 1.5022 | 0.0018 | 0.0014 | 1.4919 | 0.1111 | 1 | 1231.4 | 7.0 | 7.0 | 2.01 | 4.2 |
| astro | spare-build | 10 | 8 | 5.88 | 3.5586 | 3.7242 | 0.0099 | 0.0092 | 3.5323 | 0.4388 | 10 | 1231.4 | 294.7 | 29.5 | 9.6 | 29.03 |
| astro | spare-build | 50 | 8 | 26.19 | 3.5353 | 3.7352 | 0.0049 | 0.0062 | 3.5051 | 0.5113 | 50 | 1231.4 | 1506.3 | 30.1 | 50.82 | 144.23 |
| ripgrep | files | 1 | 1 | 0.04 | 0.0307 | 0.0307 | 0.0015 | 0.0238 | 0.0 | 0.0092 | 1 | 345.5 | 2.1 | 2.1 |  | 0.12 |
| ripgrep | files | 10 | 8 | 0.23 | 0.097 | 0.1698 | 0.0186 | 0.0553 | 0.0 | 0.0138 | 10 | 345.5 | 10.9 | 1.1 |  | 0.65 |
| ripgrep | files | 50 | 8 | 1.03 | 0.1388 | 0.1867 | 0.027 | 0.062 | 0.0 | 0.0137 | 50 | 345.5 | 50.4 | 1.0 |  | 2.95 |
| ripgrep | build | 1 | 1 | 0.08 | 0.0679 | 0.0679 | 0.0014 | 0.0126 | 0.0486 | 0.008 | 1 | 345.5 | 1.5 | 1.5 |  | 0.07 |
| ripgrep | build | 10 | 8 | 0.36 | 0.1966 | 0.3058 | 0.0394 | 0.0458 | 0.062 | 0.0165 | 10 | 345.5 | 7.8 | 0.8 |  | 0.65 |
| ripgrep | build | 50 | 8 | 1.22 | 0.1585 | 0.2301 | 0.0229 | 0.0413 | 0.0652 | 0.0193 | 50 | 345.5 | 48.3 | 1.0 |  | 2.85 |
| ripgrep | spare-files | 1 | 1 | 0.01 | 0.0055 | 0.0055 | 0.0002 | 0.0005 | 0.0 | 0.0081 | 1 | 345.5 | 0.1 | 0.1 | 0.02 | 0.08 |
| ripgrep | spare-files | 10 | 8 | 0.04 | 0.0072 | 0.0099 | 0.0003 | 0.0005 | 0.0 | 0.0115 | 10 | 345.5 | 0.7 | 0.1 | 0.26 | 0.66 |
| ripgrep | spare-files | 50 | 8 | 0.17 | 0.0104 | 0.0142 | 0.0017 | 0.0017 | 0.0 | 0.0127 | 50 | 345.5 | 4.7 | 0.1 | 0.95 | 2.79 |
| ripgrep | spare-build | 1 | 1 | 0.06 | 0.0545 | 0.0545 | 0.0002 | 0.0005 | 0.0488 | 0.0081 | 1 | 345.5 | 0.1 | 0.1 | 0.02 | 0.08 |
| ripgrep | spare-build | 10 | 8 | 0.24 | 0.142 | 0.1451 | 0.0005 | 0.0008 | 0.1293 | 0.0107 | 10 | 345.5 | -0.4 | -0.0 | 0.25 | 0.66 |
| ripgrep | spare-build | 50 | 8 | 0.86 | 0.1211 | 0.1465 | 0.0013 | 0.0019 | 0.1105 | 0.0118 | 50 | 345.5 | 5.1 | 0.1 | 0.95 | 3.05 |


## Limits

- Agents are simulated: each one applies a real commit. The replay has no agent time, so the cost of
  a redo is given in changes and lines, not in minutes.
- The worktree model gives every agent its commit's true parent. Real agents start on the main of
  their start time; the bench does not model longer tasks or agents that pull.
- The queue runs CI on the original commits in history order, so no change fails CI and none is kicked
  out. A real queue also sends back the 26 % to 37 % of changes that conflict, and they pass the queue
  again.
- The requests CI is about 72 s, and most of it is timeout tests at 1 % CPU. A faster suite gives a
  shorter queue; the cap stays 3600 / CI seconds per hour.
- Workspace timings come from at most 8 parallel setups on one machine with other sessions running.
  Setups for N = 10 and N = 50 are slower per workspace than N = 1 because they share CPU and disk.
- TreeYard "ready" means an API-only agent: it can read, grep, diff and edit, but not build or run
  tests. Agents that run code still need a workspace (RFC phase 2).
- Phase-0 land times (p50 0.6 ms on ripgrep, 11.6 ms on astro at 1 agent) come from the binary that
  rewrote the full tree on every export. Export is incremental now; see the scale section (linux p50 0.91 ms).
- `treeyard replay` "merges same tree as git" counts every merge record, including trivial ones.
- Not measured: ports, `.env` files and databases per workspace (research pain 3), and lost work from
  worktree cleanup (research pain 2).
