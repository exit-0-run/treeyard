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
- **Interface dependencies:** m9 shows the range, which m5 extends with the unit (i3:
  `Wait (1<=x<=5 s): `); m10 measures the heading, and m4 teaches click to strip OSC 8 links (i4). Each
  passes alone. Since 2026-11-22 both integration tests rest only on what the prompts state: m9 says the
  range text in the prompt is the text of the type's `repr` and error message (m5 puts the unit there),
  and m10 says that everything `click.unstyle` removes takes no columns (m4 makes `unstyle` remove
  links). Before, they passed only when the agent happened to call the shared helper (`_describe_range`,
  `term_len`). The real runs in `shakedown/mixed1/` used the older wording.
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

## Heavy set (10 agents, feature-sized tasks)

The real runs of `mixed` took 0.5 to 2 minutes per task and all three arms delivered 10/10
([shakedown/README.md](shakedown/README.md)). `heavy` has tasks sized for 5 to 10 minutes of a strong
agent: a feature or a refactor with a new helper or class, several call sites and a docs entry, whose
long-lived work meets other tasks on the same code. Defined in [`taskdefs_heavy.py`](taskdefs_heavy.py)
with its data in [`heavy/`](heavy/): the reference solutions as patches (`patches/<k>-<id>.alone.patch`
on the pin, `<k>-<id>.step.patch` on tasks 1..k-1, `without-<k>.patch`), the hidden tests (`tests/`) and
the integration tests (`integration/`). Generated to `tasks/click/heavy/` with `tasks/MANIFEST-heavy.json`;
the other sets are generated byte for byte as before. Same pin, same suite. Free checks:
[`checks-heavy/`](checks-heavy/).

| k | Id | Change | Files of the reference | Lines (src / docs) | Interactions |
|---|---|---|---|---|---|
| 1 | `h1-prompt-text` | refactor: the prompt text becomes class `PromptText` in new `click/_prompt.py`; `_build_prompt` stays as a thin alias | _prompt.py (new), termui.py, \_\_init\_\_.py, api.md, prompts.md | 169 / 20 | R1; P; trap Y (i1); i7 |
| 2 | `h2-help-items` | refactor: `Option.get_help_extra` split into `get_help_extra_<key>` methods, `help_extra_keys`, `format_help_extra(_item)`, `formatting.append_help_extra` | core.py, formatting.py, types.py, documentation.md | 252 / 30 | R2; H; F; trap Y (i2); i9 |
| 3 | `h3-secrets` | `click.Secret` / `SecretStr`; hidden-input defaults show `(hidden)` in help, prompt and `to_info_dict`; errors mask the value | types.py, core.py, termui.py, \_\_init\_\_.py, api.md, parameter-types.md, option-decorators.md | 118 / 33 | H; P; R; trap X (i2); i4 |
| 4 | `h4-nushell` | Nushell completion `NuComplete` (`nu_source`, JSON records, `null` for paths, version check) | shell_completion.py, shell-completion.md, examples/completion/README | 111 / 51 | control |
| 5 | `h5-prompt-attempts` | `prompt`/`confirm(max_attempts=)`, `click.TooManyAttempts`, `Option(prompt_attempts=)` | termui.py, exceptions.py, core.py, \_\_init\_\_.py, api.md, exceptions.md, prompts.md | 104 / 50 | P; H (init); R |
| 6 | `h6-option-groups` | `Option(help_group=)`, `Command(help_group=, help_group_order=)`, `get_help_groups`, sections lined up via `write_dl(first_col_width=)` | core.py, formatting.py, documentation.md | 114 / 39 | H (init); F; interface (i3, i6) |
| 7 | `h7-byte-size` | `click.ByteSize` (units, bounds, human sizes) and `ParamType.format_value` used by the help default and the prompt default | types.py, core.py, termui.py, \_\_init\_\_.py, api.md, parameter-types.md | 143 / 32 | H; P; R; i5, i7 |
| 8 | `h8-choice-menu` | enum choices and defaults by name in prompts (`Choice.format_choice`); `prompt(choice_menu=)` numbered menu, `Option(prompt_menu=)` | termui.py, types.py, core.py, prompts.md | 118 / 30 | P; H (init); trap X (i1) |
| 9 | `h9-help-theme` | `click.HelpTheme`; `HelpFormatter(theme=)` styles headings, terms, usage prefix; the option bracket; `Context(help_theme=)` | formatting.py, core.py, \_\_init\_\_.py, api.md, documentation.md | 123 / 29 | F; H (`get_help_record`); R; i3, i8 |
| 10 | `h10-help-markdown` | `Command.get_help_markdown`, `click.MarkdownHelpFormatter` (tables with a Details column from the help bracket) | formatting.py, core.py, \_\_init\_\_.py, api.md, documentation.md | 123 / 41 | F; R; interface (i4, i5, i6, i8, i9) |

Lines count `+` and `-` lines of `reference.patch` (no test files in it): under `src/` and the rest
(docs, the example README). Counted with the `awk` line in [`checks-heavy/README.md`](checks-heavy/README.md).
By that count all 10 have 148 to 282 changed lines, but only h1 and h2 have 150 or more under `src/`
(the others 104 to 143); agents also write regression tests (preamble), which the count leaves out.

- **H, `Option` help in core.py** (`__init__`, `get_help_extra`, `get_help_record`, `prompt_for_value`,
  `to_info_dict`): h2, h3, h5, h6, h7, h8, h9 (7 tasks).
- **P, the prompt text in termui.py** (`_build_prompt`, `_format_default`, `prompt` with its 3 overloads
  and docstring, `confirm`): h1, h3, h5, h7, h8 (5 tasks).
- **R, registries**: `click/__init__.py` exports and `docs/api.md`: h1, h3, h5, h7, h9, h10 (6 tasks);
  also the shared docs pages `docs/prompts.md` (h1, h5, h8), `docs/documentation.md` (h2, h6, h9, h10),
  `docs/parameter-types.md` (h3, h7).
- **F, `HelpFormatter` and the rest of formatting.py**: h2, h6, h9, h10 (4 tasks).
- **Refactors.** R1 = h1 moves `_build_prompt` and `_format_default` into `PromptText` in a new module;
  h3 (hidden default), h7 (formatted default) and h8 (enum names, menu) edit exactly that code, h5 the
  prompt loop and `confirm` around it. R2 = h2 splits `get_help_extra`/`get_help_record`; h3 and h7 edit
  the default item, h9 styles the bracket in `get_help_record`. Behaviour is unchanged, so every other
  hidden test passes on the combined tree.
- **Undo traps.** h1 says "for every input, `click.prompt` and `click.confirm` must show exactly the text
  they show today"; h8 (X) changes the text of enum choices on purpose. h2 says "every help page and every
  `get_help_extra` result stays exactly as it is today"; h3 (X) hides defaults. Read alone, the refactor's
  rule invites dropping the other task's change when the two meet (i1, i2).
- **Interface dependencies.** h10 shows the items of the help bracket that h2 (custom items, i9), h3
  (`(hidden)`, i4), h7 (sizes, i5) and h9 (styled bracket, i8) change; h6 writes its group sections with
  `formatter.section`, whose headings h9 styles (i3) and h10 turns into Markdown sections (i6).
- **Control:** h4 touches no file of another task.

In git 13 of the 45 pairs of reference patches conflict: 1-3, 1-5, 1-7, 1-8, 2-3, 2-7, 2-9, 2-10, 3-5, 3-8,
5-8, 7-8, 9-10 (mixed: 6).

### Integration tests

Hidden, run on the final main (`tasks/click/heavy/integration/<id>/`), counted as
`integration_passed`/`integration_total` apart from `tasks_on_main`. Each rests only on behaviour that the
prompts of both tasks state:

| Id | Tasks | Checks | Rests on |
|---|---|---|---|
| `i1-prompt-text-enum-names` | 1, 8 | `PromptText(..., Choice(Color)).render()` shows `(RED, GREEN) [GREEN]` and equals the prompt | h1: `render()` is what `prompt` shows, for every input; h8: prompts show enum names |
| `i2-hidden-default-item` | 2, 3 | `get_help_extra_default(ctx)` of a hidden input (and a `Secret`) is `(hidden)` | h2: the method returns the item's value; h3: the item is `(hidden)` |
| `i3-group-headings-themed` | 6, 9 | a group section heading gets the theme's heading style | h6: group sections go through `formatter.section`; h9: `write_heading` styles every heading |
| `i4-markdown-hidden-default` | 3, 10 | Markdown details show `default: (hidden)`, never the value | h10: details are the `--help` bracket items; h3: the bracket shows `(hidden)` |
| `i5-markdown-byte-size` | 7, 10 | details show `default: 10 MiB; x<=1 GiB` | h10: as i4; h7: the bracket shows sizes |
| `i6-markdown-option-groups` | 6, 10 | a `## Network` table holds the grouped option, after `## Options` | h10: every section of `--help`, through `format_help`; h6: group sections |
| `i7-prompt-text-byte-size` | 1, 7 | `PromptText(..., ByteSize()).render()` and `default_part()` show `10 MiB` | h1: `render()` is what `prompt` shows; h7: the prompt default uses `format_value` |
| `i8-markdown-with-a-theme` | 9, 10 | with a theme on the context the Markdown has no escape code | h10: style codes are removed everywhere; h9: the theme styles the bracket |
| `i9-markdown-custom-item` | 2, 10 | an item added with `help_extra_keys` shows in the details | h2: subclass items show in the bracket; h10: details are the bracket items |

`validate` checks each integration test fails on the pin, with either partner alone and with all tasks
but either partner, and (new, for every extra set) that on the tree without one partner the hidden test
of the other partner passes, so the "all but" trees are real trees.

### How the reference applies

`resolved-<k>.patch` is the step from tasks 1..k-1 to 1..k (the `combined_edits`); it is what the oracle
writes on a hand-back or a land conflict. Steps 1, 2, 4 and 6 merge cleanly in git and give the same tree
as the step; steps 3, 5, 7, 8, 9, 10 conflict, so the oracle resolves them with the step. Arm B holds a
land that removes another agent's landed words (`undoes`), so the steps keep earlier tasks' words on one
or two adjacent lines (an extra `versionchanged` block instead of a rewritten one, the theme branch next
to h2's unchanged `return` in `append_help_extra`).

The `without-<k>.patch` trees: the kit first applies the other tasks' `combined_edits`; where they need
task k it merges each step in with `git merge-file` (falling back to the task alone, then `--union`)
(`t2.replay_without`). That gave broken trees without h1, h3 and h7, which the new sanity check caught;
those three are written by hand (`heavy/patches/without-{1,3,7}.patch`, `WITHOUT` in the taskdefs).

### Set-level settings (`tasks/click/heavy/set.json`)

- Agents 10. Slots `codex, claude, codex, claude, codex, claude, claude, claude, claude, codex` (Codex: h1,
  h3, h5, h10). The pairs form two triangles (2-3-10 and 6-9-10), so no split gives every pair two
  harnesses; this one separates 7 of the 9 (not 3-10 and 6-9).
- Limits per agent: `max_turns` 300, `max_tokens` 20 M, `timeout_s` 3600, `claude_budget_usd` 8.0. Reasons:
  the tasks are 5 to 10 times the work of a `mixed` task (whose real agents used 23 to 123 s), with a new
  module or class, docs, a regression test and suite runs shared by 10 agents, and refactors that turn into
  long hand-backs (a hand-back resumes the session within the same limits). Expected: 10 to 25 minutes per
  agent including a hand-back, 3 to 12 M tokens for Claude. The hour stops a runaway agent. Worst case
  6 x 8 = 48 USD of Claude per run; Codex reports no cost.

| Rehearsal (oracle, noop) | Tasks on main | Integration | Conflicts | Interventions | Suite all / pinned |
|---|---|---|---|---|---|
| A oracle | 10/10 | 9/9 | 6 (hand-backs) | 0 | green / green |
| A2 oracle | 10/10 | 9/9 | 0 | 0 | green / green |
| B oracle | 10/10 | 9/9 | 6 (land) | 0 | green / green |
| A, A2, B noop | 0/10 | 0/9 | 0 | 0 | green / green |

The oracle runs report 11 lost lines in every arm: lines of a task's own reference that the steps of the
refactors rewrite (h3's lines in `_build_prompt` live in `PromptText` on main); the scoring is by hidden
tests, not by lines.

### Commands

```sh
python3 bench/t2/t2.py generate --scratch S --repo click
python3 bench/t2/t2.py validate --scratch S --repo click --set heavy --results D    # D/validate-heavy.txt
T2_TREEYARD=BIN python3 bench/t2/t2.py rehearse --scratch S --repo click --set heavy --results D
python3 bench/t2/t2.py run --scratch S --arm A --repo click --set heavy --rep 1 --harness real --dry-run
python3 bench/t2/t2.py run --scratch S --arm A --repo click --set heavy --rep 1 --harness real --yes-real
python3 bench/t2/t2.py run --scratch S --arm A2 --repo click --set heavy --rep 1 --harness real --yes-real
T2_TREEYARD=BIN python3 bench/t2/t2.py run --scratch S --arm B --repo click --set heavy --rep 1 --harness real --yes-real
```

To change a reference: edit the code in a click clone with one branch per task from the pin and one
branch of steps, then write `git diff -U5 <pin> <task>` to `<k>-<id>.alone.patch` and
`git diff -U5 <step k-1> <step k>` to `<k>-<id>.step.patch` (none when equal to the alone patch), and run
`generate` and `validate`.

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
