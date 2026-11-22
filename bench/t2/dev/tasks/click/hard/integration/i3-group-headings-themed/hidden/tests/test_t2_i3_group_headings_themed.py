import click
from click.testing import CliRunner


def test_group_sections_get_the_heading_style():
    theme = click.HelpTheme(heading={"bold": True}, option={"fg": "cyan"})

    @click.command(context_settings={"help_theme": theme})
    @click.option("--host", help="Host.", help_group="Network")
    def serve(host):
        pass

    out = CliRunner().invoke(serve, ["--help"], color=True).output
    assert "\n" + click.style("Network:", bold=True) + "\n" in out
    assert click.style("--host TEXT", fg="cyan") in out
    assert "\n" + click.style("Options:", bold=True) + "\n" in out
