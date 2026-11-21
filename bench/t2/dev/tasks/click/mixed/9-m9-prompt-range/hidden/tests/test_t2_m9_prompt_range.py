import click
from click.testing import CliRunner


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


def test_the_range_is_shown():
    result = ask("3\n", text="Count", type=click.IntRange(1, 5))
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Count (1<=x<=5): ")
    assert "value=3" in result.output
    assert ask("0.5\n", text="Ratio", type=click.FloatRange(0, 1, max_open=True)).output.startswith("Ratio (0<=x<1): ")
    assert ask("7\n", text="Min", type=click.IntRange(min=2)).output.startswith("Min (x>=2): ")


def test_with_a_default_and_turned_off():
    assert ask("\n", text="Count", type=click.IntRange(1, 5), default=2).output.startswith("Count (1<=x<=5) [2]: ")
    assert ask("3\n", text="Count", type=click.IntRange(1, 5), show_range=False).output.startswith("Count: ")
    assert ask("3\n", text="Count", type=int).output.startswith("Count: ")


def test_options_that_prompt():
    @click.command()
    @click.option("--count", type=click.IntRange(1, 5), prompt=True)
    def cli(count):
        click.echo(f"count={count}")

    result = CliRunner().invoke(cli, [], input="9\n4\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Count (1<=x<=5): ")
    assert "count=4" in result.output
