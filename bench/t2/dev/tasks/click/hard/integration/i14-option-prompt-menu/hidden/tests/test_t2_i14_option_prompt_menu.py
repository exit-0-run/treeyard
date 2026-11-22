import click
from click._option_prompt import OptionPrompt


def test_the_option_prompt_passes_the_menu():
    opt = click.Option(["--region"], prompt=True, type=click.Choice(["eu", "us"]), prompt_menu=True)
    ctx = click.Context(click.Command("cli", params=[opt]))
    assert OptionPrompt(opt, ctx).prompt_kwargs(None)["choice_menu"] is True
