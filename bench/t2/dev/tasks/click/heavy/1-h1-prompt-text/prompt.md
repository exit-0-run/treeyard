# Refactor: the prompt text

The text that `click.prompt` and `click.confirm` show before they read an answer is built by the private function `_build_prompt` in `src/click/termui.py`, from strings glued together in three places. Move it into a class `PromptText` in a new module `src/click/_prompt.py`, export it as `click.PromptText` (it is also importable as `click.termui.PromptText`), document it with an `autoclass` entry (with its members) in `docs/api.md` right after `confirm`, and add a section to `docs/prompts.md` that explains it.

- `PromptText(text, suffix=": ", show_default=False, default=None, show_choices=True, type=None)`: the arguments in this order (positional or keyword), the same meaning as those of `_build_prompt` today.
- `choices_part()` returns the choices of a `Choice` type in parentheses with a leading space (`" (s, m)"`), or `""` when there is no `Choice` type or `show_choices` is false.
- `default_part()` returns the default in brackets with a leading space (`" [m]"`; a `show_default` string shows as `" [(that string)]"`), or `""` when nothing is shown.
- `render()` returns the text, then `choices_part()`, then `default_part()`, then the suffix. It always calls these two methods, so a subclass that overrides one of them (or code that replaces it on the class) changes the result. `str()` of the object is `render()`.
- The class method `PromptText.for_confirm(text, default=False, suffix=": ", show_default=True)` makes the text of a yes/no question: `PromptText.for_confirm("Go?").render()` is `"Go? [y/N]: "`, with `default=True` it shows `[Y/n]`, with `None` `[y/n]`.

`click.prompt` (its prompt and its confirmation prompt) and `click.confirm` must build their text only through `PromptText`. Keep `_build_prompt` in `termui.py`, with its signature, as a thin function that returns `PromptText(...).render()`, for code that calls it; the helper `_format_default` moves to the new module with the class. This is a pure refactor: for every input, `click.prompt` and `click.confirm` must show exactly the text they show today.

Keep the existing behaviour otherwise. Finish the work completely.
