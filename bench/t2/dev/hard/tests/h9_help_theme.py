import click
from click.testing import CliRunner

THEME = click.HelpTheme(heading={"bold": True}, option={"fg": "cyan"}, usage={"fg": "green"}, extra={"dim": True})


def make_cli(theme=THEME):
    @click.group(context_settings={"help_theme": theme} if theme else {})
    def cli():
        pass

    @cli.command(help="Run it.")
    @click.option("--count", default=1, show_default=True, help="How many.")
    def run(count):
        pass

    return cli


def test_the_theme_object():
    assert THEME.heading == {"bold": True}
    assert click.HelpTheme().option is None
    assert THEME.apply("heading", "Options:") == click.style("Options:", bold=True)
    assert THEME.apply("option", "--x") == click.style("--x", fg="cyan")
    assert click.HelpTheme().apply("heading", "Options:") == "Options:"
    assert click.HelpTheme(heading={"bold": True}) == click.HelpTheme(heading={"bold": True})


def test_styled_help_page():
    out = CliRunner().invoke(make_cli(), ["run", "--help"], color=True).output
    assert out.startswith(click.style("Usage:", fg="green") + " cli run [OPTIONS]\n")
    assert "\n" + click.style("Options:", bold=True) + "\n" in out
    assert "  " + click.style("--count INTEGER", fg="cyan") + "  How many.  " + click.style("[default: 1]", dim=True) + "\n" in out
    assert "  " + click.style("--help", fg="cyan") + "           Show this message and exit.\n" in out


def test_groups_list_commands_with_the_theme():
    out = CliRunner().invoke(make_cli(), ["--help"], color=True).output
    assert click.style("Commands:", bold=True) in out
    assert "  " + click.style("run", fg="cyan") + "  Run it.\n" in out


def test_the_layout_does_not_change():
    styled = CliRunner().invoke(make_cli(), ["run", "--help"], color=True).output
    plain = CliRunner().invoke(make_cli(None), ["run", "--help"], color=True).output
    assert click.unstyle(styled) == plain
    assert CliRunner().invoke(make_cli(), ["run", "--help"]).output == plain


def test_the_formatter_takes_a_theme():
    f = click.HelpFormatter(width=40, theme=click.HelpTheme(heading={"underline": True}))
    f.write_heading("Things")
    assert f.getvalue() == click.style("Things:", underline=True) + "\n"
    assert isinstance(click.HelpFormatter().theme, click.HelpTheme)
    f = click.HelpFormatter(width=40, theme=click.HelpTheme(option={"bold": True}))
    f.write_dl([("-a", "one"), ("-bbb", "two")])
    lines = click.unstyle(f.getvalue()).splitlines()
    assert lines == ["-a    one", "-bbb  two"]


def test_the_context_inherits_the_theme():
    parent = click.Context(click.Command("a"), help_theme=THEME)
    child = click.Context(click.Command("b"), parent=parent)
    assert child.help_theme is THEME
    assert click.Context(click.Command("c")).help_theme is None
    assert child.make_formatter().theme is THEME


def test_custom_formatters_without_the_parameter_still_work():
    class Old(click.HelpFormatter):
        def __init__(self, indent_increment=2, width=None, max_width=None):
            super().__init__(indent_increment, width, max_width)

    class Ctx(click.Context):
        formatter_class = Old

    @click.command()
    def cli():
        pass

    cli.context_class = Ctx
    assert CliRunner().invoke(cli, ["--help"]).exit_code == 0
