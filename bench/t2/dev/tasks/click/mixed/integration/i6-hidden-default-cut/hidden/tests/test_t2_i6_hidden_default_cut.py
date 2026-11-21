import click
from click.testing import CliRunner


def test_a_hidden_default_with_a_cut_list():
    @click.command()
    def cli():
        click.echo(click.prompt("Pick", type=click.Choice(["a", "b", "c"]), default="b", hide_input=True, max_choices=1))

    result = CliRunner().invoke(cli, input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Pick (a, ...) [(hidden)]: ")
