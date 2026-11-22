import click
import pytest
from click.testing import CliRunner


def run(func, answer):
    @click.command()
    def cli():
        click.echo(f"value={func()!r}")

    return CliRunner().invoke(cli, input=answer)


def test_gives_up_after_the_last_attempt():
    result = run(lambda: click.prompt("Quantity", type=int, max_attempts=3), "many\nlots\nall\n")
    assert result.exit_code == 1
    assert isinstance(result.exception, SystemExit)
    assert result.output == (
        "Quantity: many\nError: 'many' is not a valid integer. (2 attempts left)\n"
        "Quantity: lots\nError: 'lots' is not a valid integer. (1 attempt left)\n"
        "Quantity: all\nError: 'all' is not a valid integer.\n"
        "Error: Gave up after 3 invalid answers.\n"
    )


def test_a_valid_answer_in_time():
    result = run(lambda: click.prompt("Quantity", type=int, max_attempts=2), "x\n4\n")
    assert result.exit_code == 0, result.output
    assert "(1 attempt left)" in result.output
    assert "value=4" in result.output


def test_without_a_limit_nothing_changes():
    result = run(lambda: click.prompt("Quantity", type=int), "x\ny\n5\n")
    assert result.exit_code == 0
    assert result.output.count("Error: ") == 2
    assert "left)" not in result.output


def test_the_exception():
    assert issubclass(click.TooManyAttempts, click.ClickException)
    e = click.TooManyAttempts(1, "boom")
    assert e.exit_code == 1
    assert e.attempts == 1 and e.last_error == "boom"
    assert e.format_message() == "Gave up after 1 invalid answer."
    assert click.TooManyAttempts(4).format_message() == "Gave up after 4 invalid answers."

    @click.command()
    def cli():
        try:
            click.prompt("N", type=int, max_attempts=1)
        except click.TooManyAttempts as exc:
            click.echo(f"caught {exc.attempts} {exc.last_error}")

    result = CliRunner().invoke(cli, input="z\n")
    assert result.output.endswith("caught 1 'z' is not a valid integer.\n")


def test_empty_answers_without_a_default_do_not_count():
    result = run(lambda: click.prompt("Name", max_attempts=1), "\n\nbob\n")
    assert result.exit_code == 0, result.output
    assert "value='bob'" in result.output


def test_a_mismatched_confirmation_counts():
    result = run(lambda: click.prompt("Code", confirmation_prompt=True, max_attempts=2), "a\nb\nc\nd\n")
    assert result.exit_code == 1
    assert "Error: The two entered values do not match. (1 attempt left)\n" in result.output
    assert result.output.endswith("Error: Gave up after 2 invalid answers.\n")


def test_confirm():
    result = run(lambda: click.confirm("Go?", max_attempts=2), "maybe\nperhaps\n")
    assert result.exit_code == 1
    assert "Error: invalid input (1 attempt left)\n" in result.output
    assert result.output.endswith("Error: invalid input\nError: Gave up after 2 invalid answers.\n")
    assert "value=True" in run(lambda: click.confirm("Go?", max_attempts=2), "x\ny\n").output


def test_bad_limits():
    with pytest.raises(ValueError):
        click.prompt("N", max_attempts=0)


def test_options_that_prompt():
    @click.command()
    @click.option("--quantity", type=int, prompt=True, prompt_attempts=2)
    @click.option("--sure", is_flag=True, prompt=True, prompt_attempts=1)
    def order(quantity, sure):
        click.echo(f"{quantity} {sure}")

    result = CliRunner().invoke(order, input="x\ny\n")
    assert result.exit_code == 1
    assert "(1 attempt left)" in result.output
    assert result.output.endswith("Error: Gave up after 2 invalid answers.\n")
    result = CliRunner().invoke(order, input="3\nno way\n")
    assert result.exit_code == 1
    assert result.output.endswith("Error: Gave up after 1 invalid answer.\n")
    assert CliRunner().invoke(order, input="3\ny\n").output.endswith("3 True\n")
    assert click.Option(["--q"], prompt=True, prompt_attempts=3).prompt_attempts == 3
    with pytest.raises(TypeError):
        click.Option(["--q"], prompt_attempts=3)
