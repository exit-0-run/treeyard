import enum

import click
from click.testing import CliRunner


class Color(enum.Enum):
    RED = 1
    GREEN = 2
    BLUE = 3
    BLACK = 4


def test_enum_names_and_a_cut_list_together():
    @click.command()
    def cli():
        click.echo(repr(click.prompt("Pick", type=click.Choice(Color), max_choices=2)))

    result = CliRunner().invoke(cli, input="BLACK\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Pick (RED, GREEN, ...): ")
