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
python3 bench/t2/t2.py rehearse --scratch S --repo click    # oracle and noop runs of the 4 cells
python3 bench/t2/t2.py collect --results bench/t2/dev/results --summary bench/t2/dev/results/rehearsal-summary.txt
```

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

Arm A has no block; its runs on this set take no `--block`:

```sh
python3 bench/t2/t2.py run --scratch S --arm A --repo click --set disjoint --rep 1 --harness real --yes-real
```

The `real` harness starts paid or quota-using agents (3 Claude Code, 2 Codex) and needs the owner's
yes and a budget.
