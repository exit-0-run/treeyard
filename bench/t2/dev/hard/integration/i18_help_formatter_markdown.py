import click
import click._help_formatter as module


def test_the_markdown_formatter_is_a_moved_formatter():
    assert click.formatting.HelpFormatter is module.HelpFormatter
    assert issubclass(click.MarkdownHelpFormatter, module.HelpFormatter)

    @click.command()
    @click.option("--n", default=1, show_default=True, help="How many.")
    def cli(n):
        """Count."""

    ctx = click.Context(cli, info_name="cli")
    text = cli.get_help_markdown(ctx)
    assert text.startswith("# cli\n")
    assert "| `--n INTEGER` | How many. | default: 1 |" in text
