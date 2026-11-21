# A duration parameter type

Add a parameter type `click.Duration` in `src/click/types.py`, exported from `click` (next to the other types in `src/click/__init__.py`) and listed with an `autoclass` entry in the Types section of `docs/api.md`, after `DateTime`. It converts a duration to a `datetime.timedelta`: a number of seconds (`"90"`, `"1.5"`), or one or more parts with the units `d`, `h`, `m` and `s` in that order (`"2d"`, `"1h30m"`, `"45s"`, `"1d2h3m4s"`), with upper case units accepted and spaces around the value ignored. A `timedelta` passes through unchanged. Anything else fails with `'<value>' is not a valid duration.` (the value as given, in `repr` form). Its `name` is `"duration"`. Also let click pick it on its own in `convert_type`: an option whose default is a `timedelta`, or whose `type` is the class `datetime.timedelta`, gets a `Duration` type.

Keep the existing behaviour otherwise. Finish the work completely.
