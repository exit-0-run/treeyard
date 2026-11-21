import click
from click._compat import strip_ansi
from click._compat import term_len
from click.testing import CliRunner

URL = "https://example.com"


def test_style_writes_a_hyperlink():
    assert click.style("docs", link=URL) == "\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\\x1b[0m"
    red = click.style("docs", fg="red", link=URL)
    assert red.startswith("\x1b[31m\x1b]8;;https://example.com\x1b\\docs")
    assert click.style("x", fg="red") == "\x1b[31mx\x1b[0m"


def test_links_are_removed_like_styles():
    assert click.unstyle(click.style("docs", bold=True, link=URL)) == "docs"
    assert strip_ansi("\x1b]8;;u\x07text\x1b]8;;\x07") == "text"
    assert term_len(click.style("docs", fg="red", link=URL)) == 4


def test_echo_to_a_non_terminal_drops_the_link():
    @click.command()
    def cli():
        click.secho("docs", link=URL)

    result = CliRunner().invoke(cli)
    assert result.exit_code == 0, result.output
    assert result.output == "docs\n"


def test_help_columns_ignore_links():
    f = click.HelpFormatter(width=60)
    f.write_dl([(click.style("--a", link=URL), "first"), ("--bbb", "second")])
    lines = [strip_ansi(x) for x in f.getvalue().splitlines()]
    assert lines[0].index("first") == lines[1].index("second")
