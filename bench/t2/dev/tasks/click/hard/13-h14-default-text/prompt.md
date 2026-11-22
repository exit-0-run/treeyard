# Refactor: the text of a default

Two places turn a default value into text: the `default` item of an option's help bracket (inside `Option.get_help_extra` in `src/click/core.py`) and the brackets of a prompt (`_format_default` in `src/click/termui.py`, used by `_build_prompt`). Move both rules into a new module `src/click/_default_text.py` with two functions, and add a section "How Defaults Are Shown" to `docs/documentation.md`, right after "Showing Defaults", with an example of each.

- `default_help_text(option, ctx) -> str | None` returns the text of the `default` item of the option's help bracket (`2` in `[default: 2]`), or `None` when the bracket has no default item. It holds all of today's rules: whether the default is shown (the option's `show_default`, else the context's), a `show_default` string shown in parentheses, list and tuple defaults joined with `, `, enum members by name, `(dynamic)` for a function, the flag name of a boolean flag with a secondary option, an empty string as `""`, and so on.
- `Option.get_help_extra` takes the `default` item only from it: for every option and context, `option.get_help_extra(ctx).get("default")` equals `default_help_text(option, ctx)`.
- `default_prompt_text(default, type=None) -> str` returns the text that a prompt shows between the brackets for its default value (`alice` in `Name [alice]: `). `type` is the `type` argument of the prompt (a `ParamType`, a Python type or `None`). Today the text is the name of an open file, and `str()` of anything else, whatever the type.
- Whenever `click.prompt` shows its default value in brackets, the text between the brackets is `default_prompt_text(default, type)`, called with the prompt's `default` and `type`. A text shown in place of the default value, such as a `show_default` string (`[(from the keyring)]`), is not the default value and does not go through it.

Keep `_format_default` in `termui.py` as a thin function that returns `default_prompt_text(default)`, for code that calls it. This is a pure refactor: every help page and every prompt stays exactly as it is today.

Keep the existing behaviour otherwise. Finish the work completely.
