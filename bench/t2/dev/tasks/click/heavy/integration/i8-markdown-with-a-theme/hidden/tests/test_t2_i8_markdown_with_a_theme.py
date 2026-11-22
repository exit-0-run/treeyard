import click


def test_markdown_ignores_the_theme():
    theme = click.HelpTheme(heading={"bold": True}, option={"fg": "cyan"}, usage={"bold": True}, extra={"dim": True})

    @click.command(context_settings={"help_theme": theme})
    @click.option("--count", default=1, show_default=True, help="How many.")
    def run(count):
        pass

    with run.make_context("run", []) as ctx:
        assert ctx.help_theme is theme
        md = run.get_help_markdown(ctx)
    assert "\x1b" not in md
    assert "| `--count INTEGER` | How many. | default: 1 |" in md
    assert "## Options" in md
