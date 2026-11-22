import click
from click.testing import CliRunner


def test_a_secret_prompt_through_the_prompter_hides_its_input():
    @click.command()
    def cli():
        value = click.Prompter("Token", type=click.Secret()).ask()
        click.echo(f"{type(value).__name__} {value.reveal()}")

    result = CliRunner().invoke(cli, input="abc\n")
    assert result.exception is None, result.output
    assert result.output == "Token: \nSecretStr abc\n"


def test_a_hidden_default_through_the_prompter():
    @click.command()
    def cli():
        click.echo(click.Prompter("Password", default="hunter2", hide_input=True).ask())

    result = CliRunner().invoke(cli, input="\n")
    assert "hunter2" not in result.output.splitlines()[0]
    assert result.output.startswith("Password [(hidden)]: ")
