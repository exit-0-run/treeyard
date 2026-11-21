import click
from click.testing import CliRunner


def test_dots_in_an_argument_name_become_underscores():
    assert click.Argument(["src.path"]).name == "src_path"

    @click.command()
    @click.argument("src.path")
    def cli(src_path):
        click.echo(src_path)

    result = CliRunner().invoke(cli, ["a.txt"])
    assert result.exit_code == 0, result.output
    assert result.output == "a.txt\n"
