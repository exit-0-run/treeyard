import click
from click.testing import CliRunner


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


REGIONS = click.Choice(["eu", "us", "ap", "sa"])


def test_long_lists_are_cut():
    result = ask("sa\n", text="Region", type=REGIONS, max_choices=2)
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Region (eu, us, ...): ")
    assert "value='sa'" in result.output


def test_lists_that_fit_stay():
    assert ask("eu\n", text="Region", type=REGIONS, max_choices=4).output.startswith("Region (eu, us, ap, sa): ")
    assert ask("eu\n", text="Region", type=REGIONS).output.startswith("Region (eu, us, ap, sa): ")


def test_with_a_default():
    result = ask("\n", text="Region", type=REGIONS, max_choices=1, default="us")
    assert result.output.startswith("Region (eu, ...) [us]: ")
    assert "value='us'" in result.output
