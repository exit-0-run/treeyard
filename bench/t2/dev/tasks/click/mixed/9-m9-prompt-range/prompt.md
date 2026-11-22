# Prompts: show the accepted range

`click.prompt` with an `IntRange` or `FloatRange` type does not tell the user which values it accepts until they type a wrong one. Show the range after the text, in the same form as in the help and the error messages: `click.prompt("Count", type=click.IntRange(1, 5))` shows `Count (1<=x<=5): `, and `click.FloatRange(0, 1, max_open=True)` shows `(0<=x<1)`. The text in the parentheses is always the same text that the type's `repr` and its error message (`7 is not in the range 1<=x<=5.`) show for the range, so it changes when they do. Options that prompt show it too (`@click.option("--count", type=click.IntRange(1, 5), prompt=True)` asks `Count (1<=x<=5): `). Add a keyword parameter `show_range: bool = True` to `click.prompt` (in all its overloads and in the docstring) that turns it off. Prompts with other types stay as they are.

Keep the existing behaviour otherwise. Finish the work completely.
