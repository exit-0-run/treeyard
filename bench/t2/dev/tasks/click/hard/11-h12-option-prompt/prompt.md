# Refactor: how an option prompts

`Option.prompt_for_value` in `src/click/core.py` decides everything about the prompt of an option in one method: the default it offers, whether a boolean flag asks a yes/no question instead, and the arguments of `click.prompt`. Nothing can change one of these without copying the method. Move it into a class `OptionPrompt` in a new module `src/click/_option_prompt.py`, and add a section "Custom Option Prompts" to `docs/prompts.md` with an example subclass.

- `OptionPrompt(option, ctx)`, kept as the attributes `option` and `ctx`.
- `get_default()` returns the default that the prompt offers, computed once before anything is asked, by today's rules: `option.get_default(ctx)`, and for a boolean flag `None` when there is no default, else the default as a `bool`.
- `confirm_kwargs(default)` returns the keyword arguments of `click.confirm` for a boolean flag, given that default: `{"default": default}` today.
- `prompt_kwargs(default)` returns the keyword arguments of `click.prompt` for any other option, given that default: `default` (`None` when the option has none); the option's `type`, `hide_input`, `show_choices` and `confirmation_prompt`; `value_proc` (it converts an answer with `option.process_value(ctx, answer)`), and `show_default` only when the option's `show_default` is not `None`.
- `ask()` takes `default = self.get_default()`, then calls `click.confirm(option.prompt, **self.confirm_kwargs(default))` for a boolean flag, else `click.prompt(option.prompt, **self.prompt_kwargs(default))`, and returns the answer. It passes nothing else, so every argument with which an option prompts is in one of these two dicts.
- `click.Option` gets a class attribute `prompt_class = OptionPrompt` (documented). `Option.consume_value` prompts with `self.prompt_class(self, ctx).ask()` and no longer calls `prompt_for_value`, so a subclass changes how its options prompt by setting `prompt_class` to a subclass of `OptionPrompt`.

Keep `Option.prompt_for_value(ctx)`, with its signature and docstring, as a thin method that returns `self.prompt_class(self, ctx).ask()`, for code that calls it. This is a pure refactor: every option prompts exactly as it does today.

Keep the existing behaviour otherwise. Finish the work completely.
