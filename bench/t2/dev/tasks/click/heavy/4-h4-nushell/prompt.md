# Shell completion for Nushell

Add shell completion for Nushell to `src/click/shell_completion.py`: a class `NuComplete(ShellComplete)` with `name = "nu"`, registered with the other shells so that `get_completion_class("nu")` returns it and the instructions `nu_source` and `nu_complete` work.

Nushell has no completion scripts per command; it calls one external completer with the words of the command line, and that completer returns a list of records. So:

- `source()` returns a Nushell script that defines a command `def "<func_name>" [spans: list<string>]` (`func_name` as for the other shells, `_foo_bar_completion` for `foo-bar`). It runs the program (`^foo-bar`) with the completion variable (`_FOO_BAR_COMPLETE`) set to `nu_complete` and `COMP_WORDS` set to the spans, one per line, and parses the answer with `from json`. A comment in the script shows how to call it from `$env.config.completions.external.completer`. When a `nu` executable is on the `PATH` and is older than 0.90, `source()` also writes a warning to stderr, like the Bash version check.
- `get_completion_args()` reads `COMP_WORDS`: the first line is the program, the last line is the word being completed (empty after a space), the lines between are the complete arguments; `foo-bar\ndeploy\n--re` gives `(["deploy"], "--re")`.
- `complete()` returns a JSON list with one object per completion: `{"value": ...}`, plus `"description"` when the item has a help text, its whitespace (newlines included) collapsed to single spaces. When any item is of type `file` or `dir`, it returns `null` instead, so Nushell completes paths itself.

Document it: Nushell in the list of supported shells and in both tab sets of `docs/shell-completion.md` (the `eval`-like setup and the saved-script setup), plus a section on how the external completer calls the command, and a Nushell paragraph in `examples/completion/README`.

Keep the existing behaviour otherwise. Finish the work completely.
