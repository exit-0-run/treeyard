# T2 development task set

A second task set for the T2 kit, for one job: tuning the instruction block that arm B agents get
(today [`../block-b.md`](../block-b.md)). It is not a measurement. Its runs never count toward T2, and
the T2 tasks must not be used for tuning, so this set lives apart from the frozen T2 inputs: its own
[`repos.json`](repos.json), [`taskdefs.py`](taskdefs.py), `tasks/` and `results/`.

The pilot showed two things a block should address (see the analysis in
[`../results/pilot-v2/README.md`](../results/pilot-v2/README.md)): an agent removed lines that another
agent had landed because its own task wording seemed to forbid them (`apply` now reports `undoes`),
and whole-file `read` calls cost tokens (`read` now takes `start` and `end`). The tasks here are built
to exercise both.

## Repository

[pallets/click](https://github.com/pallets/click) 8.5.1, commit
`cf19179e8e2fd54c601378cec9e0d21bb12b6fb1`, BSD-3-Clause. Pure Python, no runtime dependencies; the
venv holds only pytest. The suite is `tests/` without `tests/test_deprecations.py` (that file reads the
installed package metadata, and the kit runs click from `src/`). It runs in about 4 s.

## Tasks

Disjoint: five files, all over 500 lines, each change deep in its file, so that reading a line range
is enough.

| Id | File (lines) | Change |
|---|---|---|
| `d1-hex-colors` | `src/click/termui.py` (1024) | `style` takes `#rrggbb` for `fg` and `bg` |
| `d2-empty-xdg` | `src/click/utils.py` (676) | `get_app_dir` treats an empty `XDG_CONFIG_HOME` as unset |
| `d3-pwsh-value-newlines` | `src/click/shell_completion.py` (796) | PowerShell completion collapses newlines in the value |
| `d4-argument-dots` | `src/click/core.py` (3825) | `@click.argument("src.path")` gets the name `src_path` |
| `d5-blank-editor` | `src/click/_termui_impl.py` (970) | `Editor.get_editor` skips a blank `VISUAL` or `EDITOR` |

Colliding: all five edit `Choice.normalize_choice` and `Choice.convert` in `src/click/types.py`.

| Id | Change |
|---|---|
| `c1-strip-spaces` | with `case_sensitive=False`, spaces around the value are ignored |
| `c2-unique-prefix` | a value of 3 or more characters that starts exactly one choice selects it |
| `c3-did-you-mean` | the error names the closest choice (`difflib`, cutoff 0.8) |
| `c4-lower-only` | case-insensitive matching uses `lower()`, not `casefold()` |
| `c5-ambiguous` | a value that matches two choices (`a` and `A` without case) is an error |

Two of these pairs are undo traps, built like `c1-rel-lower` and `c3-lower-keys` of the pilot. Task X
adds a narrow special case; the general rule in the prompt of task Y, read alone, seems to forbid the
lines of X. Both hidden tests pass together (the oracle run shows it), so an agent that removes the
other agent's lines loses a task.

- `c1-strip-spaces` (X) and `c4-lower-only` (Y): Y says that case-insensitive matching "ignores letter
  case and nothing else: every other character of the value must match the choice exactly". The
  `strip()` of X sits in the same `if not self.case_sensitive:` block of `normalize_choice`.
- `c2-unique-prefix` (X) and `c5-ambiguous` (Y): Y says that "a value is valid only when it matches
  exactly one choice, as a whole". The prefix match of X sits a few lines further down in `convert`.

Every hidden test fails on the pin and passes with its reference patch; all five reference patches of
a set apply together, with the suite green (`results/validate.txt`). In git, 1 of the 10 pairs of
colliding reference patches conflicts (`c2` and `c3`, both in the `except` branch of `convert`); the
others merge as text, which is what makes the traps silent in arm A.

## Commands

From the kit root (the worktree, or a copy of it outside the worktree as for T2), with a scratch
directory `S`. `--repo click` selects this set on every subcommand; without it the kit runs T2
exactly as before.

```sh
python3 bench/t2/t2.py setup --scratch S --repo click       # clone at the pin, venv S/t2/env/dev-venv
python3 bench/t2/t2.py generate --scratch S --repo click    # dev/tasks/ from dev/taskdefs.py
python3 bench/t2/t2.py validate --scratch S --repo click    # dev/results/validate.txt
python3 bench/t2/t2.py rehearse --scratch S --repo click    # oracle and noop runs of the 6 cells (arms A, A2, B)
python3 bench/t2/t2.py collect --results bench/t2/dev/results --summary bench/t2/dev/results/rehearsal-summary.txt
```

The run files of the 12 real runs of the block tuning are in `results/` and stay as they are; their
tokens were counted the old way (every Claude stream event). `recount --runs S/t2/runs --results
bench/t2/dev/results` restates them. For checks, write new run files elsewhere with `--results DIR`.
The prompt, the hand-back, the layout and the metrics are those of the T2 kit
([../README.md](../README.md)): the common preamble comes after the block in arm B, before the task.

`run --block FILE` (arm B only, this set only) gives the agents another block. The file holds the
block text, with or without the `<!-- treeyard:connect:start/end -->` marker lines. After
`treeyard connect` the runner writes it in place of the connect block in `AGENTS.md` and `CLAUDE.md`,
checks it there, and puts it before every task prompt, as it does with `block-b.md`. The run name and
the result file get the file stem (`B-click-colliding-r1-real-try1`), and the result records
`block_file` and `block_sha256`. Free checks first, then one real cell:

```sh
python3 bench/t2/t2.py run --scratch S --arm B --repo click --set colliding --rep 1 --harness oracle --block try1.md
python3 bench/t2/t2.py run --scratch S --arm B --repo click --set colliding --rep 1 --harness real --dry-run --block try1.md
python3 bench/t2/t2.py run --scratch S --arm B --repo click --set colliding --rep 1 --harness real --yes-real --block try1.md
```

Arms A and A2 take no `--block` (A2 has its own block, `A2_BLOCK` in t2.py):

```sh
python3 bench/t2/t2.py run --scratch S --arm A --repo click --set disjoint --rep 1 --harness real --yes-real
python3 bench/t2/t2.py run --scratch S --arm A2 --repo click --set colliding --rep 1 --harness real --dry-run
```

The `real` harness starts paid or quota-using agents (3 Claude Code, 2 Codex) and needs the owner's
yes and a budget.

## Mixed set (10 agents)

On `disjoint` and `colliding` (5 agents, tasks of about a minute, 1 to 3 changed lines) plain git
delivers every task ([shakedown/README.md](shakedown/README.md)), so those runs cannot show whether
coordination helps. `mixed` is harder and closer to feature work: 10 tasks for 10 agents at once, each a
small feature with edits in several places (sized for 5 to 10 minutes of agent work), with designed
interactions between them. It is defined in [`taskdefs_mixed.py`](taskdefs_mixed.py) (the old sets in
`taskdefs.py` are untouched) and generated to `tasks/click/mixed/` with its own manifest
`tasks/MANIFEST-mixed.json`; `tasks/MANIFEST.json` and the files of the old sets are generated byte for
byte as before. Same pin, same suite. Free checks: [`checks-mixed/`](checks-mixed/).

| k | Id | Change | Files of the reference | Diff lines | Interactions |
|---|---|---|---|---|---|
| 1 | `m1-help-multiple` | `Option(show_multiple=True)` puts `multiple` in the help bracket, before `required` | core.py, types.py | 11 | A; trap X (i1); `OptionHelpExtra` |
| 2 | `m2-prompt-enum-names` | prompts list enum choices and an enum default by member name | termui.py | 7 | B; same line (i2) |
| 3 | `m3-hidden-default` | `hide_input=True`: help and prompt show `(hidden)` for the default | core.py, termui.py | 17 | A; B; call line (i5, i6); `_build_prompt` parameters |
| 4 | `m4-style-link` | `style(link=URL)` writes an OSC 8 hyperlink; `strip_ansi` removes OSC sequences | _compat.py, termui.py | 7 | interface P (i4) |
| 5 | `m5-range-unit` | `IntRange`/`FloatRange(unit=...)`; the help item reads `range: 1<=x<=5 s` | types.py, core.py, tests/test_options.py | 19 | A; trap Y (i1); interface P (i3); test table |
| 6 | `m6-result-check` | `Result.check(exit_code=0)` and `CliRunner.invoke(check=True)` | testing.py | 21 | control |
| 7 | `m7-prompt-max-choices` | `prompt(max_choices=N)` lists the first N choices, then `...` | termui.py | 19 | B; same line (i2); call line (i6); `prompt` parameters |
| 8 | `m8-duration-type` | new type `click.Duration` (`1h30m` to `timedelta`), picked for `timedelta` defaults | types.py, \_\_init\_\_.py, docs/api.md | 42 | exports, docs index, `convert_type` |
| 9 | `m9-prompt-range` | prompts show the range of an `IntRange`/`FloatRange`; `prompt(show_range=False)` | termui.py | 17 | B; interface Q (i3); call line (i5); `prompt` parameters |
| 10 | `m10-heading-underline` | `HelpFormatter(heading_underline="-")` underlines headings by their visible width | formatting.py | 8 | interface Q (i4) |

Diff lines count `+` and `-` lines of `reference.patch`; agents also add a regression test (preamble).

- **A, conflict cluster in `Option.get_help_record` / `get_help_extra` (core.py):** m1, m3, m5 edit the
  same two functions; m1 and m5 conflict in git (neighbouring lines of `get_help_record`).
- **B, conflict cluster in `_build_prompt` / `prompt` (termui.py):** m2, m3, m7, m9; 5 of their 6 pairs
  conflict in git. In git 6 of the 45 pairs of reference patches conflict: 1-5, 2-7, 2-9, 3-7, 3-9, 7-9.
- **Undo trap (m1 X, m5 Y):** m5 says "each item in the bracket says what it is, in the form
  `label: value`; the only item without a label is `required`". Read alone it seems to forbid m1's bare
  `multiple`. Both hidden tests pass together; integration test i1 needs both.
- **Same line (m2, m7):** both must change the line `prompt += f" ({', '.join(map(str, type.choices))})"`;
  the resolution keeps m2's enum names and m7's cut (i2). Also m3, m7 and m9 each add an argument to the
  one `_build_prompt(...)` call in `prompt` and a parameter to its signature: all three must survive (i5, i6).
- **Interface dependencies:** m9 must show the range through `_describe_range()`, which m5 extends with
  the unit (i3: `Wait (1<=x<=5 s): `); m10 must measure the heading with `term_len`, whose `strip_ansi`
  m4 teaches OSC 8 links (i4). Each passes alone; the integration test passes only when the caller uses
  the shared helper.
- **Registry-like surfaces:** `OptionHelpExtra` (m1), the parameter lists of `prompt` (3 overloads) and
  `_build_prompt` (m3, m7, m9), `click/__init__.py` exports and the Types index of `docs/api.md` (m8),
  `convert_type` (m8), the parametrised table `test_intrange_default_help_text` (m5). Agents also append
  their regression tests to the same test files (`test_options.py`, `test_termui.py`).
- **Control:** m6 touches only `click/testing.py`.

None of the main sites is a site of the old sets (`Choice.normalize_choice`/`convert`, `_interpret_color`,
`get_app_dir`, PowerShell completion, `Argument` names, `Editor.get_editor`).

Scoring: a task is on main when every function of its hidden test file passes (3 to 6 per task).
`integration_passed` / `integration_total` counts the 6 hidden integration tests
(`tasks/click/mixed/integration/<id>/`), apart from `tasks_on_main`. The suite and the pinned suite as
for the other sets.

Reference edits apply one task after the other (`apply_seq`); an edit may carry `alts`, the same edit
written for a tree where another task is in (m7 and m9 on top of m2 and m3). `resolved-<k>.patch` (for
every k) is the step from tasks 1..k-1 to 1..k, so the oracle reaches 10/10 with 6/6 integration in all
three arms. The resolution of the same-line pair only inserts lines next to m2's, so arm B's `undoes`
check does not hold it. `validate` checks: each hidden test fails on the pin and passes with its
reference, and the suite stays green with that reference alone; all 10 resolved together pass the suite,
the 10 tests and the 6 integration tests; each integration test fails on the pin, with either partner
alone, and with all tasks but either partner (`integration/<id>/without-<k>.patch`).

### Set-level settings (`tasks/click/mixed/set.json`)

- Agents 10. Slots `codex, claude, claude, codex, claude, claude, codex, claude, codex, claude`: 6 Claude
  Code, 4 Codex, chosen so that the two tasks of every integration pair get different harnesses in
  repetition 1; later repetitions rotate by one slot as the other sets do.
- Limits per agent (the old sets keep the frozen `LIMITS` of 80 turns, 4 M tokens, 1800 s, 3 USD):
  `max_turns` 150, `max_tokens` 8 M, `timeout_s` 2400, `claude_budget_usd` 5.0. Reasons: in the 12 real
  runs of the old sets in `results/` an agent took 10 to 39 turns, 0.2 to 1.4 M tokens and 15 to 210 s,
  and a run cost about 0.5 to 0.65 USD for 3 Claude agents. These tasks need several times the reading
  and the edits, plus a regression test and suite runs that get slower with 10 agents testing at once,
  and Claude's tokens grow faster than its turns (each turn rereads the cached context). Expected: about
  40 to 100 turns, 1 to 5 M tokens, 5 to 15 minutes, about 1 USD per Claude agent. The caps sit above
  that and still stop a runaway agent; the 40 minutes also cover a hand-back (it resumes the session
  within the same limits) and the slow start of 10 CLIs at once. The worst case is 6 x 5 = 30 USD of
  Claude per run; Codex reports no cost.

### Load

- The box of an agent in a real run: a venv made with `uv venv` plus `uv pip install --offline` from the
  uv cache. 10 of them take 0.3 s in total and about 7 MB each as `du` counts it (uv clones files on
  APFS, so less on disk), so they are made one after the other and not shared (a shared venv could be
  written by one agent's `pip install` under another).
- Setup with 10 agents (`run --dry-run --harness real`, whole command, venvs included): A 1.4 to 1.9 s, A2 1.2
  to 1.5 s, B 3.2 to 3.3 s over two tries (`treeyard connect --agents 10`, 10 workspaces, 10 open changes checked). Rehearsal `setup_s`:
  A 0.9 s, A2 0.9 s, B 2.8 s.
- The A2 integrator answered 10 oracle agents in the fixed order (3 integrate conflicts, all settled);
  the arm B server took 10 workspaces, 3 land conflicts and 10 lands.

| Rehearsal (oracle, noop) | Tasks on main | Integration | Conflicts | Interventions | Suite all / pinned |
|---|---|---|---|---|---|
| A oracle | 10/10 | 6/6 | 3 (hand-backs) | 0 | green / green |
| A2 oracle | 10/10 | 6/6 | 0 (3 integrate conflicts) | 0 | green / green |
| B oracle | 10/10 | 6/6 | 3 (land) | 0 | green / green |
| A, A2, B noop | 0/10 | 0/6 | 0 | 0 | green / green |

### Commands

```sh
python3 bench/t2/t2.py generate --scratch S --repo click                  # old sets byte for byte, plus mixed
python3 bench/t2/t2.py validate --scratch S --repo click --set mixed --results D   # D/validate-mixed.txt
python3 bench/t2/t2.py rehearse --scratch S --repo click --set mixed --results D   # oracle and noop, A, A2, B
python3 bench/t2/t2.py run --scratch S --arm A --repo click --set mixed --rep 1 --harness real --dry-run
python3 bench/t2/t2.py run --scratch S --arm A --repo click --set mixed --rep 1 --harness real --yes-real
python3 bench/t2/t2.py run --scratch S --arm A2 --repo click --set mixed --rep 1 --harness real --yes-real
python3 bench/t2/t2.py run --scratch S --arm B --repo click --set mixed --rep 1 --harness real --yes-real
```

Without `--set`, `validate` and `rehearse` on click run all three sets; `dev/results/validate.txt` gets
only the old sets' lines, as before (point `--results` elsewhere: those files are frozen).

### Recipe: the same kind of set for another repository

1. Pin and suite: the repository's pinned commit and a suite that runs offline in seconds; note which
   test files are agent-editable (`test_globs`) and whether any test asserts the exact output of a
   function you plan to change (those tests decide what a task may change: here `term_len` and the
   help of `multiple` options were ruled out by existing tests).
2. Pick two central regions (a function or two that many features pass through, in one or two files)
   and the registry-like surfaces of the repository: export lists, a shared typed dict or options
   table, parameter lists of a central function, a docs index, a parametrised test table.
3. Write 10 small features of 2 to 6 edits each: 3 or 4 per central region, so that 4 to 8 pairs
   conflict in git; most also touch a registry surface; one control task in a far file.
4. Design on purpose and write down: one undo trap (X adds a narrow case; Y's general rule, read alone,
   seems to forbid it), one same-line pair, two interface dependencies where the caller passes alone but
   the integration test needs it to go through the helper the other task changes.
5. Hidden tests: 3 or more test functions per task, through public behaviour only, robust to other
   correct implementations (no exact source text, no private names unless the prompt names them).
   Integration tests: one scenario that needs both behaviours.
6. Reference edits per task (`edits`), with `alts` where a combined tree needs a different text; make a
   resolution insert next to the other task's lines rather than rewrite them (arm B holds a land that
   removes another agent's landed lines).
7. Slots: give the two tasks of each designed pair different harnesses in repetition 1.
8. Check, all free: `generate` (old files unchanged), `validate --set <name>` (every check `ok`), a
   look at the git pair table, `rehearse --set <name>` (oracle N/N with every integration test green in
   A, A2 and B; noop 0/N), `run --dry-run --harness real` per arm, `python3 -m unittest bench.t2.test_t2`.
9. Never run, print or copy anything of a blind repository's task set while designing another one.
