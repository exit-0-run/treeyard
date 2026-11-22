import click
from click._option_prompt import OptionPrompt


def make(*decls, **kwargs):
    opt = click.Option(list(decls), **kwargs)
    return opt, click.Context(click.Command("cli", params=[opt]))


def test_the_option_prompt_passes_the_attempts():
    opt, ctx = make("--n", prompt=True, type=int, prompt_attempts=3)
    assert OptionPrompt(opt, ctx).prompt_kwargs(None)["max_attempts"] == 3
    opt, ctx = make("--force/--no-force", prompt=True, prompt_attempts=2)
    assert OptionPrompt(opt, ctx).confirm_kwargs(False)["max_attempts"] == 2
