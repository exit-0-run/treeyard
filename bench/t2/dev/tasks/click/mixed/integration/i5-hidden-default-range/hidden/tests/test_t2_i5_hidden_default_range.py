import click
from click.testing import CliRunner


def test_a_hidden_pin_with_a_range():
    @click.command()
    def cli():
        click.echo(f"pin={click.prompt('PIN', default=1234, hide_input=True, type=click.IntRange(0, 9999))}")

    result = CliRunner().invoke(cli, input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("PIN (0<=x<=9999) [(hidden)]: ")
    assert "pin=1234" in result.output
