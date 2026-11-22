"""The `hard` development set: 13 tasks for 13 Claude Code agents on click (dev/README.md, "Hard set").

The nine feature and refactor tasks of `heavy` without its control h4 (same ids, prompts, reference patches and
hidden tests; the h7 prompt states the range rule on its own line), plus four refactors that move code which
three feature tasks each edit, and keep an old name that nothing in the suite calls:

- h11 moves the loops of `click.prompt` and `click.confirm` into `Prompter` (`click/_prompter.py`); h3, h5 and h8
  edit those loops. Kept: `termui._readline_prompt`.
- h12 moves `Option.prompt_for_value` into `OptionPrompt` (`click/_option_prompt.py`); h5 and h8 add their
  prompt arguments there, h3 hides the input of a `Secret`. Kept: `Option.prompt_for_value`.
- h13 moves `HelpFormatter` and its helpers into `click/_help_formatter.py`; h6 and h9 edit the class, h10
  subclasses it. Kept: the names in `click.formatting` (`measure_table`, `iter_rows`, `HelpFormatter`, `wrap_text`).
- h14 moves the text of a default (help bracket and prompt) into `click/_default_text.py`; h3, h7 and h8 change
  that text. Kept: `termui._format_default`.

The refactors come last in the reference order, so their steps (`combined_edits`) move the features' code into
the new structures, keeping the words of every line another task wrote (arm B holds a land that removes them).
The data is in `hard/`: `patches/<k>-<id>.alone.patch` (the task on the pin), `<k>-<id>.step.patch` (the task on
tasks 1..k-1, where it differs), `without-<k>.patch` (trees of all tasks but k, written by hand), `tests/`,
`integration/`.
"""

from pathlib import Path

HERE = Path(__file__).resolve().parent / "hard"

SUFFIX = "\n\nKeep the existing behaviour otherwise. Finish the work completely.\n"


def ops_from_patch(text):
    """The edit ops of a `git diff` text: `replace` (hunk context and old lines -> context and new lines)
    for a changed file, `create` for a new one."""
    ops, path, new_file, old, new = [], None, False, None, None

    def flush():
        if old is None:
            return
        if new_file:
            ops.append({"op": "create", "file": path, "text": "".join(new)})
        else:
            ops.append({"op": "replace", "file": path, "old": "".join(old), "new": "".join(new)})

    for line in text.splitlines(True):
        if line.startswith("diff --git "):
            flush()
            old = new = None
            path, new_file = line.split(" b/", 1)[1].strip(), False
        elif line.startswith("new file mode"):
            new_file = True
        elif line.startswith(("--- ", "+++ ", "index ", "similarity", "rename ")) and old is None:
            continue
        elif line.startswith("@@"):
            if not new_file:
                flush()
                old, new = [], []
            elif old is None:
                old, new = [], []
        elif old is not None and line.startswith("\\"):
            continue
        elif old is not None:
            tag, body = line[:1], line[1:]
            if tag in (" ", "-"):
                old.append(body)
            if tag in (" ", "+"):
                new.append(body)
    flush()
    return ops


def patch(k, tid, kind):
    f = HERE / "patches" / f"{k}-{tid}.{kind}.patch"
    return f.read_text() if f.is_file() else None


PROMPTS = {
    "h1-prompt-text": (
        "Refactor: the prompt text",
        "The text that `click.prompt` and `click.confirm` show before they read an answer is built by the "
        "private function `_build_prompt` in `src/click/termui.py`, from strings glued together in three "
        "places. Move it into a class `PromptText` in a new module `src/click/_prompt.py`, export it as "
        "`click.PromptText` (it is also importable as `click.termui.PromptText`), document it with an "
        "`autoclass` entry (with its members) in `docs/api.md` right after `confirm`, and add a section to "
        "`docs/prompts.md` that explains it.\n\n"
        "- `PromptText(text, suffix=\": \", show_default=False, default=None, show_choices=True, type=None)`: the "
        "arguments in this order (positional or keyword), the same meaning as those of `_build_prompt` today.\n"
        "- `choices_part()` returns the choices of a `Choice` type in parentheses with a leading space "
        "(`\" (s, m)\"`), or `\"\"` when there is no `Choice` type or `show_choices` is false.\n"
        "- `default_part()` returns the default in brackets with a leading space (`\" [m]\"`; a `show_default` "
        "string shows as `\" [(that string)]\"`), or `\"\"` when nothing is shown.\n"
        "- `render()` returns the text, then `choices_part()`, then `default_part()`, then the suffix. It always "
        "calls these two methods, so a subclass that overrides one of them (or code that replaces it on the class) "
        "changes the result. `str()` of the object is `render()`.\n"
        "- The class method `PromptText.for_confirm(text, default=False, suffix=\": \", show_default=True)` makes the "
        "text of a yes/no question: `PromptText.for_confirm(\"Go?\").render()` is `\"Go? [y/N]: \"`, with "
        "`default=True` it shows `[Y/n]`, with `None` `[y/n]`.\n\n"
        "`click.prompt` (its prompt and its confirmation prompt) and `click.confirm` must build their text only "
        "through `PromptText`. Keep `_build_prompt` in `termui.py`, with its signature, as a thin function that "
        "returns `PromptText(...).render()`, for code that calls it; the helper `_format_default` moves to the new "
        "module with the class. This is a pure refactor: for every input, `click.prompt` and `click.confirm` must "
        "show exactly the text they show today.",
    ),
    "h2-help-items": (
        "Refactor: one method per item of the option help bracket",
        "The bracket at the end of an option's help line (`[env var: N; default: 2; 1<=x<=5; required]`) is "
        "built by `Option.get_help_extra` in `src/click/core.py`, one long method that computes all items, and "
        "`Option.get_help_record`, which formats them with a chain of `if`s. Nobody can add an item or change one "
        "without copying both. Split them:\n\n"
        "- A class attribute `Option.help_extra_keys = (\"envvars\", \"default\", \"range\", \"required\")`: the keys "
        "of the bracket, in the order they are shown.\n"
        "- For each key a method `get_help_extra_<key>(ctx)` that returns its value, or `None` to leave the item "
        "out: `get_help_extra_envvars` (a tuple of names), `get_help_extra_default`, `get_help_extra_range`, "
        "`get_help_extra_required` (strings), with today's rules for when each item appears.\n"
        "- `get_help_extra(ctx)` returns `{key: value}` for each key of `help_extra_keys` whose method does not "
        "return `None`, in that order.\n"
        "- `format_help_extra(ctx, extra)` returns the texts of the items of the dict `extra`, in the order of "
        "`help_extra_keys` (keys that are not in `help_extra_keys` are left out); "
        "`format_help_extra_item(key, value)` is the text of one item: `env var: A, B`, `default: 2`, the bare "
        "range, `required`, and for any other key `<key>: <value>` (a tuple or list value joined with `, `).\n"
        "- A function `append_help_extra(help, items)` in `src/click/formatting.py` appends the bracket to a help "
        "text: `append_help_extra(\"Name.\", [\"default: x\", \"required\"])` is `\"Name.  [default: x; required]\"`; "
        "with an empty help text the bracket stands alone (`\"[required]\"`), without items the help text is "
        "returned as it is. `get_help_record` uses `get_help_extra`, `format_help_extra` and this function.\n\n"
        "So a subclass adds an item by extending `help_extra_keys` and writing its method "
        "(`help_extra_keys = (*click.Option.help_extra_keys, \"unit\")` with `get_help_extra_unit` returning `\"s\"` "
        "shows `[default: 30; unit: s]`), and drops one by returning `None`. Document the methods, mention the "
        "keys in the docstring of the `OptionHelpExtra` typed dict in `src/click/types.py`, and add a section "
        "\"The Option Help Bracket\" with such an example to `docs/documentation.md`. This is a refactor: every "
        "help page and every `get_help_extra` result stays exactly as it is today.",
    ),
    "h3-secrets": (
        "Secrets: a secret type and hidden defaults",
        "Options with `hide_input=True` (passwords, tokens) leak their value in several places: `--help` with "
        "`show_default=True` shows `[default: hunter2]`, the prompt shows `Password [hunter2]: `, "
        "`Option.to_info_dict()` reports the default, and an error such as `'abc9' is not a valid integer.` "
        "repeats the value given on the command line. Fix all of them and add a type for secret values.\n\n"
        "- `click.SecretStr(value)` (in `src/click/types.py`, exported from `click`): holds a string that does not "
        "show itself. `str()` and formatting give `***`, `repr()` gives `SecretStr('***')`, `reveal()` returns the "
        "value. Two secrets are equal (and hash alike) when their values are; `len()` and truth are those of the "
        "value.\n"
        "- `click.Secret()` (exported too): a parameter type named `\"secret\"` with the metavar `SECRET`; it converts "
        "text to a `SecretStr` and passes a `SecretStr` through unchanged. An `Option` of this type always has "
        "`hide_input` set to `True`, and `click.prompt(..., type=click.Secret())` hides the input as with "
        "`hide_input=True`.\n"
        "- The default of an option with `hide_input` (or a `Secret` type) shows as `(hidden)`: "
        "`get_help_extra(ctx)` gives `{\"default\": \"(hidden)\"}`, the help shows `[default: (hidden)]`, and "
        "`to_info_dict()[\"default\"]` is `\"(hidden)\"` (it stays `None` without a default). A `show_default` string "
        "is not the value and is shown as before (`[default: (from the keyring)]`).\n"
        "- `click.prompt(\"Password\", default=\"hunter2\", hide_input=True)` shows `Password [(hidden)]: `, and so does "
        "an option with `prompt=True, hide_input=True` or a `Secret` type.\n"
        "- An error about a value given for a hidden-input option, from the command line or an environment "
        "variable, shows `'***'` in place of the value: `Error: Invalid value for '--pin': '***' is not a valid "
        "integer.`\n\n"
        "Defaults and errors of inputs that are not hidden show as before. Document both classes with `autoclass` "
        "entries in the Types section of `docs/api.md` (after `Tuple`) and in the list of types in "
        "`docs/parameter-types.md`, the `hide_input` parameters of `Option` and `click.prompt`, and add a section "
        "about secrets to `docs/option-decorators.md`.",
    ),
    "h5-prompt-attempts": (
        "Prompts: give up after a number of invalid answers",
        "`click.prompt` and `click.confirm` ask again forever when the answer is invalid, which hangs scripts that "
        "pipe input in. Add a keyword parameter `max_attempts: int | None = None` to `click.prompt` (all its "
        "overloads) and to `click.confirm`, documented in both docstrings. With it, after that many invalid "
        "answers the prompt raises a new exception `click.TooManyAttempts` instead of asking again.\n\n"
        "- An invalid answer is a value the conversion rejects, a confirmation that does not match "
        "(`confirmation_prompt`), or for `confirm` anything but yes/no. An empty answer re-asked because there is "
        "no default does not count.\n"
        "- Each invalid answer still prints its error line. Before the last allowed one the line ends with how "
        "many attempts are left: `Error: 'many' is not a valid integer. (2 attempts left)`, "
        "`(1 attempt left)`; the error line of the last one has no such ending. Without `max_attempts` the output "
        "stays as it is. `max_attempts` below 1 raises `ValueError` at once, before anything is asked.\n"
        "- `click.TooManyAttempts(attempts, last_error=None)` in `src/click/exceptions.py` (exported from "
        "`click`, an `autoexception` entry in `docs/api.md` after `BadArgumentUsage`, and listed in "
        "`docs/exceptions.md`) is a `ClickException` with exit code 1 and the message "
        "`Gave up after 3 invalid answers.` (`Gave up after 1 invalid answer.` for one). Its attributes are "
        "`attempts` and `last_error`, the error message of the last invalid answer (`'z' is not a valid "
        "integer.`). So inside a command it ends the program with `Error: Gave up after 3 invalid answers.` "
        "and exit code 1.\n"
        "- `click.Option` takes `prompt_attempts: int | None = None` (documented) and passes it as `max_attempts` "
        "when it prompts, for normal options and for boolean flags that use `confirm`. `prompt_attempts` without "
        "`prompt` raises `TypeError`.\n\n"
        "Add a section \"Limiting Attempts\" to `docs/prompts.md` with an example session.",
    ),
    "h6-option-groups": (
        "Help pages: option groups",
        "Commands with many options get a help page that is one long list. Let options be listed in groups.\n\n"
        "- `click.Option` takes `help_group: str | None = None` (an attribute of the same name, documented): the "
        "name of a section of the help page, such as `\"Network\"`.\n"
        "- `Command.format_options` lists the options without a group in the `Options` section as today (the "
        "`--help` option among them), then one section per group, headed by the group name exactly as given "
        "(`Network:`). Each group section is written the same way as `Options`: `formatter.section(name)` and "
        "`formatter.write_dl(rows)`, so a formatter that changes how headings or lists look applies to it too. "
        "Hidden options are left out, and a section without any shown option is not written (not even "
        "`Options`).\n"
        "- The order of the groups: first those named in a new parameter `help_group_order` of `click.Command` "
        "(a sequence of names, stored as the tuple attribute `help_group_order`, empty by default), then the "
        "others in the order their first option appears.\n"
        "- The second column starts at the same place in all option sections: the first column is as wide as "
        "the widest option over all these sections (capped as today). For this `HelpFormatter.write_dl` takes a "
        "new keyword `first_col_width: int | None = None`, used in place of the width of the widest term of the "
        "rows, still capped at `col_max`.\n"
        "- A new method `Command.get_help_groups(ctx)` returns the rows: a dict from group name (`None` for the "
        "options without a group, always first when present) to the list of `(spec, help)` records of its shown "
        "options, in the section order. Arguments are not in it.\n"
        "- The commands of a `click.Group` can be grouped the same way: `click.Command` takes "
        "`help_group: str | None = None` too, and the group's help lists the commands without a group under "
        "`Commands` as today, then one section per command group, ordered by the group's `help_group_order` and "
        "then by first appearance, with the second column lined up across these sections.\n\n"
        "A help page without groups stays as it is. Add a section \"Option Groups\" to `docs/documentation.md`.",
    ),
    "h7-byte-size": (
        "A byte size parameter type",
        "Add a parameter type `click.ByteSize` to `src/click/types.py` (exported from `click`, an `autoclass` "
        "entry in the Types section of `docs/api.md` after `FloatRange`, in the list of types of "
        "`docs/parameter-types.md`, and a section \"Byte Sizes\" there with an example). It converts a size to "
        "a number of bytes (an `int`), and it is a range type like `IntRange`.\n\n"
        "- Accepted: a whole number of bytes (`\"512\"`, or an `int`), or a number (fractions allowed) followed by "
        "a unit, with or without a space, in any letter case, with spaces around ignored: `B`; `K`/`KB`, "
        "`M`/`MB`, `G`/`GB`, `T`/`TB` (powers of 1000); `KiB`, `MiB`, `GiB`, `TiB` (powers of 1024). A fraction "
        "of a byte is rounded down: `\"1.5KiB\"` is 1536, `\"1.9B\"` is 1. Anything else (negative numbers "
        "included) fails with `'<value>' is not a valid size.` (the value as given, in `repr` form).\n"
        "- `ByteSize(min=None, max=None, min_open=False, max_open=False, clamp=False)` bounds the size as "
        "`IntRange` does; `min` and `max` may be sizes (`max=\"1GiB\"`) and are stored as bytes. The name of the "
        "type is `\"size\"` (so the metavar is `SIZE`).\n"
        "- Sizes are shown in binary units: the largest of TiB, GiB, MiB, KiB that the size reaches, with at most "
        "one decimal and no `.0` (`1.5 KiB`, `10 MiB`, `1 GiB`), else bytes (`512 B`). The range description "
        "uses that form: `repr(click.ByteSize(min=1024, max=\"10MiB\"))` is `<ByteSize 1 KiB<=x<=10 MiB>`, the help "
        "bracket shows `x<=1 GiB`, the error `... is not in the range x<=1 GiB.`\n"
        "- A `ByteSize` without `min` and without `max` has no range item in the help bracket at all: not "
        "`x>=0`, although negative sizes are rejected, and not any other range text. For example "
        "`click.Option([\"--any\"], type=click.ByteSize()).get_help_extra(ctx)` is `{}`, and its help line has "
        "no bracket.\n"
        "- Help pages and prompts show values through a new method `ParamType.format_value(value) -> str`: the text "
        "for a value of the type, `str(value)` by default. `ByteSize.format_value` shows an `int` as a size "
        "(`10485760` as `10 MiB`) and anything else with `str()`. The `default` item of an option's help uses it "
        "for the default (each element for a list or tuple default, joined with `, `), and `click.prompt` uses it "
        "for the default it shows when `type` is a `ParamType`: `click.prompt(\"Size\", type=click.ByteSize(), "
        "default=10485760)` shows `Size [10 MiB]: `.\n\n"
        "An option `--cache` with `type=click.ByteSize(max=\"1GiB\"), default=10 * 1024**2, show_default=True` "
        "shows `[default: 10 MiB; x<=1 GiB]`. Document `format_value` in the section on custom types too.",
    ),
    "h8-choice-menu": (
        "Choice prompts: enum names and a numbered menu",
        "Two problems with prompts for a `click.Choice`. A choice built from an enum accepts the member names "
        "(`RED`), but `click.prompt` lists the choices with `str()` (`(Color.RED, Color.GREEN)`) and shows an "
        "enum default as `[Color.GREEN]`. And a long list of choices is hard to type.\n\n"
        "- In prompts, an enum member is shown by its name, in the list of choices and as the default: "
        "`click.prompt(\"Color\", type=click.Choice(Color), default=Color.GREEN)` shows "
        "`Color (RED, GREEN, BLUE) [GREEN]: `. Other choices and defaults show as before. For this "
        "`Choice` gets a method `format_choice(choice) -> str`: the name of an enum member, `str()` of anything "
        "else.\n"
        "- `click.prompt` takes a keyword parameter `choice_menu: bool = False` (all overloads, documented). "
        "With a `Choice` type it first prints a numbered menu, one line per choice, `  1) eu`, `  2) us` (two "
        "spaces, the number, `) `, `format_choice` of the choice), once, before the first prompt line. The prompt "
        "then shows the range of numbers in place of the list of choices, `Region (1-3): `, and a default that "
        "is one of the choices as its number: `Region (1-3) [2]: `.\n"
        "- The answer may be the number of a choice or a choice itself. An answer that is itself a valid choice "
        "is that choice, so choices that are numbers keep working (`[\"10\", \"20\"]`: `20` is `\"20\"`, `2` is "
        "also `\"20\"`); any other number is an error `Error: 9 is not a number of the menu (1-3).` and the "
        "prompt asks again. A `Choice` method `choice_from_number(answer)` returns the choice for a menu "
        "number (`\"1\"` is the first), or `None`. With other types `choice_menu` changes nothing.\n"
        "- `click.Option` takes `prompt_menu: bool = False` (documented) and prompts with the menu; with a type "
        "that is not a `Choice` it raises `TypeError`.\n\n"
        "Add a section \"Choice Menus\" to `docs/prompts.md`.",
    ),
    "h9-help-theme": (
        "Help pages: themes",
        "Let applications style their help pages. Add a class `click.HelpTheme` to `src/click/formatting.py` "
        "(exported from `click`, an `autoclass` entry in the Formatting section of `docs/api.md` after "
        "`HelpFormatter`, and a section \"Help Themes\" at the end of `docs/documentation.md`).\n\n"
        "- `HelpTheme(heading=None, option=None, usage=None, extra=None)`: each argument is a dict of keyword "
        "arguments for `click.style` (`{\"fg\": \"cyan\", \"bold\": True}`) or `None` for no style, stored as an "
        "attribute of the same name. Two themes with the same styles are equal. `apply(part, text)` returns "
        "`text` styled with the style of the part named `part` (`\"heading\"`, ...), or `text` unchanged when that "
        "part has no style.\n"
        "- `click.HelpFormatter` takes `theme: HelpTheme | None = None`, kept as the attribute `theme` (an empty "
        "`HelpTheme()` when none is given). `write_heading` styles the heading with its colon "
        "(`style(\"Options:\", bold=True)`), `write_dl` styles the terms of the first column with the `option` "
        "style (the term only, not the padding), and `write_usage` styles the default `Usage:` prefix (not the "
        "space after it) with the `usage` style. Styles take no columns: once unstyled, the output is exactly "
        "the output without a theme.\n"
        "- The bracket at the end of an option's help line, brackets included (`[default: 1]`), gets the `extra` "
        "style.\n"
        "- `click.Context` takes `help_theme: HelpTheme | None = None` (so also through `context_settings`), "
        "inherited from the parent context when not given, kept as the attribute `help_theme`. "
        "`Context.make_formatter` passes it to the formatter as `theme`, but only when it is set, so formatter "
        "classes that do not take a `theme` keep working. Subcommands and the command list of a group are "
        "styled the same way.\n\n"
        "As with all styles, the codes are removed when the help is not written to a terminal unless colors "
        "are forced (`CliRunner.invoke(..., color=True)`).",
    ),
    "h10-help-markdown": (
        "Help pages as Markdown",
        "Projects copy their `--help` output into their docs by hand. Add a method "
        "`Command.get_help_markdown(ctx) -> str` and a formatter class `click.MarkdownHelpFormatter` (a subclass "
        "of `HelpFormatter` in `src/click/formatting.py`, exported from `click`, an `autoclass` entry in the "
        "Formatting section of `docs/api.md` after `HelpFormatter`, and a section \"Help Pages as Markdown\" at the "
        "end of `docs/documentation.md` with an example).\n\n"
        "`get_help_markdown` writes a title line `# <command path>` (`# cli run`), then lets the command format "
        "its help as usual (`format_help`) with a `MarkdownHelpFormatter`. So the Markdown has every section of "
        "`--help`, in the same order, including sections that a command writes in its own `format_help` or "
        "`format_epilog`. The formatter writes:\n\n"
        "- the usage line as ``Usage: `cli run [OPTIONS] [TARGET]` `` (the prefix, a space, the program and its "
        "arguments in backticks);\n"
        "- each paragraph of a text (the help of the command, the epilog) as a paragraph, lines stripped, not "
        "wrapped;\n"
        "- a section heading as `## Options`;\n"
        "- a definition list (options, arguments, commands, any `write_dl`) as a table with the header "
        "`| Name | Description | Details |` and the line `| --- | --- | --- |`, then one row per entry: the term "
        "in backticks, the description, and the details: the items of the bracket at the end of the help line, "
        "as `--help` shows them between `[` and `]` (`default: 1; required`), empty when there is none. An empty "
        "cell is a single space between the bars (`| `--quiet` |  |  |`).\n\n"
        "Blocks (title, usage, paragraphs, headings, tables) are separated by one blank line and the result ends "
        "with a newline. Style codes are removed everywhere (as `click.unstyle` does), whitespace inside a cell "
        "is collapsed to single spaces, and `|` in a cell is escaped as `\\|`. `MarkdownHelpFormatter` also has "
        "`write_title(title)` (the `# title` line) and `split_help(text)`, which returns the description and "
        "the details of a help line: `split_help(\"Text.  [a; b]\")` is `(\"Text.\", \"a; b\")`, "
        "`split_help(\"Text.\")` is `(\"Text.\", \"\")`.",
    ),
    "h11-prompter": (
        "Refactor: the prompt loop",
        "`click.prompt` and `click.confirm` in `src/click/termui.py` each build their text and then run a loop: "
        "read an answer, convert it, write an error line and ask again. The loops sit inside the two functions, so "
        "nothing can change one step without copying them. Move them into a class `Prompter` in a new module "
        "`src/click/_prompter.py`, export it as `click.Prompter` (it is also importable as `click.termui.Prompter`), "
        "document it with an `autoclass` entry (with its members) in `docs/api.md` right after `confirm`, and add a "
        "section \"How a Prompt Asks\" to `docs/prompts.md` with an example subclass.\n\n"
        "- `Prompter(text, default=None, hide_input=False, confirmation_prompt=False, type=None, value_proc=None, "
        "prompt_suffix=\": \", show_default=True, err=False, show_choices=True)`: the parameters of `click.prompt`, in "
        "the same order, with the same defaults and meaning, each stored as an attribute of the same name.\n"
        "- `ask()` asks until the answer is valid and returns the value: everything `click.prompt` does today once "
        "it has its arguments.\n"
        "- `read(text)` reads one answer: with `click.termui.hidden_prompt_func` when `hide_input` is set, else with "
        "`click.termui.visible_prompt_func`, each looked up at the moment the answer is read (`CliRunner` and the "
        "documentation tools replace them), and it raises `click.Abort` on an interrupt or at the end of the input, "
        "as today. `ask()` reads every answer, the confirmation included, through `self.read`.\n"
        "- The class method `Prompter.confirm(text, default=False, abort=False, prompt_suffix=\": \", "
        "show_default=True, err=False)` asks a yes/no question: the parameters of `click.confirm`, and everything "
        "`click.confirm` does today.\n"
        "- The static method `Prompter.readline(func, text, err)` is today's helper `_readline_prompt` of "
        "`termui.py`, moved: it hands the text to `func` and strips style codes when the stream needs that. `read` "
        "and `Prompter.confirm` read through it.\n\n"
        "After the change, `click.prompt` does nothing but `return Prompter(<all its arguments>).ask()`, and "
        "`click.confirm` nothing but `return Prompter.confirm(<all its arguments>)`. This holds for every parameter: "
        "whatever parameters `click.prompt` and `click.confirm` have, `Prompter` and `Prompter.confirm` have the same "
        "ones, with the same names, positions and defaults, so that `click.prompt(*args, **kwargs)` and "
        "`Prompter(*args, **kwargs).ask()` always do the same.\n\n"
        "Keep `_readline_prompt` in `termui.py`, with its signature, as a thin function that returns "
        "`Prompter.readline(func, text, err)`, for code that calls it. This is a pure refactor: for every input, "
        "`click.prompt` and `click.confirm` show and return exactly what they show and return today.",
    ),
    "h12-option-prompt": (
        "Refactor: how an option prompts",
        "`Option.prompt_for_value` in `src/click/core.py` decides everything about the prompt of an option in one "
        "method: the default it offers, whether a boolean flag asks a yes/no question instead, and the arguments of "
        "`click.prompt`. Nothing can change one of these without copying the method. Move it into a class "
        "`OptionPrompt` in a new module `src/click/_option_prompt.py`, and add a section \"Custom Option Prompts\" "
        "to `docs/prompts.md` with an example subclass.\n\n"
        "- `OptionPrompt(option, ctx)`, kept as the attributes `option` and `ctx`.\n"
        "- `get_default()` returns the default that the prompt offers, computed once before anything is asked, by "
        "today's rules: `option.get_default(ctx)`, and for a boolean flag `None` when there is no default, else the "
        "default as a `bool`.\n"
        "- `confirm_kwargs(default)` returns the keyword arguments of `click.confirm` for a boolean flag, given that "
        "default: `{\"default\": default}` today.\n"
        "- `prompt_kwargs(default)` returns the keyword arguments of `click.prompt` for any other option, given that "
        "default: `default` (`None` when the option has none); the option's `type`, `hide_input`, `show_choices` "
        "and `confirmation_prompt`; `value_proc` (it converts an answer with "
        "`option.process_value(ctx, answer)`), and `show_default` only when the option's `show_default` is not "
        "`None`.\n"
        "- `ask()` takes `default = self.get_default()`, then calls `click.confirm(option.prompt, "
        "**self.confirm_kwargs(default))` for a boolean flag, else `click.prompt(option.prompt, "
        "**self.prompt_kwargs(default))`, and returns the answer. It passes nothing else, so every argument with "
        "which an option prompts is in one of these two dicts.\n"
        "- `click.Option` gets a class attribute `prompt_class = OptionPrompt` (documented). `Option.consume_value` "
        "prompts with `self.prompt_class(self, ctx).ask()` and no longer calls `prompt_for_value`, so a subclass "
        "changes how its options prompt by setting `prompt_class` to a subclass of `OptionPrompt`.\n\n"
        "Keep `Option.prompt_for_value(ctx)`, with its signature and docstring, as a thin method that returns "
        "`self.prompt_class(self, ctx).ask()`, for code that calls it. This is a pure refactor: every option "
        "prompts exactly as it does today.",
    ),
    "h13-help-formatter": (
        "Refactor: the help formatter in its own module",
        "`src/click/formatting.py` holds `HelpFormatter`, the class that writes help pages, the helpers it lays out "
        "text with (`wrap_text`, `measure_table`, `iter_rows`), and `join_options`. Move `HelpFormatter`, "
        "`wrap_text`, `measure_table` and `iter_rows` into a new module `src/click/_help_formatter.py`, and add a "
        "section \"Custom Help Formatters\" at the end of `docs/documentation.md` that says where the class lives "
        "and shows a subclass used as a context's `formatter_class`.\n\n"
        "- The four live only in the new module (their `__module__` is `\"click._help_formatter\"`), with their "
        "behaviour unchanged.\n"
        "- `join_options` and `FORCED_WIDTH` stay in `formatting.py`.\n"
        "- The width that the test system forces stays the variable `click.formatting.FORCED_WIDTH` (`CliRunner` "
        "sets it there): a `HelpFormatter` made without a width reads it from `click.formatting` when it is "
        "made.\n"
        "- Inside click, every module imports these names from the new module, not from `formatting`: `core.py`, "
        "and `src/click/__init__.py` for `click.HelpFormatter` and `click.wrap_text`.\n\n"
        "Keep the four names importable from `click.formatting`, as the same objects "
        "(`click.formatting.HelpFormatter is click._help_formatter.HelpFormatter`), for code that imports them from "
        "there. This is a pure refactor: every help page stays exactly as it is today.",
    ),
    "h14-default-text": (
        "Refactor: the text of a default",
        "Two places turn a default value into text: the `default` item of an option's help bracket (inside "
        "`Option.get_help_extra` in `src/click/core.py`) and the brackets of a prompt (`_format_default` in "
        "`src/click/termui.py`, used by `_build_prompt`). Move both rules into a new module "
        "`src/click/_default_text.py` with two functions, and add a section \"How Defaults Are Shown\" to "
        "`docs/documentation.md`, right after \"Showing Defaults\", with an example of each.\n\n"
        "- `default_help_text(option, ctx) -> str | None` returns the text of the `default` item of the option's "
        "help bracket (`2` in `[default: 2]`), or `None` when the bracket has no default item. It holds all of "
        "today's rules: whether the default is shown (the option's `show_default`, else the context's), a "
        "`show_default` string shown in parentheses, list and tuple defaults joined with `, `, enum members by "
        "name, `(dynamic)` for a function, the flag name of a boolean flag with a secondary option, an empty "
        "string as `\"\"`, and so on.\n"
        "- `Option.get_help_extra` takes the `default` item only from it: for every option and context, "
        "`option.get_help_extra(ctx).get(\"default\")` equals `default_help_text(option, ctx)`.\n"
        "- `default_prompt_text(default, type=None) -> str` returns the text that a prompt shows between the "
        "brackets for its default value (`alice` in `Name [alice]: `). `type` is the `type` argument of the prompt "
        "(a `ParamType`, a Python type or `None`). Today the text is the name of an open file, and `str()` of "
        "anything else, whatever the type.\n"
        "- Whenever `click.prompt` shows its default value in brackets, the text between the brackets is "
        "`default_prompt_text(default, type)`, called with the prompt's `default` and `type`. A text shown in "
        "place of the default value, such as a `show_default` string (`[(from the keyring)]`), is not the default "
        "value and does not go through it.\n\n"
        "Keep `_format_default` in `termui.py` as a thin function that returns `default_prompt_text(default)`, "
        "for code that calls it. This is a pure refactor: every help page and every prompt stays exactly as it is "
        "today.",
    ),
}

IDS = list(PROMPTS)


def task(k, tid):
    title, prompt = PROMPTS[tid]
    t = {"id": tid, "title": title, "prompt": prompt,
         "edits": ops_from_patch(patch(k, tid, "alone")),
         "test": (HERE / "tests" / f"{tid.replace('-', '_')}.py").read_text()}
    step = patch(k, tid, "step")
    if step is not None:
        t["combined_edits"] = ops_from_patch(step)
    return t


CLICK_HARD = [task(k, tid) for k, tid in enumerate(IDS, 1)]

# Integration tests: the combined behaviour of a designed pair (1-based task numbers, k: 1 h1, 2 h2, 3 h3, 4 h5,
# 5 h6, 6 h7, 7 h8, 8 h9, 9 h10, 10 h11, 11 h12, 12 h13, 13 h14). Each rests only on behaviour that the prompts of
# both tasks state. i1 to i9 are those of `heavy`; i10 to i21 put a feature through the new structure of a refactor.
INTEGRATION = [
    {"id": "i1-prompt-text-enum-names", "pair": [1, 7],
     "why": "trap: h1 keeps today's text, h8 changes it on purpose; PromptText.render() is what prompt shows"},
    {"id": "i2-hidden-default-item", "pair": [2, 3],
     "why": "trap: h2 keeps the bracket as today, h3 hides defaults; the per-item method gives the item"},
    {"id": "i3-group-headings-themed", "pair": [5, 8],
     "why": "interface: h6 writes group sections with formatter.section, h9 styles every heading"},
    {"id": "i4-markdown-hidden-default", "pair": [3, 9],
     "why": "interface: h10 shows the bracket items, h3 hides the default in the bracket"},
    {"id": "i5-markdown-byte-size", "pair": [6, 9],
     "why": "interface: h10 shows the bracket items, h7 formats the default and range as sizes"},
    {"id": "i6-markdown-option-groups", "pair": [5, 9],
     "why": "h10 renders every section of --help through the formatter, h6 adds group sections and a write_dl keyword"},
    {"id": "i7-prompt-text-byte-size", "pair": [1, 6],
     "why": "refactor: h7's formatted prompt default must live in PromptText.default_part"},
    {"id": "i8-markdown-with-a-theme", "pair": [8, 9],
     "why": "h9 styles the help bracket in core, h10 removes every style code"},
    {"id": "i9-markdown-custom-item", "pair": [2, 9],
     "why": "refactor x consumer: an item added through h2's help_extra_keys shows in h10's details"},
    {"id": "i10-prompter-secret", "pair": [10, 3],
     "why": "refactor: h3's hidden input for a Secret type must live in Prompter.ask"},
    {"id": "i11-prompter-attempts", "pair": [10, 4],
     "why": "refactor: h5's max_attempts must be a parameter of Prompter and Prompter.confirm and work in their loops"},
    {"id": "i12-prompter-choice-menu", "pair": [10, 7],
     "why": "refactor: h8's choice_menu must be a parameter of Prompter and work in Prompter.ask"},
    {"id": "i13-option-prompt-attempts", "pair": [11, 4],
     "why": "refactor: h5's prompt_attempts must reach OptionPrompt.prompt_kwargs and confirm_kwargs"},
    {"id": "i14-option-prompt-menu", "pair": [11, 7],
     "why": "refactor: h8's prompt_menu must reach OptionPrompt.prompt_kwargs"},
    {"id": "i15-option-prompt-secret", "pair": [11, 3],
     "why": "refactor x feature: the prompt of a Secret option hides its input in OptionPrompt.prompt_kwargs"},
    {"id": "i16-help-formatter-groups", "pair": [12, 5],
     "why": "refactor: h6's write_dl(first_col_width=) must be in the class of click._help_formatter"},
    {"id": "i17-help-formatter-theme", "pair": [12, 8],
     "why": "refactor: h9's theme must be in the class of click._help_formatter"},
    {"id": "i18-help-formatter-markdown", "pair": [12, 9],
     "why": "refactor: h10's MarkdownHelpFormatter must subclass the class of click._help_formatter"},
    {"id": "i19-default-text-hidden", "pair": [13, 3],
     "why": "refactor: h3's (hidden) default must come from default_help_text"},
    {"id": "i20-default-text-byte-size", "pair": [13, 6],
     "why": "refactor: h7's sizes must come from default_help_text and default_prompt_text"},
    {"id": "i21-default-text-enum", "pair": [13, 7],
     "why": "refactor: h8's enum names in prompts must come from default_prompt_text"},
]
for it in INTEGRATION:
    it["test"] = (HERE / "integration" / f"{it['id'].replace('-', '_')}.py").read_text()

# Set-level settings (tasks/click/hard/set.json): 13 agents, all Claude Code (the model of the next step; the
# rule is that git with a hand-back loses with Claude agents alone). Limits as for `heavy`.
SETTINGS = {
    "agents": 13,
    "mix": ["claude"] * 13,
    "limits": {"max_turns": 300, "max_tokens": 20_000_000, "timeout_s": 3600, "claude_budget_usd": 8.0},
    "sequential": True,
}

# Trees of all tasks but k, written by hand where merging the other steps without task k does not give a
# working tree (t2.replay_without): the refactors h11 to h14 come last and their steps move the code of the
# features, so without a feature their steps do not apply.
WITHOUT = {"click": {"hard": {int(f.name.split("-")[1].split(".")[0]): ops_from_patch(f.read_text())
                              for f in sorted((HERE / "patches").glob("without-*.patch"))}}}

TASKS = {"click": {"hard": CLICK_HARD}}
INTEGRATIONS = {"click": {"hard": INTEGRATION}}
SET_SETTINGS = {"hard": SETTINGS}
SETS = ("hard",)
