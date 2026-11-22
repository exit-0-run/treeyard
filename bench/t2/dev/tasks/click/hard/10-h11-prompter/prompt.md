# Refactor: the prompt loop

`click.prompt` and `click.confirm` in `src/click/termui.py` each build their text and then run a loop: read an answer, convert it, write an error line and ask again. The loops sit inside the two functions, so nothing can change one step without copying them. Move them into a class `Prompter` in a new module `src/click/_prompter.py`, export it as `click.Prompter` (it is also importable as `click.termui.Prompter`), document it with an `autoclass` entry (with its members) in `docs/api.md` right after `confirm`, and add a section "How a Prompt Asks" to `docs/prompts.md` with an example subclass.

- `Prompter(text, default=None, hide_input=False, confirmation_prompt=False, type=None, value_proc=None, prompt_suffix=": ", show_default=True, err=False, show_choices=True)`: the parameters of `click.prompt`, in the same order, with the same defaults and meaning, each stored as an attribute of the same name.
- `ask()` asks until the answer is valid and returns the value: everything `click.prompt` does today once it has its arguments.
- `read(text)` reads one answer: with `click.termui.hidden_prompt_func` when `hide_input` is set, else with `click.termui.visible_prompt_func`, each looked up at the moment the answer is read (`CliRunner` and the documentation tools replace them), and it raises `click.Abort` on an interrupt or at the end of the input, as today. `ask()` reads every answer, the confirmation included, through `self.read`.
- The class method `Prompter.confirm(text, default=False, abort=False, prompt_suffix=": ", show_default=True, err=False)` asks a yes/no question: the parameters of `click.confirm`, and everything `click.confirm` does today.
- The static method `Prompter.readline(func, text, err)` is today's helper `_readline_prompt` of `termui.py`, moved: it hands the text to `func` and strips style codes when the stream needs that. `read` and `Prompter.confirm` read through it.

After the change, `click.prompt` does nothing but `return Prompter(<all its arguments>).ask()`, and `click.confirm` nothing but `return Prompter.confirm(<all its arguments>)`. This holds for every parameter: whatever parameters `click.prompt` and `click.confirm` have, `Prompter` and `Prompter.confirm` have the same ones, with the same names, positions and defaults, so that `click.prompt(*args, **kwargs)` and `Prompter(*args, **kwargs).ask()` always do the same.

Keep `_readline_prompt` in `termui.py`, with its signature, as a thin function that returns `Prompter.readline(func, text, err)`, for code that calls it. This is a pure refactor: for every input, `click.prompt` and `click.confirm` show and return exactly what they show and return today.

Keep the existing behaviour otherwise. Finish the work completely.
