# Ranges: a unit, and a label in the option help

`click.IntRange` and `click.FloatRange` take a new keyword argument `unit: str | None = None`, a unit name for the help and the error messages. The range description then ends with a space and the unit: `click.IntRange(1, 5, unit="s")` describes itself as `1<=x<=5 s`, in its `repr` and in the error `7 is not in the range 1<=x<=5 s.`. Also give the range item in an option's help bracket a label, like the other items: an option with `type=click.IntRange(1, 5, unit="s")` shows `[range: 1<=x<=5 s]`. Each item in the bracket says what it is, in the form `label: value` (`env var: ...`, `default: ...`, `range: ...`); the only item without a label is `required`. `Option.get_help_extra(ctx)["range"]` stays the bare description (`1<=x<=5 s`). Add your regression case to the parametrised test `test_intrange_default_help_text` in `tests/test_options.py`.

Keep the existing behaviour otherwise. Finish the work completely.
