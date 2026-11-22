import enum

import click
from click.testing import CliRunner


class Color(enum.Enum):
    RED = 1
    GREEN = 2


def test_prompt_text_shows_enum_names_like_the_prompt():
    text = click.PromptText("Color", ": ", True, Color.GREEN, True, click.Choice(Color))
    assert text.render() == "Color (RED, GREEN) [GREEN]: "

    @click.command()
    def cli():
        click.prompt("Color", type=click.Choice(Color), default=Color.GREEN)

    assert CliRunner().invoke(cli, input="\n").output.startswith(text.render())
