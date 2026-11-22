# Help pages: option groups

Commands with many options get a help page that is one long list. Let options be listed in groups.

- `click.Option` takes `help_group: str | None = None` (an attribute of the same name, documented): the name of a section of the help page, such as `"Network"`.
- `Command.format_options` lists the options without a group in the `Options` section as today (the `--help` option among them), then one section per group, headed by the group name exactly as given (`Network:`). Each group section is written the same way as `Options`: `formatter.section(name)` and `formatter.write_dl(rows)`, so a formatter that changes how headings or lists look applies to it too. Hidden options are left out, and a section without any shown option is not written (not even `Options`).
- The order of the groups: first those named in a new parameter `help_group_order` of `click.Command` (a sequence of names, stored as the tuple attribute `help_group_order`, empty by default), then the others in the order their first option appears.
- The second column starts at the same place in all option sections: the first column is as wide as the widest option over all these sections (capped as today). For this `HelpFormatter.write_dl` takes a new keyword `first_col_width: int | None = None`, used in place of the width of the widest term of the rows, still capped at `col_max`.
- A new method `Command.get_help_groups(ctx)` returns the rows: a dict from group name (`None` for the options without a group, always first when present) to the list of `(spec, help)` records of its shown options, in the section order. Arguments are not in it.
- The commands of a `click.Group` can be grouped the same way: `click.Command` takes `help_group: str | None = None` too, and the group's help lists the commands without a group under `Commands` as today, then one section per command group, ordered by the group's `help_group_order` and then by first appearance, with the second column lined up across these sections.

A help page without groups stays as it is. Add a section "Option Groups" to `docs/documentation.md`.

Keep the existing behaviour otherwise. Finish the work completely.
