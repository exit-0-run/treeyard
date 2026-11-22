# Refactor: the help formatter in its own module

`src/click/formatting.py` holds `HelpFormatter`, the class that writes help pages, the helpers it lays out text with (`wrap_text`, `measure_table`, `iter_rows`), and `join_options`. Move `HelpFormatter`, `wrap_text`, `measure_table` and `iter_rows` into a new module `src/click/_help_formatter.py`, and add a section "Custom Help Formatters" at the end of `docs/documentation.md` that says where the class lives and shows a subclass used as a context's `formatter_class`.

- The four live only in the new module (their `__module__` is `"click._help_formatter"`), with their behaviour unchanged.
- `join_options` and `FORCED_WIDTH` stay in `formatting.py`.
- The width that the test system forces stays the variable `click.formatting.FORCED_WIDTH` (`CliRunner` sets it there): a `HelpFormatter` made without a width reads it from `click.formatting` when it is made.
- Inside click, every module imports these names from the new module, not from `formatting`: `core.py`, and `src/click/__init__.py` for `click.HelpFormatter` and `click.wrap_text`.

Keep the four names importable from `click.formatting`, as the same objects (`click.formatting.HelpFormatter is click._help_formatter.HelpFormatter`), for code that imports them from there. This is a pure refactor: every help page stays exactly as it is today.

Keep the existing behaviour otherwise. Finish the work completely.
