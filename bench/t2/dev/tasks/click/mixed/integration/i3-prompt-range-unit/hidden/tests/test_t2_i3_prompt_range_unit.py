import click
from click.testing import CliRunner


def test_the_prompt_shows_the_unit():
    @click.command()
    def cli():
        click.echo(click.prompt("Wait", type=click.IntRange(1, 5, unit="s")))

    result = CliRunner().invoke(cli, input="2\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Wait (1<=x<=5 s): ")
