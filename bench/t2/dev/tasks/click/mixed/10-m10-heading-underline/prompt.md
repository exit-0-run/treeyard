# Help: an optional underline for section headings

Add a parameter `heading_underline: str | None = None` to `click.HelpFormatter` (in `src/click/formatting.py`, documented in the class docstring). When it is set to a character such as `"-"`, `write_heading` writes the heading as before and then, on the next line and at the same indentation, that character repeated as many times as the heading line is wide, colon included: `HelpFormatter(heading_underline="-")` writes `Options:` and then `--------`. The width is what the terminal shows: ANSI style codes in the heading take no columns. A command uses it through `Context.formatter_class`. Without it the help output is as before.

Keep the existing behaviour otherwise. Finish the work completely.
