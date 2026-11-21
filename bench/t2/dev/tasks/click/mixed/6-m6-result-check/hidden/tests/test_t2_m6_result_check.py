import click
import pytest
from click.testing import CliRunner


@click.command()
@click.option("--fail", type=int, default=0)
def cli(fail):
    for i in range(30):
        click.echo(f"line {i}")
    if fail == 1:
        raise RuntimeError("boom")
    if fail:
        raise SystemExit(fail)


def test_check_returns_the_result():
    result = CliRunner().invoke(cli, [])
    assert result.check() is result
    assert CliRunner().invoke(cli, ["--fail", "3"]).check(3).exit_code == 3


def test_check_names_codes_and_output():
    result = CliRunner().invoke(cli, ["--fail", "2"])
    with pytest.raises(AssertionError) as e:
        result.check()
    text = str(e.value)
    assert text.startswith("exit code 2, expected 0")
    assert "exception:" not in text
    assert "\noutput:\n" in text
    assert "line 29" in text and "line 10" in text and "line 9\n" not in text


def test_check_names_the_exception():
    result = CliRunner().invoke(cli, ["--fail", "1"])
    with pytest.raises(AssertionError) as e:
        result.check()
    assert str(e.value).startswith("exit code 1, expected 0\nexception: RuntimeError('boom')")


def test_invoke_check():
    assert CliRunner().invoke(cli, [], check=True).exit_code == 0
    with pytest.raises(AssertionError):
        CliRunner().invoke(cli, ["--fail", "2"], check=True)
