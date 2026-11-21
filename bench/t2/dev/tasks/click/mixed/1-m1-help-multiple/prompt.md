# Option help: say that an option can be repeated

An option declared with `multiple=True` can be given several times, but its help line does not say so. Add a parameter `show_multiple: bool = False` to `click.Option`, documented in its docstring next to `show_envvar`. When it is true and the option has `multiple=True`, the bracket at the end of the option's help line gets the item `multiple`, after the range and before `required`. Example: `@click.option("--tag", multiple=True, show_multiple=True, help="A tag.")` shows `A tag.  [multiple]` in `--help`; with `default=("a", "b"), show_default=True, required=True` added it shows `[default: a, b; multiple; required]`. `Option.get_help_extra(ctx)` returns the item under the key `"multiple"` with the value `"multiple"`; declare the key in the `OptionHelpExtra` typed dict in `src/click/types.py`. Without `show_multiple` the help stays as it is.

Keep the existing behaviour otherwise. Finish the work completely.
