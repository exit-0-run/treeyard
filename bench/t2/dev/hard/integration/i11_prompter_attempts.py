import click
from click.testing import CliRunner


def test_the_prompter_gives_up_after_max_attempts():
    @click.command()
    def cli():
        click.echo(click.Prompter("N", type=int, max_attempts=2).ask())

    result = CliRunner().invoke(cli, input="a\nb\n")
    assert result.exit_code == 1
    assert "Error: 'a' is not a valid integer. (1 attempt left)\n" in result.output
    assert "Error: 'b' is not a valid integer.\n" in result.output
    assert result.output.endswith("Error: Gave up after 2 invalid answers.\n")


def test_the_prompter_confirm_gives_up_after_max_attempts():
    @click.command()
    def cli():
        click.echo(click.Prompter.confirm("Go?", max_attempts=1))

    result = CliRunner().invoke(cli, input="x\n")
    assert result.exit_code == 1
    assert result.output.endswith("Error: Gave up after 1 invalid answer.\n")
    assert isinstance(click.TooManyAttempts(1), click.ClickException)
