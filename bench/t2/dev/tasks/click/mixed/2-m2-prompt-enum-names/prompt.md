# Prompts: show enum choices by name

A `click.Choice` built from an enum accepts the member names on the command line (`RED`), but `click.prompt` lists the choices with `str()`, so a plain `enum.Enum` shows `(Color.RED, Color.GREEN)`, and a default member shows as `[Color.GREEN]`. In `src/click/termui.py`, show enum members by their name in both places: `click.prompt("Color", type=click.Choice(Color), default=Color.GREEN)` shows `Color (RED, GREEN, BLUE) [GREEN]: `. Choices and defaults that are not enum members show as before.

Keep the existing behaviour otherwise. Finish the work completely.
