# Prompts: cut a long list of choices

`click.prompt` lists every choice of a `Choice` type after the text, which gets unreadable for long lists. Add a keyword parameter `max_choices: int | None = None` to `click.prompt` (in all its overloads and in the docstring): when it is set and the type has more choices than that, the prompt lists only the first `max_choices` of them, then `...`. Example: `click.prompt("Region", type=click.Choice(["eu", "us", "ap", "sa"]), max_choices=2)` shows `Region (eu, us, ...): `. With `max_choices=None`, or when the choices fit, the prompt stays as it is. Every choice is still accepted.

Keep the existing behaviour otherwise. Finish the work completely.
