# Secrets: a secret type and hidden defaults

Options with `hide_input=True` (passwords, tokens) leak their value in several places: `--help` with `show_default=True` shows `[default: hunter2]`, the prompt shows `Password [hunter2]: `, `Option.to_info_dict()` reports the default, and an error such as `'abc9' is not a valid integer.` repeats the value given on the command line. Fix all of them and add a type for secret values.

- `click.SecretStr(value)` (in `src/click/types.py`, exported from `click`): holds a string that does not show itself. `str()` and formatting give `***`, `repr()` gives `SecretStr('***')`, `reveal()` returns the value. Two secrets are equal (and hash alike) when their values are; `len()` and truth are those of the value.
- `click.Secret()` (exported too): a parameter type named `"secret"` with the metavar `SECRET`; it converts text to a `SecretStr` and passes a `SecretStr` through unchanged. An `Option` of this type always has `hide_input` set to `True`, and `click.prompt(..., type=click.Secret())` hides the input as with `hide_input=True`.
- The default of an option with `hide_input` (or a `Secret` type) shows as `(hidden)`: `get_help_extra(ctx)` gives `{"default": "(hidden)"}`, the help shows `[default: (hidden)]`, and `to_info_dict()["default"]` is `"(hidden)"` (it stays `None` without a default). A `show_default` string is not the value and is shown as before (`[default: (from the keyring)]`).
- `click.prompt("Password", default="hunter2", hide_input=True)` shows `Password [(hidden)]: `, and so does an option with `prompt=True, hide_input=True` or a `Secret` type.
- An error about a value given for a hidden-input option, from the command line or an environment variable, shows `'***'` in place of the value: `Error: Invalid value for '--pin': '***' is not a valid integer.`

Defaults and errors of inputs that are not hidden show as before. Document both classes with `autoclass` entries in the Types section of `docs/api.md` (after `Tuple`) and in the list of types in `docs/parameter-types.md`, the `hide_input` parameters of `Option` and `click.prompt`, and add a section about secrets to `docs/option-decorators.md`.

Keep the existing behaviour otherwise. Finish the work completely.
