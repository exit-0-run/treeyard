import enum

import click
from click.testing import CliRunner


class Color(enum.Enum):
    RED = 1
    GREEN = 2
    BLUE = 3


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


def test_enum_choices_are_listed_by_name():
    result = ask("RED\n", text="Color", type=click.Choice(Color))
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Color (RED, GREEN, BLUE): ")
    assert "value=<Color.RED: 1>" in result.output


def test_an_enum_default_is_shown_by_name():
    result = ask("\n", text="Color", type=click.Choice(Color), default=Color.GREEN)
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Color (RED, GREEN, BLUE) [GREEN]: ")


def test_plain_choices_and_defaults_stay():
    result = ask("\n", text="Size", type=click.Choice(["s", "m"]), default="m")
    assert result.output.startswith("Size (s, m) [m]: ")
