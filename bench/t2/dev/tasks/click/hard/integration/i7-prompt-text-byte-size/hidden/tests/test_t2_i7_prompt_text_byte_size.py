import click
from click.testing import CliRunner


def test_prompt_text_shows_sizes_like_the_prompt():
    size = click.ByteSize()
    text = click.PromptText("Size", ": ", True, 10 * 1024**2, True, size)
    assert text.render() == "Size [10 MiB]: "
    assert text.default_part() == " [10 MiB]"

    @click.command()
    def cli():
        click.prompt("Size", type=size, default=10 * 1024**2)

    assert CliRunner().invoke(cli, input="\n").output.startswith("Size [10 MiB]: ")
