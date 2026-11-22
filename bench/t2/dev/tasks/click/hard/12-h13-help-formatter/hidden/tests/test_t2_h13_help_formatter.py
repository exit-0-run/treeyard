import click
import click._help_formatter as module
import click.formatting
from click.testing import CliRunner


def test_the_class_and_helpers_live_in_the_new_module():
    assert module.HelpFormatter.__module__ == "click._help_formatter"
    for name in ("wrap_text", "measure_table", "iter_rows"):
        assert getattr(module, name).__module__ == "click._help_formatter", name


def test_the_old_names_are_the_same_objects():
    assert click.HelpFormatter is module.HelpFormatter
    assert click.wrap_text is module.wrap_text
    for name in ("HelpFormatter", "wrap_text", "measure_table", "iter_rows"):
        assert getattr(click.formatting, name) is getattr(module, name), name
    assert click.formatting.join_options(["--x", "-x"]) == ("-x, --x", False)


def test_the_old_helpers_still_work():
    assert click.formatting.measure_table([("a", "bbb"), ("cccc", "d")]) == (4, 3)
    assert list(click.formatting.iter_rows([("a",), ("b", "c")], 2)) == [("a", ""), ("b", "c")]
    assert click.formatting.wrap_text("one two three", width=8) == "one two\nthree"


def test_forced_width_is_read_from_formatting(monkeypatch):
    monkeypatch.setattr(click.formatting, "FORCED_WIDTH", 61)
    assert module.HelpFormatter().width == 61
    assert click.HelpFormatter(width=40).width == 40


def test_commands_use_the_class_of_the_new_module(monkeypatch):
    calls = []
    real = module.HelpFormatter.write_heading

    def spy(self, heading):
        calls.append(heading)
        real(self, heading)

    monkeypatch.setattr(module.HelpFormatter, "write_heading", spy)

    @click.command()
    @click.option("--name", help="The name.")
    def cli(name):
        pass

    result = CliRunner().invoke(cli, ["--help"])
    assert "Options:" in result.output
    assert calls == ["Options"]


def test_help_pages_are_unchanged():
    @click.group()
    def cli():
        """The tool."""

    @cli.command()
    @click.option("--count", type=int, default=1, show_default=True, help="How many.")
    def run(count):
        """Run it."""

    result = CliRunner().invoke(cli, ["run", "--help"])
    assert result.output == (
        "Usage: cli run [OPTIONS]\n\n  Run it.\n\nOptions:\n"
        "  --count INTEGER  How many.  [default: 1]\n"
        "  --help           Show this message and exit.\n")
    result = CliRunner().invoke(cli, ["--help"])
    assert "Commands:\n  run  Run it.\n" in result.output
