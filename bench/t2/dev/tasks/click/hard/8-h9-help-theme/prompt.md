# Help pages: themes

Let applications style their help pages. Add a class `click.HelpTheme` to `src/click/formatting.py` (exported from `click`, an `autoclass` entry in the Formatting section of `docs/api.md` after `HelpFormatter`, and a section "Help Themes" at the end of `docs/documentation.md`).

- `HelpTheme(heading=None, option=None, usage=None, extra=None)`: each argument is a dict of keyword arguments for `click.style` (`{"fg": "cyan", "bold": True}`) or `None` for no style, stored as an attribute of the same name. Two themes with the same styles are equal. `apply(part, text)` returns `text` styled with the style of the part named `part` (`"heading"`, ...), or `text` unchanged when that part has no style.
- `click.HelpFormatter` takes `theme: HelpTheme | None = None`, kept as the attribute `theme` (an empty `HelpTheme()` when none is given). `write_heading` styles the heading with its colon (`style("Options:", bold=True)`), `write_dl` styles the terms of the first column with the `option` style (the term only, not the padding), and `write_usage` styles the default `Usage:` prefix (not the space after it) with the `usage` style. Styles take no columns: once unstyled, the output is exactly the output without a theme.
- The bracket at the end of an option's help line, brackets included (`[default: 1]`), gets the `extra` style.
- `click.Context` takes `help_theme: HelpTheme | None = None` (so also through `context_settings`), inherited from the parent context when not given, kept as the attribute `help_theme`. `Context.make_formatter` passes it to the formatter as `theme`, but only when it is set, so formatter classes that do not take a `theme` keep working. Subcommands and the command list of a group are styled the same way.

As with all styles, the codes are removed when the help is not written to a terminal unless colors are forced (`CliRunner.invoke(..., color=True)`).

Keep the existing behaviour otherwise. Finish the work completely.
