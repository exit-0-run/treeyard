import click
from click._default_text import default_help_text


def make(*decls, **kwargs):
    opt = click.Option(list(decls), **kwargs)
    return opt, click.Context(click.Command("cli", params=[opt]))


def test_the_help_text_of_a_hidden_default():
    assert default_help_text(*make("--pw", default="hunter2", show_default=True, hide_input=True)) == "(hidden)"
    assert default_help_text(*make("--token", default="abc", show_default=True, type=click.Secret())) == "(hidden)"
    assert default_help_text(*make("--pw", show_default="keyring", hide_input=True)) == "(keyring)"
