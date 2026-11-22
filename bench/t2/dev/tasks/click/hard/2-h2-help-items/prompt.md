# Refactor: one method per item of the option help bracket

The bracket at the end of an option's help line (`[env var: N; default: 2; 1<=x<=5; required]`) is built by `Option.get_help_extra` in `src/click/core.py`, one long method that computes all items, and `Option.get_help_record`, which formats them with a chain of `if`s. Nobody can add an item or change one without copying both. Split them:

- A class attribute `Option.help_extra_keys = ("envvars", "default", "range", "required")`: the keys of the bracket, in the order they are shown.
- For each key a method `get_help_extra_<key>(ctx)` that returns its value, or `None` to leave the item out: `get_help_extra_envvars` (a tuple of names), `get_help_extra_default`, `get_help_extra_range`, `get_help_extra_required` (strings), with today's rules for when each item appears.
- `get_help_extra(ctx)` returns `{key: value}` for each key of `help_extra_keys` whose method does not return `None`, in that order.
- `format_help_extra(ctx, extra)` returns the texts of the items of the dict `extra`, in the order of `help_extra_keys` (keys that are not in `help_extra_keys` are left out); `format_help_extra_item(key, value)` is the text of one item: `env var: A, B`, `default: 2`, the bare range, `required`, and for any other key `<key>: <value>` (a tuple or list value joined with `, `).
- A function `append_help_extra(help, items)` in `src/click/formatting.py` appends the bracket to a help text: `append_help_extra("Name.", ["default: x", "required"])` is `"Name.  [default: x; required]"`; with an empty help text the bracket stands alone (`"[required]"`), without items the help text is returned as it is. `get_help_record` uses `get_help_extra`, `format_help_extra` and this function.

So a subclass adds an item by extending `help_extra_keys` and writing its method (`help_extra_keys = (*click.Option.help_extra_keys, "unit")` with `get_help_extra_unit` returning `"s"` shows `[default: 30; unit: s]`), and drops one by returning `None`. Document the methods, mention the keys in the docstring of the `OptionHelpExtra` typed dict in `src/click/types.py`, and add a section "The Option Help Bracket" with such an example to `docs/documentation.md`. This is a refactor: every help page and every `get_help_extra` result stays exactly as it is today.

Keep the existing behaviour otherwise. Finish the work completely.
