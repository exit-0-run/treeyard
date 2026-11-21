import datetime
from pathlib import Path

import click
import pytest
from click.testing import CliRunner

D = datetime.timedelta


@pytest.mark.parametrize(
    ("value", "expect"),
    [("90", D(seconds=90)), ("1.5", D(seconds=1.5)), ("2d", D(days=2)), ("1h30m", D(hours=1, minutes=30)),
     ("45s", D(seconds=45)), ("1d2h3m4s", D(days=1, hours=2, minutes=3, seconds=4)), (" 5M ", D(minutes=5)),
     (D(hours=3), D(hours=3))],
)
def test_converts(value, expect):
    assert click.Duration().convert(value, None, None) == expect


@pytest.mark.parametrize("value", ["", "abc", "1x", "5m1h", "-3", "1.5h"])
def test_rejects(value):
    with pytest.raises(click.BadParameter) as e:
        click.Duration().convert(value, None, None)
    assert f"{value!r} is not a valid duration." in str(e.value)


def test_name_and_guessing():
    assert click.Duration.name == "duration"
    assert isinstance(click.Option(["--wait"], default=D(minutes=5)).type, click.Duration)
    assert isinstance(click.Option(["--wait"], type=datetime.timedelta).type, click.Duration)


def test_on_the_command_line():
    @click.command()
    @click.option("--wait", type=click.Duration(), default="1m")
    def cli(wait):
        click.echo(wait.total_seconds())

    assert CliRunner().invoke(cli, ["--wait", "1h30m"]).output == "5400.0\n"
    assert CliRunner().invoke(cli, []).output == "60.0\n"
    bad = CliRunner().invoke(cli, ["--wait", "soon"])
    assert bad.exit_code == 2
    assert "'soon' is not a valid duration." in bad.output


def test_docs_list_it():
    types = Path("docs/api.md").read_text().split("## Types")[1].split("\n## ")[0]
    assert types.index(".. autoclass:: DateTime") < types.index(".. autoclass:: Duration")
