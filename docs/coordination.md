# Coordination (phase 3)

TreeYard sees every agent's edits while the agents work. Phase 3 uses that to make conflicts appear
early and small, and to land without a global lock. Six parts, each configured per repo in
`<data>/treeyard.toml` (a commented template is written by `treeyard init`; restart `serve` after a
change). The file lives in the data directory and not in the repo tree, because it names commands that
the server runs: an agent's change must never change them.

| Part | Operations and events | Code | Test |
|---|---|---|---|
| (a) Intents | `open_change` / `declare` with `paths` (globs) and `symbols`; reply `overlaps`, `advice`; event `intent_overlap` | `src/coord.rs` | `a_intents_report_overlaps_at_open` |
| (b) Live collisions | `apply` reply `collisions`; `undoes` in `apply`, `resolve`, `change`, `land` (a land that removes another agent's landed lines is held: status `undoes`, override `undo` + `reason`); events `collision`, `undoes`, `undo_override`; `events` (poll, long poll), `GET /api/events` (server-sent events) | `src/coord.rs`, `src/server.rs` | `b_live_collisions_trial_merge_and_events`, `g_apply_warns_when_it_undoes_another_agents_landed_lines`, `tests/undoes.rs` |
| (c) Continuous rebase | `land` reply `rebased`; events `rebased`, `conflict`; `apply` `seen` | `src/repo.rs` | `c_continuous_rebase_…` |
| (d) Merge drivers | `land` reply `drivers` | `src/drivers.rs`, `src/repo.rs` | `d_merge_drivers_…`, unit tests in `src/drivers.rs` |
| (e) Landing modes | `land` (queued), `queue`, `queue_run`, `test_main`; events `queued`, `rejected`, `test_failed`, `reverted`, `test_passed`, `test_run` (one per test run, with `seconds`) | `src/landing.rs` | `e_queue_mode_…`, `e_optimistic_mode_…` |
| (f) Semantic guard | `reviews`, `review_close`; `land` reply `reviews`; event `review` | `src/coord.rs`, `src/repo.rs` | `f_guard_…`, `tests/replay.rs` |
| (g) Shared directories | `assign`, `claim`, `apply_assigned`; `treeyard share` | `src/hunks.rs`, `src/share.rs` | `tests/share.rs` |

All tests are in `tests/coordination.rs` unless named otherwise. Every operation is in the one table in
`src/rpc.rs`, so HTTP, MCP and `treeyard call` get it at once.

## Demo (two agents, one function)

`/private/tmp/rootstock-harness/009/demo.sh` (not in the repo; the commands are below) ran on
2026-10-17 against `treeyard serve` with `continuous_rebase = true`:

```sh
treeyard call open_change '{"agent":"alice","intent":"fix parse","paths":["lib.rs"],"symbols":["parse"]}'
treeyard call open_change '{"agent":"bob","intent":"speed up parse","symbols":["parse"]}'
treeyard call apply '{"change":"<alice>","edits":[{"path":"lib.rs","replace":{"old":"    1","new":"    10"}}]}'
treeyard call apply '{"change":"<bob>","edits":[{"path":"lib.rs","replace":{"old":"    1","new":"    100"}}]}'
treeyard call land '{"change":"<alice>"}'
treeyard call events '{"change":"<bob>","kinds":["collision","conflict"]}'
```

Output (fields picked by the script):

```
alice opened ch_9e5229d6155f
bob open_change overlaps: [('ch_9e5229d6155f', ['parse'])] | 1 open change(s) plan to touch the same code. To avoid a late conflict: wait for them to land (watch events for kind landed), or stack on one with open_change base = ch_9e5229d6155f, or narrow your paths.
bob apply collisions: [('ch_9e5229d6155f', 'conflict', {'mine': [[2, 3]], 'theirs': [[2, 3]]})]
alice land: landed | rebased: [('ch_5d9244f23620', ['lib.rs'])]
events for bob: [(4, 'collision', None, 'symbol'), (5, 'collision', None, 'conflict'), (8, 'conflict', 'continuous_rebase', None)]
bob conflict task: [('lib.rs', 'content', 'alice')]
```

Bob learns three times before he finishes: at `open_change` (Alice declared `parse`), at his first
`apply` (a trial merge with Alice's change conflicts on line 2), and when Alice lands (his change now
holds the conflict as a task naming Alice).

## (a) Intents

`open_change` takes `paths` (globs, gitignore style: a pattern without `/` matches at any depth, a
leading `/` anchors it at the root, a plain directory name covers everything below it) and `symbols`
(`name` or `path#name`). `declare` replaces them later. The reply lists every open change that
overlaps:

- paths: my globs match its edited paths, or both declared globs match the same file of main;
- symbols: it declared the same name, or one of its changed hunks lies inside the definition of the
  symbol in its base. For Rust, Python, Go, JavaScript and TypeScript the definition comes from
  tree-sitter (`src/symbols.rs`). Otherwise a regex for common keywords (`fn`, `struct`, `impl`,
  `def`, `class`, `function`, …) finds it, and it ends at the next line at the same or smaller indent.

Each overlap carries `via` (`edited` or `declared` per path). `advice` names the choices: wait, stack
(`base = <change>`), or narrow the paths. The server never blocks an open; the orchestrator decides.

## (b) Live collisions and events

Every `apply` (the workspace daemon's `sync` too, since it calls `apply`) checks the change against all
other open changes. A shared path gets two trial landings on today's main, one per order: the first
change merges onto main, then the second onto that result, the same merges `land` runs, with nothing
stored. The reply's `collisions` gives `kind` = `conflict` or `overlap` per path and a `reason` for a
conflict: `landing_order` (the change that lands second conflicts), `you_conflict_with_main` or
`they_conflict_with_main` (one side cannot land on main even alone). `if_you_land_second` and
`if_they_land_second` give the outcome of each order (`null` when the first one already conflicts with
main). A `landing_order` conflict carries `advice: {"stack_on": "<other change>"}`: `rebase {change, onto}`
stacks the change on that open change (its edits merge in now, `land` waits until it has landed; two
changes cannot stack on each other). A change that no longer merges onto main by itself gets one
collision with `"change": "main"` instead of one per agent. A clean `overlap` whose edits on both sides are more than 3 lines apart is not reported (it
merges either way); the shared path still shows in `overlaps`. It also gives the changed line ranges of both sides (`hunks.mine`,
`hunks.theirs`, half-open, 1-based, in base lines). Edits inside another change's declared paths
(`declared`) or symbols (`symbol`) count too. Each new or changed collision of a pair and path is
published once as a `collision` event.

A clean change can still remove another agent's work: the other change landed first, this one was
opened on top of it, so there is nothing to merge and no conflict. `undoes` lists such lines. A line of
the change's base counts when all of this holds:

1. the change's base-to-head diff deletes or rewrites it;
2. by blame at the base, it comes from a change of another agent that has landed, or that is still open
   (the change this one is stacked on; the entry then has `"landed": false`). The git import, abandoned
   changes and the caller's own work do not count: the same agent name, or the same non-empty `session`
   (`open_change` `session`);
3. its text is gone from the change's new tree. Lines are compared without whitespace, across all paths
   the change edits: the line must occur fewer times in their head versions than in their base versions.
   So a block that moved (into a helper, to another file, a rename), was re-indented (wrapped in `try`),
   changed only its line ends (CRLF to LF), or gained a newline at the end of the file, is not reported;
4. what the other change contributed to the line is gone. The contribution comes from that change's
   own diff of the path (the hunk in which it added or rewrote the line): the words of the line
   (identifiers, numbers, words of strings) that are not among the words of the lines that hunk
   replaced, one occurrence each. A line it added with no counterpart (or a rewrite that added no word
   of its own: an operator, a new order) contributes every word. The line is quiet when the
   contributed words, in order, all appear on one added line of the current diff hunk or on two
   adjacent ones. So an agent that rewrites a line another agent rewrote keeps that agent's part and
   still does its own task: agent 1 made `v = v.casefold()` into `v = v.strip().casefold()`
   (contribution `strip`); `v = v.strip().lower()` is quiet, `v = v.casefold()` or `v = v.lower()`
   is reported. For a line the other agent added whole this is the earlier rule: a rewrite that keeps
   every word adds to it (`value.lower()` to `value.lower().strip()`, `link[key] = value` to
   `link[key.lower()] = value`, quiet); one that drops a word (`value = value.lower()` to
   `value = value`) is reported. Operators are not words, so `x > 0` to `x >= 0` is not reported;
5. several changes on one line: blame names only the last one. The lines its hunk replaced are
   followed back (blame at its parent, up to 3 steps, 8 replaced lines per step): an earlier change of
   another agent whose contributed words the later line still carries contributed them too. Dropping
   them is reported with that earlier change as the owner, keeping them is quiet. Example: agent 4
   landed `v.strip().lower()` over agent 1's `v.strip().casefold()`; a third agent's `v.lower()` loses
   agent 1's `strip` (entry of agent 1), its `v.strip().casefold()` loses agent 4's `lower` (entry of
   agent 4). Contributions are words, so an earlier change whose words the later one removed is not
   followed (the later change undid it, not this one);
6. it holds a letter or a digit (a lone `}` is never an entry alone, but joins a run of reported lines).

Why the contribution rule: in two real 5-agent runs agent 1 (strip spaces) landed
`normed_value = normed_value.strip().casefold()` over `normed_value.casefold()`; agent 4 (use `lower()`)
got a land conflict and resolved it correctly to `.strip().lower()`. Blame gave the whole line to agent
1, the old rule wanted every word of it (`casefold` too), so `undoes` reported it, and agent 4 followed
the advice, put `.strip().casefold()` back and landed nothing of its own task: 2 of 3 runs lost a task
to the warning.

```json
{"path": "norm.py", "lines": [3, 3], "agent": "agent-1", "intent": "strip spaces",
 "changeset": "<origin changeset>", "change": "ch_...", "text": "    normed_value = normed_value.strip().casefold()\n",
 "contributed": [{"line": 3, "words": ["strip"], "before": "    normed_value = normed_value.casefold()\n",
                  "after": "    normed_value = normed_value.strip().casefold()\n"}]}
```

`lines` is an inclusive, 1-based range in the base file; adjacent lines with the same owner form one
entry; `text` is the removed text, at most 8 lines and 800 bytes (`text_cut: true` past that).
`contributed` lists, per line of the entry that the owner rewrote rather than added whole, the words it
contributed and its own `before` (the lines its hunk replaced, at most 300 bytes) and `after` (its
version of the line), so the agent can keep exactly that part; a line the owner added whole has no item
(its whole text is the contribution). The reply carries the advice once (`undoes_advice`: keep what the
other agent contributed and still make your own edit of the line), at most 20 entries (`undoes_more: N`
counts the rest), and only entries the caller may read (token read globs).

Which replies carry it:

- `apply` and `resolve`: only entries that no earlier `apply` or `resolve` reply of the change sent;
  `undoes_earlier: N` counts the ones sent before that still stand. An entry that goes away (the agent put
  the lines back) and comes back is sent again. `resolve` with `take = ours` after a land conflict is the
  usual way to drop a line that just landed.
- `change`: every entry that stands now.
- `land`: every entry, after the rebase. A land with entries of lines another agent landed is held
  (see below); a `landed` or `queued` reply lists the entries that did not hold it (lines of a change
  this one was stacked on) and those of an override.

**The land hold.** A warning in a reply does not stop an agent: in a 5-agent run, agent B settled a
conflict by removing the feature agent A had landed (8 lines) and rewrote A's test to assert the
opposite; `undoes` was in B's `apply` reply and again in its `land` reply (status `landed`), and B said
so only in its final message. So `land` does not land such a change. It answers

```json
{"status": "undoes", "change": "ch_b", "undoes": [{"path": "link.py", "lines": [5, 6], "agent": "a", "...": "..."}],
 "advice": "Not landed: your change removes these lines, which another agent landed for its own task. ..."}
```

and main does not move (the change stays rebased onto main, like a conflict). The advice: put the lines
back and land again; if the task and these lines truly cannot both hold, keep the other agent's lines,
land the rest, and say so in the final message so a person decides; only if the task names these lines
for removal, land again with `undo: true` and a non-empty `reason` (`land {change, undo, reason}`;
`undo` without a reason, or a reason without `undo`, is an error). What holds a land: entries of a
landed change of another agent, among all entries (not only the 20 a reply shows; a held reply lists
at most 20 and counts the rest in `undoes_more`). What does not: the agent's own lines (agent name or `session`), moved,
re-indented or extended lines (they are no entries), lines of an open change this one is stacked on
(`landed: false`) or was stacked on when it edited them (`stacked_on` in the change), and lines a
`revert` put back. A revert and a human `git push` are never held.

An overridden land lands and is recorded, so people see it without reading agent transcripts: `undid`
(`by`, `reason`, `time`, `head`, `undoes`) in the `land` reply and in the change (`change`); the event
`undo_override` (`by`, `reason`, `agents` whose lines went, `undoes` without text; `changes` names the
landing change and the changes whose lines went); `status` lists the last 20 (`undid`, `undid_total`);
`log` entries of such a change carry `undid`; the audit row of the land says `undid: B removed landed
lines of A (ch_a); reason: ...`. A held land publishes the event `undoes` (`cause` = `land` or `queue`)
and writes an audit row with status `undoes`. An entry whose path the caller may not read is left out
of a held reply and counted in `undoes_hidden`.

Queue mode: `land` holds at enqueue (not queued) or, with `undo` + `reason`, enqueues with the yes on the
change's head. Before each batch the queue checks every change again (the per-path scans are kept, so an
unchanged change costs no diff or blame): a change whose head moved after the yes, or that has entries
without a yes, leaves the queue with the note `undoes`, the event `undoes` (`cause: queue`) and a review
task `undoes`; `land_status` then says `undoes`. A later `land` without `undo` of a change queued with a
yes for its current head is not held again (the yes stands; it answers `queued`).

Cost: with no entries (the normal case) a land computes `undoes` once, before it moves main, where it
was computed after the land before; with no rebase it now reuses the scans the change's `apply` calls
kept. The hold adds one lookup per owner change only when there are entries.

Setting: `[coordination] hold_undoes = false` in `treeyard.toml` turns the hold off: the land lands
and its reply lists `undoes`, as before. CLI: `treeyard workspace land NAME --undo REASON` is the
override; on status `undoes` it updates the workspace to the change as the server holds it and prints
the reply (it never asks). Through `treeyard call land '{"change": ..., "undo": true, "reason": ...}'`
as any other parameter.

Cost: per open change, the diff and blame of each path are kept while the change's base and the path's
head blob stay the same, so an `apply` to another file reuses them. The contribution rule reads the
owner's own diff of the path only for removed lines of other agents (none in the normal case); those
diffs are kept per (changeset, path) for the server process (up to 512, content-addressed, never stale),
and following a line back costs one cached blame at the owner's parent per step. With a 60 000-line file, the
`apply` after the one that removed a line of it took 4 to 5 ms instead of about 60 ms (debug build, one
machine). The memory is per server process: after a restart an entry is sent once more.

What cannot be decided: agent names are free strings, so one agent under two names without a shared
`session` (or token) looks like two agents; a semantic removal that keeps every word (an operator, an
argument order) is not reported; a moved line that lands in a file the change does not edit (a line
that already existed elsewhere) does not hide a removal, but a duplicate in an edited file does.
Contributions are words, so a contributed word that is common on the line (`self`, a variable that
the replaced lines did not have) can be kept by accident; an earlier contribution is followed only
through lines the later change's hunk replaced, at most 3 changes back; the caller's own earlier
rewrite of another agent's line counts as the caller's line (blame's origin is the caller), so the
other agent's part in it is not followed; a contribution that only changed an operator falls back to
the whole line.

The case comes from a pilot with 5 agents: one agent's task ("parameter names in lower case, values keep
their case") replaced a block that another agent had landed minutes before
(`if key.lower() == "rel": value = value.lower()`), and nothing warned it. Tests: `tests/undoes.rs` (one
per probed case, the 20-entry cut, the read-glob filter, the line both agents edit, a hold past 20 entries,
a repeated queued land), `g_apply_warns_…` in `tests/coordination.rs`.

Events: `opened`, `intent_overlap`, `collision`, `conflict` (`cause` = `land`, `continuous_rebase`,
`stale_write` or `queue`), `rebased`, `landed`, `abandoned`, `review`, `queued`, `rejected`,
`test_failed`, `reverted`, `test_passed`, `undoes` (a land held, `cause` = `land` or `queue`),
`undo_override` (a land with `undo` + `reason`). Each event has `seq`, `time`, `kind`, `changes` and `agents`.

- `events {since, change?, agent?, kinds?, limit?, wait_ms?}`: poll; `wait_ms` (max 30 000) makes it a
  long poll. Pass the reply's `next` as the next `since`. MCP clients use this one.
- `GET /api/events?since=N&change=…&agent=…&kinds=a,b`: server-sent events. The first message is
  `hello` with `epoch`; each event has `id` = seq, so `Last-Event-ID` resumes.

Events live in memory (the last 20 000). A restart starts a new `epoch` at seq 0. The reply says
`dropped: true` when the client fell behind the kept window.

## (c) Continuous rebase

With `[coordination] continuous_rebase = true`, every land rebases each other open change onto the new
main, except changes that are stacked on an open change, queued, or already conflicted. A conflict
appears on the change at once, as a task, with a `conflict` event (`cause: continuous_rebase`).

The owner's files can be stale after such a rebase. The server keeps the head that the owner saw
(`view`). A later whole-file write (`content`, `content_b64`, `delete`) to a path that changed since
then is merged 3-way (view, write, current head) instead of overwriting main's edits. `replace` edits
work on the current text and need no merge. A client that read the current head passes it as `seen`;
that write goes in as is and clears `view`. Test `c_…` checks that a stale whole-file write keeps both
edits.

### Conflicts as regions

Every reply that carries a change (`change`, `apply`, `resolve`, `rebase`, `declare`, `land`,
`restore_change`) lists each conflict as the parts that need a decision, not as three whole files.
The path as the change holds it is the merge with markers (`<<<<<<< main` .. `=======` is main's side,
`=======` .. `>>>>>>> change` the change's); each marker block is one region:

```json
{"path": "lib.rs", "kind": "content", "base": {"blob": "…", "mode": "100644", "size": 81234},
 "ours": {…}, "theirs": {…}, "theirs_by": [{"agent": "b", "intent": "…", …}],
 "regions": [{"lines": [412, 418], "text": "<<<<<<< main\n…>>>>>>> change\n", "before": "3 lines", "after": "3 lines"}]}
```

`lines` is 1-based and inclusive in that text (`read` with `at` = the change id returns the same lines).
To resolve, one `apply` with one `replace` per region (`old` = the region's `text`) or the whole content,
or `resolve` with `take`. The change carries the help text once (`resolve`), not once per conflict.
Conflicts without markers (`modify_delete`, `mode`, `binary`) have no regions; their sides come inline
as `text` when each is at most 4 KiB. Bounds: 80 lines per region (`cut: true` past that), 12 regions per
path (`regions_more: N`), 48 KiB of region text per reply; when any of these cut, `conflicts_cut` says
so and how to get the rest. `change {change, full: true}` adds `text` with the whole `base`, `ours` and
`theirs` (up to 256 KiB each), as every reply did before. Why: in 87 pilot calls of `change`, `rebase`
and `declare`, 91 to 98 % of 2.16 MB of replies were the three whole texts, and one `land` reply of
115 540 characters was refused by the client as too large. Tests: `tests/conflicts.rs`
(`conflicts_come_as_regions_that_resolve_with_replace`), unit tests in `src/conflict_view.rs`.

### Conflicts that do not block

By default a conflicted change stays on its base: continuous rebase skips it and `rebase` refuses
until every conflict is resolved. With `[coordination] keep_conflicted = true` it keeps moving:

- The rebase stores the conflict per path (`base`, `ours`, `theirs` entries) and the text with markers
  as the change's content of that path. The change shows `conflicted: true`.
- The agent keeps applying edits on top. Continuous rebase (and `rebase`) moves the change with main:
  each kept conflict is merged again from its own base and our side onto the new main, never from the
  text with markers. Main's later edits to the path join the conflict (`theirs`, `theirs_by`); a main
  that settles the path clears the conflict.
- `land` answers `status: conflict` until every conflict is resolved. Writing the path with `apply`
  (taken as is, not merged with the stale `view`) or calling `resolve` clears it.

A write to a conflicted path is its resolution, and it must not keep the markers (`<<<<<<< main`,
`>>>>>>> change`). The check runs once per path after all edits of one `apply`, so several `replace`
edits in the same call may remove the markers together. A write that leaves some gets an error with
the line, the number of conflict regions left, and the rule: one `apply` must remove every marker of
that file (whole `content`, all the `replace` edits at once, or `resolve`).

Tests: `tests/conflicts.rs` (both modes; several `replace` edits in one resolution).

### Following main without continuous rebase

`treeyard connect` leaves `continuous_rebase` off. With it on, the server would rewrite an agent's
change while the agent works: a conflict would arrive as markers in a file the agent is editing, and
its next `apply` there would be refused until it resolves them (in the pilots, 6 applies already failed
on markers after `land`). Off, the agent decides when it takes main's work, with one `rebase` call.

Instead, a `replace` whose old text the change lacks says why, in the error text:

- main has the text and the change is N landings behind main: another agent changed the file after
  the change's base; call `rebase`, then `read` with `at` = the change id, and apply again;
- main has the text and the change is not behind: the change already replaced it; read the file with
  `at` = the change id;
- otherwise: how far main is ahead, and read with `at` = the change id.

`read` with no `at` stays main, also in the MCP bridge. Defaulting it to the agent's open change would
match `apply`, but `grep`, `symbols` and `outline` answer for main, so their line numbers would no
longer fit what `read` returns, and the HTTP `/mcp` endpoint (no bridge) would differ from stdio. The
error above names the fix instead. Test: `tests/conflicts.rs`
(`a_replace_that_misses_says_how_far_behind_main_the_change_is`).

## (d) Merge drivers

Drivers run only where the line merge conflicts (content or add/add), so a clean merge is the same as
git's. First matching rule wins:

```toml
[merge]
syntax = "auto"                  # mergiraf for the rest when it runs; "mergiraf" = must run; "off"
# mergiraf = "/path/to/mergiraf"
[[merge.driver]]
paths = ["Cargo.lock"]
driver = "cargo-lock"            # 3-way per [[package]]; dependency lists merge as sets
[[merge.driver]]
paths = ["CHANGELOG.md", "HISTORY.md"]
driver = "union"                 # both sides' lines, ours first (git merge=union)
[[merge.driver]]
paths = ["package-lock.json", "pnpm-lock.yaml", "poetry.lock", "uv.lock"]
driver = "regenerate"
command = "pnpm install --lockfile-only --offline"
```

- `cargo-lock`: packages keyed by name, version and source. A package that one side added or removed
  follows that side. A package both sides changed merges per field. It stays a conflict when both sides
  change one field differently, or replace the same package version with different new versions.
  Adding `command` runs that command on the merged lockfile afterwards.
- `union`: git's union rule. git's own manual warns that it "tends to leave the added lines in the
  resulting file in random order" ([gitattributes](https://git-scm.com/docs/gitattributes), opened
  2026-10-17). Use it for changelogs and registries only.
- `regenerate`: writes the merged tree (main plus the change) to a temp dir under `<data>/tmp`, runs the
  command there with `TREEYARD_PATH` set, and reads the path back. A failing command leaves the
  conflict. The command runs under the repo lock (the limit is below).
- `ours` / `theirs`: take one side.
- `syntax`: [mergiraf](https://mergiraf.org/languages.html) (opened 2026-10-17; it lists Rust, Python,
  TypeScript, Go, Java and more) on the remaining conflicts. Test `d_…` resolves two `use` lines added
  at the same place in a Rust file with mergiraf 0.20.0 when `MERGIRAF=/path` is set. Without it, the
  test checks that the path stays a conflict.

Safety rule: drivers run only when the change's base is an ancestor of main. On another base (work main
does not have, as in `treeyard replay`'s stale-parent model), main looks as if it deleted that work, and a
driver would apply the deletion silently. The first replay with drivers on that model showed it:
conflicts fell from 111 to 49, but no wave matched history (21 of 32 before), because CHANGELOG.md and
Cargo.lock lost lines (run `t-drivers`, 2026-10-17, ripgrep at 16 agents).

## (e) Landing modes

```toml
[landing]
mode = "queue"                   # or "optimistic"; default "direct"
test = "cargo test --offline"    # sh -c in a temp copy of the tree; exit 0 = pass
batch = 8
parallel = 4                     # test runs side by side; default half the cores, at most 4
warm = true                      # default: one kept test dir per slot, only changed files written
timeout_s = 600
```

Tests run in `<data>/scratch/test-slot-N`, one kept directory per parallel slot (`TREEYARD_TEST_SLOT=N`).
Each run writes only the paths that changed since that slot's last run (and tracked files a test edited),
deletes paths that left the tree, and keeps untracked files, so `target/`, `node_modules` or a `.venv` built
by an earlier run stay warm and `cargo test` rebuilds only what changed. `warm = false` tests in a fresh
copy each time (test `e_warm_test_dirs_…`).

- `direct` (default): `land` moves main at once. No tests.
- `queue`: `land` returns `queued`. A step takes up to `batch` changes, rebases them one on top of the
  other without moving main, and tests the top. Pass: the batch lands. Fail: bisect the prefixes, land
  the longest passing prefix, and send the first failing change back to its owner (review task
  `tests_failed`, event `rejected`; the change stays open). A change that conflicts only with a
  batch-mate waits for the next batch; a conflict with main is real and goes to the owner. Test `e_queue…`:
  4 changes, the second breaks the test, 3 test runs, 1 lands, then the other 2 land in 1 run.
  With `parallel` above 1, a failing batch tests each of its changes alone on main, `parallel` at a time,
  and sends the failing ones back at once; the next step batches the rest again. A passing batch still
  costs one run. In scripted runs of 40 changes (6 s suite, 10 at a time): no broken change, 8 test runs and
  56 s (git queue 259 s); 1 change in 4 broken, 172 s with `parallel = 4`, 212 s with 2, 277 s with 1
  (git queue 329 s). Testing every change alone before its batch instead cost one run per change (44 runs
  for 50 changes in the wide50e cloud run, as many as the git queue).
- `optimistic`: `land` moves main at once. A step tests main. When it fails, it bisects the main
  changesets since the last green one, reverts the first failing changeset through the normal `revert`
  operation, and opens a `reverted_by_bisect` review task for its owner. Test `e_optimistic…`.
  When that revert conflicts (later changes edited the same lines), the culprit stays on main as `red`
  and its owner gets a `revert_failed` task: fix forward. While main is red, a step runs one test of main
  and no bisect; when the owner's newest changeset passes, main is green there again and the next step
  bisects only what landed after it. (The wide50 cloud run without this: every step bisected again, and
  later changes waited for a verdict until their agents gave up.)

`land_status {change}` tells an agent what happened after `land`: `verdict` = `passed`, `pending`,
`queued`, `rejected` (with the failing `output`), `reverted` (with the `output` that made bisect revert it),
`broke_main` (the same, but the revert conflicted), `main_red` (tested, but main is red from `red_change`),
`not_tested` (direct mode), `open` or `abandoned`. `treeyard workspace land` polls it, so an agent that
lands in optimistic mode learns about a revert before it stops.

`treeyard serve` starts a worker thread for `queue` and `optimistic`. The `queue_run` and `test_main`
operations run one step by hand (tests and CLI). Tests run outside the repo lock: agents keep reading
and applying during a test. Before the queue lands a batch it checks that main and each change are
unchanged; if not, the step reports `stale` and runs again. A `revert` and a human `git push` always land
directly.

## (f) Semantic guard

The replay found merges that are clean but wrong: a fix ("remove stray `dbg!`") lands before the
commit that added the `dbg!`, becomes a no-op, and the bug lands after it. Every rebase now checks each
of the change's own edits. If the result no longer carries a hunk of base → ours against main, that edit
"vanished": main had the same edit already, or main lacks the code the edit changed. Each such path
opens a review task `absorbed_edit` with the vanished hunks (unified diff text), `whole_file`, and the
main changesets that touched the path. A change that a rebase empties lands as `already_on_main` with a
second task `already_on_main`. Review tasks are signals for an external reviewer (RFC: "w v1 jako
sygnał dla zewnętrznego review"); `review_close {id, state: done | dismissed}` closes one.

In the replay of ripgrep at 16 agents (stale parents, 2026-10-17), the guard opened 1 `absorbed_edit`
task, and it is the `dbg!` fix: commit `e92e2ef8137a` "cli: remove stray `dbg!`", path
`crates/cli/src/lib.rs`, a path that is wrong at the end of its wave. Numbers for all runs:
[benchmarks.md](benchmarks.md#phase-3-coordination-replay).

## (g) Shared directories: one checkout, many agents

Sometimes several agents (for example Claude Code sessions) edit one checkout at the same time,
without workspaces. One `apply` of that directory would mix their edits into one change. A *share*
splits them per hunk instead:

```sh
treeyard share join --dir . --agent alice --intent "fix parse" --symbols parse
treeyard share join --dir . --agent bob --intent "new output" --paths "src/render/**"
treeyard share sync --dir .        # after edits; again whenever you like
treeyard share claim --dir . --agent bob h1a2b3c4d5e6
```

`join` opens one change per agent on the share base (main when the first agent joins) and keeps the
list in `DIR/.treeyard/share.json`. `sync` sends every file that differs from the base to `assign`,
then calls `apply_assigned` for each agent.

- **Hunks.** Each file's diff against the share base (git's Myers diff, `src/xdiff.rs`) gives hunks.
  A hunk id hashes the path, the removed and added lines and 2 base lines on each side. Line numbers
  are not in it, so an edit elsewhere in the file keeps the id.
- **Owner**, first rule that gives exactly one open change of the share:
  1. a `claim {change, hunks}` on the id, or a claim on the earlier hunk that this one grew from;
  2. intent: a declared symbol whose definition holds the hunk (in the base or the new text), then a
     declared path glob; a tie goes to the earlier owner, else the hunk is `ambiguous` with `candidates`;
  3. stickiness: the owner of the same id, or of the one earlier hunk it overlaps;
  4. else unassigned (`why: none`).
- **Dependencies.** A hunk that overlaps an earlier hunk of another change builds on that change's
  lines (one agent edited inside the other's new code). It gets `depends_on` and is held: it is not
  applied, and the other change keeps its older version (`kept`). A claim by that change settles it.
- **`apply_assigned {change}`** writes base plus the change's ready hunks to each path. When the head
  moved since the last write (a rebase, continuous rebase), the step from the last written text to the
  new one is merged 3-way onto the head; a conflict leaves the path out (`conflicts`).
- After a change lands, its hunks show as `landed` (still in the directory, now on main).
  `share sync --rebase` rebases every agent's change onto main and moves the share there.

`assign` replies with hunks per change and `unassigned` hunks; each hunk has `id`, `path`, `old` and
`new` line ranges (1-based, half-open), `owner`, `why` (`claim`, `intent`, `sticky`, `none`,
`ambiguous`, `landed`) and the first 40 diff lines. The idea comes from GitButler's hunk assignment
and hunk dependency locks (static inspection, 2026-10-18); the code here is our own.

Limits of (g): text files only (binary files and symlinks are skipped); file modes stay as in the
head; every sync hashes every tracked file of the directory; the server keeps the directory's changed
files in the share row (SQLite), so it suits source files, not large data. `assign` needs a token that
may write the paths; `claim` and `apply_assigned` need the change's owner.

## Limits

- Events are in memory only; a restart loses them (new `epoch`).
- Collision checks scan all open changes on each `apply`: O(open changes) trial merges per shared path.
  Measured cost is in the land and apply numbers of the replay, not separately.
- Symbol ranges come from tree-sitter for Rust, Python, Go, JavaScript and TypeScript (since phase 4,
  docs/derived.md); other languages, and names tree-sitter does not define, use the regex heuristic.
- `regenerate` and queue/optimistic tests copy the whole tree to a temp dir. `regenerate` runs under
  the repo lock, so a slow command blocks every agent for its duration.
- mergiraf can take seconds on large files: in an early wave-model replay one land took 5.2 s (run
  `t3-wfull`, land max 5216 ms). TreeYard passes `--timeout 5000` to cap one file.
- mergiraf prints a `mergiraf review <id>` hint after each solved conflict. Where it keeps that data
  was not checked.
- No auth: anyone who reaches the port can read events.
