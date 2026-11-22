# Choice prompts: enum names and a numbered menu

Two problems with prompts for a `click.Choice`. A choice built from an enum accepts the member names (`RED`), but `click.prompt` lists the choices with `str()` (`(Color.RED, Color.GREEN)`) and shows an enum default as `[Color.GREEN]`. And a long list of choices is hard to type.

- In prompts, an enum member is shown by its name, in the list of choices and as the default: `click.prompt("Color", type=click.Choice(Color), default=Color.GREEN)` shows `Color (RED, GREEN, BLUE) [GREEN]: `. Other choices and defaults show as before. For this `Choice` gets a method `format_choice(choice) -> str`: the name of an enum member, `str()` of anything else.
- `click.prompt` takes a keyword parameter `choice_menu: bool = False` (all overloads, documented). With a `Choice` type it first prints a numbered menu, one line per choice, `  1) eu`, `  2) us` (two spaces, the number, `) `, `format_choice` of the choice), once, before the first prompt line. The prompt then shows the range of numbers in place of the list of choices, `Region (1-3): `, and a default that is one of the choices as its number: `Region (1-3) [2]: `.
- The answer may be the number of a choice or a choice itself. An answer that is itself a valid choice is that choice, so choices that are numbers keep working (`["10", "20"]`: `20` is `"20"`, `2` is also `"20"`); any other number is an error `Error: 9 is not a number of the menu (1-3).` and the prompt asks again. A `Choice` method `choice_from_number(answer)` returns the choice for a menu number (`"1"` is the first), or `None`. With other types `choice_menu` changes nothing.
- `click.Option` takes `prompt_menu: bool = False` (documented) and prompts with the menu; with a type that is not a `Choice` it raises `TypeError`.

Add a section "Choice Menus" to `docs/prompts.md`.

Keep the existing behaviour otherwise. Finish the work completely.
