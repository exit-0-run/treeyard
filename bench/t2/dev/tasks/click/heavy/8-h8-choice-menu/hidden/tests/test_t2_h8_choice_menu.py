import enum

import click
from click.testing import CliRunner


class Color(enum.Enum):
    RED = 1
    GREEN = 2
    BLUE = 3


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


REGIONS = click.Choice(["eu", "us", "ap"])


def test_enum_choices_and_defaults_by_name():
    result = ask("RED\n", text="Color", type=click.Choice(Color))
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Color (RED, GREEN, BLUE): ")
    assert "value=<Color.RED: 1>" in result.output
    result = ask("\n", text="Color", type=click.Choice(Color), default=Color.GREEN)
    assert result.output.startswith("Color (RED, GREEN, BLUE) [GREEN]: ")
    assert ask("\n", text="Size", type=click.Choice(["s", "m"]), default="m").output.startswith("Size (s, m) [m]: ")


def test_format_choice():
    assert click.Choice(Color).format_choice(Color.BLUE) == "BLUE"
    assert REGIONS.format_choice("eu") == "eu"
    assert click.Choice([1, 2]).format_choice(2) == "2"


def test_the_menu():
    result = ask("2\n", text="Region", type=REGIONS, choice_menu=True)
    assert result.exit_code == 0, result.output
    assert result.output.startswith("  1) eu\n  2) us\n  3) ap\nRegion (1-3): ")
    assert "value='us'" in result.output
    assert "value='ap'" in ask("ap\n", text="Region", type=REGIONS, choice_menu=True).output


def test_the_menu_with_a_default_and_enums():
    result = ask("\n", text="Region", type=REGIONS, default="us", choice_menu=True)
    assert result.output.startswith("  1) eu\n  2) us\n  3) ap\nRegion (1-3) [2]: ")
    assert "value='us'" in result.output
    result = ask("3\n", text="Color", type=click.Choice(Color), choice_menu=True)
    assert result.output.startswith("  1) RED\n  2) GREEN\n  3) BLUE\nColor (1-3): ")
    assert "value=<Color.BLUE: 3>" in result.output


def test_wrong_numbers_and_numeric_choices():
    result = ask("9\n1\n", text="Region", type=REGIONS, choice_menu=True)
    assert result.exit_code == 0, result.output
    assert "Error: 9 is not a number of the menu (1-3).\n" in result.output
    assert result.output.count("  1) eu") == 1
    assert "value='eu'" in result.output
    numbers = click.Choice(["10", "20", "30"])
    assert "value='20'" in ask("20\n", text="N", type=numbers, choice_menu=True).output
    assert "value='20'" in ask("2\n", text="N", type=numbers, choice_menu=True).output


def test_without_the_menu_nothing_changes():
    result = ask("eu\n", text="Region", type=REGIONS)
    assert result.output.startswith("Region (eu, us, ap): ")
    assert "1)" not in result.output
    assert ask("3\n", text="N", type=int, choice_menu=True).output.startswith("N: ")


def test_options_that_prompt_with_a_menu():
    @click.command()
    @click.option("--region", type=REGIONS, prompt=True, prompt_menu=True)
    def deploy(region):
        click.echo(f"region={region}")

    result = CliRunner().invoke(deploy, [], input="3\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("  1) eu\n  2) us\n  3) ap\nRegion (1-3): ")
    assert "region=ap" in result.output
    assert click.Option(["--r"], type=REGIONS, prompt=True, prompt_menu=True).prompt_menu is True
    try:
        click.Option(["--n"], type=int, prompt=True, prompt_menu=True)
    except TypeError:
        pass
    else:
        raise AssertionError("prompt_menu without a Choice type must raise TypeError")
