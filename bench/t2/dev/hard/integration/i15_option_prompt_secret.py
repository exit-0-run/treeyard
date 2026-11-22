import click
from click._option_prompt import OptionPrompt


def test_the_option_prompt_of_a_secret_hides_the_input():
    opt = click.Option(["--token"], prompt=True, type=click.Secret())
    ctx = click.Context(click.Command("cli", params=[opt]))
    kwargs = OptionPrompt(opt, ctx).prompt_kwargs(None)
    assert kwargs["hide_input"] is True
    assert kwargs["type"] is opt.type
