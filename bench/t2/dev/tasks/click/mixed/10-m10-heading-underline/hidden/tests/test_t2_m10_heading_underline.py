import click
from click.testing import CliRunner


def test_headings_get_an_underline():
    f = click.HelpFormatter(heading_underline="-")
    f.write_heading("Options")
    assert f.getvalue() == "Options:\n--------\n"


def test_the_underline_keeps_the_indent_and_ignores_styles():
    f = click.HelpFormatter(heading_underline="=")
    f.indent()
    f.write_heading(click.style("Commands", bold=True))
    lines = f.getvalue().splitlines()
    assert lines[1] == "  " + "=" * 9


def test_without_it_nothing_changes():
    f = click.HelpFormatter()
    f.write_heading("Options")
    assert f.getvalue() == "Options:\n"


def test_a_command_uses_it():
    class Underlined(click.HelpFormatter):
        def __init__(self, **kwargs):
            super().__init__(heading_underline="~", **kwargs)

    class Ctx(click.Context):
        formatter_class = Underlined

    @click.command(context_settings={})
    @click.option("--name")
    def cli(name):
        pass

    cli.context_class = Ctx
    out = CliRunner().invoke(cli, ["--help"]).output
    assert "\nOptions:\n~~~~~~~~\n" in out
