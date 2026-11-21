# Never show the default of a hidden input

An option with `hide_input=True` (a password, a token) still shows its default value in clear text: in `--help` with `show_default=True` (`[default: hunter2]`) and in the prompt (`Password [hunter2]: `). Show `(hidden)` in its place in both: `[default: (hidden)]` in the help, and `Password [(hidden)]: ` when `click.prompt("Password", default="hunter2", hide_input=True)` or an option with `prompt=True, hide_input=True` asks. Update the docstring of the `hide_input` parameter of `click.prompt`. A `show_default` string is not the value and is shown as before (`[default: (from the keyring)]`); defaults of inputs that are not hidden show as before.

Keep the existing behaviour otherwise. Finish the work completely.
