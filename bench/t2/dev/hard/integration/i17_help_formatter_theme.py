import click
import click._help_formatter as module


def test_the_moved_formatter_takes_a_theme():
    assert module.HelpFormatter().theme == click.HelpTheme()
    f = module.HelpFormatter(width=60, theme=click.HelpTheme(heading={"bold": True}))
    f.write_heading("Options")
    assert f.getvalue() == click.style("Options:", bold=True) + "\n"
    assert click.formatting.HelpFormatter is module.HelpFormatter
