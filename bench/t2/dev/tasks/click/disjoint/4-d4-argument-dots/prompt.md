# Argument: dots in the name

In `src/click/core.py`, an argument declared with a dot in its name, such as `@click.argument("src.path")`, gets the Python name `src.path`, which cannot be a keyword argument of the callback. Replace dots with underscores, the same way as dashes: `click.Argument(["src.path"]).name` is `"src_path"`.

Keep the existing behaviour otherwise. Finish the work completely.
