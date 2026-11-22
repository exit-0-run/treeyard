import click
from click.testing import CliRunner


def test_the_prompter_shows_a_numbered_menu():
    @click.command()
    def cli():
        choice = click.Choice(["eu", "us", "ap"])
        click.echo(click.Prompter("Region", type=choice, choice_menu=True, default="us").ask())

    result = CliRunner().invoke(cli, input="3\n")
    assert result.exception is None, result.output
    assert result.output == "  1) eu\n  2) us\n  3) ap\nRegion (1-3) [2]: 3\nap\n"
