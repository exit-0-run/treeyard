# T2 preregistration: plain git worktrees against `treeyard connect`, 5 agents at once

Status: frozen before any real-agent run. This file holds no result. The kit, the free rehearsals
and their outputs are in [README.md](README.md). A change to this file after the first real run makes
the experiment exploratory, and the report must say so.

## 1. Question

Five coding agents work at the same time on one repository. Does `treeyard connect` (arm B) lower the
human work that conflicts cause, compared with one plain `git worktree` per agent and merges in
sequence (arm A)? Does it keep correctness? What does it cost on tasks that do not collide?

## 2. Hypotheses

- H1 (colliding sets): arm B needs at most half the human interventions of arm A.
- H2 (all sets): the final correctness of arm B is not below arm A.
- H3 (disjoint sets): arm B is not slower and not dearer than arm A by more than 10 %.

## 3. Design

- 3 repositories, pinned in [repos.json](repos.json): requests `611c6162` (small), ripgrep `3fce3b5`
  (medium), astro `c75e8971` (monorepo).
- 2 task sets per repository, 5 tasks each:
  - `disjoint`: the 5 tasks edit 5 different files.
  - `colliding`: the 5 tasks edit one function on purpose. In git, the reference patches of 6 of 10
    pairs (requests, ripgrep) and 10 of 10 pairs (astro) conflict (`t2.py validate` counts them).
- 2 arms, 3 repetitions per cell: 3 x 2 x 2 x 3 = 36 runs.
- 5 agents at once in each run, one task each: 2 Claude Code, 2 Codex, 1 OpenCode. The slot order is
  `claude, claude, codex, codex, opencode`. It rotates by one slot per repetition (`MIX` in t2.py).
- Models: the default model of each CLI on the day of the pilot, recorded in the run file. OpenCode uses
  `opencode/deepseek-v4.1-flash-free` (free, no login). The pilot fixes the models for the full run.

### Arm A

1. The runner clones the pinned commit into a new repository (branch `main`, no remote).
2. It makes one `git worktree` and one branch `agent-<k>` per agent. The agent works in its worktree.
3. When all 5 agents stop, the runner commits the work that an agent left uncommitted.
4. The runner merges `agent-1` to `agent-5` into `main` in this order (`git merge --no-ff`).
5. On a merge conflict, the runner aborts the merge and gives the conflict back to the agent once
   (hand-back text A, section 6), within the agent's limits. Then it merges again.

### Arm B

1. The runner clones the pinned commit the same way and runs `treeyard connect --harness
   claude,codex,opencode --agents 5 --no-mirror` on a free loopback port (never 7420).
2. Each agent gets a TreeYard workspace from `treeyard workspace-hook create` (Workspace mode) and
   works in it. Its MCP bridge is `treeyard mcp` with a fixed agent name per slot.
3. The prompt is the instruction block of `connect` ([block-b.md](block-b.md), sha256 of the text
   between the markers `3fd268440d0b22471c232d768bcf667bcfc7606017fd15de21e5d19a4604f898`), one empty
   line, and the task prompt. The runner refuses a run when `connect` writes another block.
4. When all 5 agents stop, the runner lands every open change of each agent, in slot order.
5. On a land conflict, the runner gives the conflict back to the agent once (hand-back text B), within
   the agent's limits. Then it lands again.

The task prompts are identical in both arms. The only difference in the agent's input is the block.
The block names `apply` as the way to edit. A Workspace agent can edit the files on disk or call `apply`.
Both paths reach main: the runner lands the workspace change and any other open change of the agent.

## 4. Frozen inputs

- Task prompts: `tasks/<repo>/<set>/<k>-<id>/prompt.md`. No prompt names TreeYard (`test_t2.py` checks
  it). `tasks/MANIFEST.json` holds the sha256 of every task file. `test_t2.py` fails when a file differs.
- Hidden deciding tests: `tasks/.../hidden/<path in the repo>`. The runner copies them into the final
  tree only at scoring. On the pinned commit each test fails. With `reference.patch` it passes
  (`results/validate.txt`).
- Reference solutions: `reference.patch` (task alone) and, for colliding sets, `resolved-<k>.patch`
  (tasks 1 to k-1 applied, then task k). No agent sees them. They live in this kit, outside the run
  repositories.
- Limits per agent: 80 turns, 4 000 000 tokens (all input tokens, cached or not, plus output tokens),
  1800 s wall clock. Claude Code also gets `--max-turns 80 --max-budget-usd 3.0`. The runner reads the
  usage events of each CLI and kills the process group at a limit.
- Config isolation: `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and the `XDG_*` directories of OpenCode point to a
  directory of the run. The runner hashes the user config files before and after each run
  (`user_config_unchanged` in the run file).

The sha256 of each prompt:

| Prompt | sha256 |
|---|---|
| `tasks/astro/colliding/1-c1-ext-query/prompt.md` | `c6b64e4ac89c542fd6ef15fcebe6172104cf94fb62b792695873d9f925d73f6f` |
| `tasks/astro/colliding/2-c2-ext-dotfile/prompt.md` | `a14dd638174e860079f1f60a01f8190b8e23fae03e79df30e19f0319d94cb7e6` |
| `tasks/astro/colliding/3-c3-ext-dir-dot/prompt.md` | `dbbab5e1ce9e7ae84af038bae49920d67ddb300499fe2c34187b41d9006e7923` |
| `tasks/astro/colliding/4-c4-ext-trailing-dot/prompt.md` | `1727b965189f70eb83dd2a1cb68ed54d16884f9a2164d5996c25758eb4592eb3` |
| `tasks/astro/colliding/5-c5-ext-dts/prompt.md` | `f425439b30b1934e5360ce4da08edcf169c3ecee00dd4c4abdb5fe8c895e2d51` |
| `tasks/astro/disjoint/1-d1-package-name-length/prompt.md` | `a8f30e49272379a7a090bc2c1eeff20c1defc80aed5fd4e292fa1f6f525ae786` |
| `tasks/astro/disjoint/2-d2-forwarded-skip-empty/prompt.md` | `de185ddab024af4b7a5407fc8627e98b687eb73029cc290164b47ea319082b03` |
| `tasks/astro/disjoint/3-d3-protocol-colon/prompt.md` | `e62cdf1d964a2eaaffd168b0bd4579a5c28c5a2917b919502d665e4d06bd329e` |
| `tasks/astro/disjoint/4-d4-remove-hash/prompt.md` | `84262043b5c499034dca813ea0f85caf20c3c45616c67b67475f2d3dd674d361` |
| `tasks/astro/disjoint/5-d5-get-by-path/prompt.md` | `c08fe70022faa58d82594397dfd4b48a3e88a588cec3a9bed231f75c2a375962` |
| `tasks/requests/colliding/1-c1-rel-lower/prompt.md` | `5fe14bd88b3718da1016875d351be122922dda07496a638bf9b1812538e23904` |
| `tasks/requests/colliding/2-c2-skip-empty-url/prompt.md` | `39019788f6a5acfbb58fb15c075911506aade194b72b5ed9642f02bd3fb7eb5c` |
| `tasks/requests/colliding/3-c3-lower-keys/prompt.md` | `1e755bfd97115cf7ab2233ec5b0470126a37fe511a6253536234f90dd4418f71` |
| `tasks/requests/colliding/4-c4-drop-duplicates/prompt.md` | `ca36fde476f2a68dae2a4aa698629440dd8dfab22fd1bdf8e8904b3ec9377045` |
| `tasks/requests/colliding/5-c5-valueless-params/prompt.md` | `ab1a1276a15624c7d5aaef0196f301d88e18a79ebc0cf322023319ff2293bc4d` |
| `tasks/requests/disjoint/1-d1-cid-union/prompt.md` | `249dbeaf00b81e8fde5065e8c730fc9329a2a5b90ea338bee3a0c95de5760c9b` |
| `tasks/requests/disjoint/2-d2-status-reason/prompt.md` | `754173ab25dc96bdc04122ca59cc80966b469a19f1b6e24bb1aecbaa2ec745b1` |
| `tasks/requests/disjoint/3-d3-basic-auth-repr/prompt.md` | `712865874cd7158fa819106b197ac8a9c153811a5378fbec6974408b8d5cf0f0` |
| `tasks/requests/disjoint/4-d4-netmask-range/prompt.md` | `cded7cea2ae83b7b65daac67d380d32629773bbe55e6739fbf05d5db2a7024fe` |
| `tasks/requests/disjoint/5-d5-native-memoryview/prompt.md` | `a40aa20acf4bc07e32b8609f5a1ff14ac942c9fdece899396e2efe36093a138f` |
| `tasks/ripgrep/colliding/1-c1-size-lowercase/prompt.md` | `243ebdcea45f983a9900542097c71478d630ed3eea1042250f8dc6d319973541` |
| `tasks/ripgrep/colliding/2-c2-size-terabytes/prompt.md` | `632af7fd2a1ca28e091a63fd04f37475a96dd85d0f0faf29d73dfc9c3b1a24f7` |
| `tasks/ripgrep/colliding/3-c3-size-bytes/prompt.md` | `6bfb4b7e912e437e9c310aca6e01fbe7a8f51ed576f0ef8b3f2e103f2da3bc8f` |
| `tasks/ripgrep/colliding/4-c4-size-space/prompt.md` | `af1a300cccbafd762463c7aa8e4488a4af1b3ecf45cbc17e0e61cf7e03cc4379` |
| `tasks/ripgrep/colliding/5-c5-size-binary/prompt.md` | `0d150208c633a20c93b9ab0ed697c371ee10ba4e915288c88ae847728625f6ef` |
| `tasks/ripgrep/disjoint/1-d1-type-roc/prompt.md` | `97854fa4d5a337d524c65249b5836ed17005cbd10a5624344cb56c7bb32dd0d3` |
| `tasks/ripgrep/disjoint/2-d2-globset-first-match/prompt.md` | `142ab1111ed6f6d1450b082714bc6d41d6479c1288e5a877b73fcd055c956669` |
| `tasks/ripgrep/disjoint/3-d3-stats-without-match/prompt.md` | `0bb792170a765823cb8b6d11fd0cd7f4ed30f4aea876d9e53e39cd36d2cfde73` |
| `tasks/ripgrep/disjoint/4-d4-match-overlaps/prompt.md` | `27d0681e95e0491e9148a236b73088f35a6bf5edf01ae289d7f89d09526ff141` |
| `tasks/ripgrep/disjoint/5-d5-glob-has-wildcards/prompt.md` | `032a7d02b062e68ed81974a0ac4011bd8ae39e9263e7a39ae303d341a2c20853` |

## 5. Metrics (one value per run, from the run file `results/<run>.json`)

| Metric | Field | Operational rule |
|---|---|---|
| Repository tests green at the end | `suite_green` | The `suite` command of repos.json passes on final main, before the hidden tests go in. |
| Tasks on main | `tasks_on_main` | The number of the 5 hidden deciding tests that pass on final main. |
| Conflicts at merge or land | `merge_conflicts` | A: merges with a non-zero `git merge` exit at the first try. B: `conflict` events with cause `land`. |
| Conflicts reported before land | `warned_before_land` | B: agents with a `collision` event of kind `overlap` or `conflict` before their first land. A: 0. |
| Agent reaction | `reacted` | B: warned agents with an `apply` or `resolve` of their change after the apply that caused the warning. |
| Human interventions | `interventions` | See below. |
| Wall time | `wall_s` | From the start of the agents to the end of the last merge or land, in seconds. |
| Lost changes | `lost_lines` | Lines that an agent added (its own diff, 4 or more characters, one word character) and that final main does not have. |
| Tokens and cost | `tokens_in`, `tokens_out`, `cost_usd` | The sum over the 5 agents, from the usage events of each CLI. `cost_usd` is the cost that the CLI reports. |
| False alarms | `false_alarms` | B: warned agent pairs whose reference patches do not conflict in git (`pair_table` in t2.py). |

Rule for a human intervention:

- The runner stands in for a person. After a conflict, the agent gets one hand-back. When the merge or
  land still fails, or the merged tree still has conflict markers, the runner takes the agent's side of
  each conflict (A: `git merge -X theirs`; B: `resolve` with `take = ours`). That is 1 intervention.
- Any step that the operator does by hand during a run is 1 intervention too. The operator writes it in
  `results/interventions-manual.txt` (run, time, what).

Known limits of the rules: `reacted` matches an event to an audit row by the second of the event. The
`false_alarms` rule uses the designed overlap of the reference patches, not the agents' own diffs.

## 6. Hand-back texts (frozen)

- A: "Your branch `agent-<k>` does not merge cleanly into `main`: git reports a conflict. Run
  `git merge main` in this directory, resolve every conflict so that your work and the work on `main` are
  both kept, and commit the merge."
- B: "Your change does not land cleanly on main: it has a conflict with work that landed first. The
  files in this directory now hold that work with conflict markers. Resolve every conflict so that your
  work and the landed work are both kept, then run `treeyard workspace land <name> --url <url>`."

The exact strings are `HANDBACK_A` and `HANDBACK_B` in t2.py. The runner resumes the agent's own session
(`claude --resume`, `codex exec resume`, `opencode run --session`).

## 7. Success threshold

All three conditions must hold over the 36 runs. Sums are over the runs of each arm in the named sets.

1. Colliding sets: the sum of `interventions` in B is at most half the sum in A. When the sum in A is 0,
   this condition fails (no effect is shown).
2. All sets: the sum of `tasks_on_main` in B is at least the sum in A, and the count of runs with
   `suite_green` in B is at least the count in A.
3. Disjoint sets: the median `wall_s` of B is at most 1.10 x the median of A. The sum of tokens (in plus
   out) of B is at most 1.10 x the sum of A. When both arms report a cost, the same rule applies to
   `cost_usd`.

The report gives every metric per run and per cell, with no significance test (3 repetitions per cell).

## 8. Stop threshold

The operator checks these after the pilot and after each repository. When one holds, the experiment
stops and the report gives the reason.

1. Agents ignore warnings in more than half the cases: in B, the sum of `reacted` is below half the sum of
   `warned_before_land`. The check needs 6 or more warned agents.
2. Token overhead above 20 % with no gain in correctness: in the same cells, the B tokens are above 1.20 x
   the A tokens, and the B `tasks_on_main` sum is not above the A sum.
3. A user config hash changes (`user_config_unchanged` is false), or any paid call happens that the
   budget does not cover.

The runner itself stops a run (exit 3, `stop_reason`) on an anomaly: the agent count is not 5, the arm B
server does not answer `/health`, or an agent makes zero tool calls. A stopped run is run again once with
the same arguments. A second stop of the same cell stops the experiment.

## 9. Pilot

- Repository requests, both arms, both sets, 3 repetitions: 2 x 2 x 3 = 12 runs.
- Order: for repetition r, the cells `disjoint` then `colliding`; arm A first in repetitions 1 and 3,
  arm B first in repetition 2. One run at a time.
- After the pilot: check the stop threshold. Then compute the budget with
  `python3 bench/t2/t2.py budget --pilot <dir with the 12 run files>`.
- The pilot runs are part of the 36 runs when no input of section 4 changes. Else they are exploratory.

## 10. Budget

```
full budget = mean pilot cost per run x 12 x (sum of the repo factors) x 1.25
repo factor = (reference diff lines of the repo) / (reference diff lines of requests)
```

Each repository has 12 runs, so 12 x the sum of 3 factors covers the 36 runs, and 1.25 is the margin.
The reference diff lines are the `+` and `-` lines of the 10 `reference.patch` files of a repository.
`t2.py budget` prints the factors, the mean tokens per run and the total. The owner approves the total
before the runs on ripgrep and astro.

## 11. Rules for the operator

- Run the kit from a copy outside the TreeYard worktree. In T1, agents read `ps` output and the driver
  path, so a copy keeps the reference patches out of the agents' way.
- No pilot and no real run without the owner's yes and a budget. `t2.py run --harness real` refuses to
  start without `--yes-real`.
- 5 agents at once is the design. The current harness rule allows at most 2 real-agent runs at a time.
  The owner must allow 5 for this experiment.
- `t2.py run ... --harness real --dry-run` sets up the arm and prints the 5 commands with no agent
  started. Run it before the pilot.
- Do not tune the block or the prompts on T2 tasks. A new block version needs a new preregistration.
