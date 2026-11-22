import click
from click._default_text import default_help_text
from click._default_text import default_prompt_text


def test_the_text_of_a_size_default():
    opt = click.Option(["--cache"], type=click.ByteSize(), default=10 * 1024**2, show_default=True)
    ctx = click.Context(click.Command("cli", params=[opt]))
    assert default_help_text(opt, ctx) == "10 MiB"
    assert default_prompt_text(10485760, click.ByteSize()) == "10 MiB"
