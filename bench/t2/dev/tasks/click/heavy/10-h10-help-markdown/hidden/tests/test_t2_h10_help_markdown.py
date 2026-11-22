import click


@click.group()
def cli():
    """The tool.

    It does things."""


@cli.command(epilog="See the docs.")
@click.option("--count", default=1, show_default=True, required=True, help="How | many.")
@click.option("--name", help=click.style("The name.", bold=True))
@click.option("--quiet", is_flag=True)
@click.argument("target", required=False)
def run(count, name, quiet, target):
    """Run the thing."""


def markdown(cmd, *names):
    with click.Context(cli, info_name="cli") as ctx:
        if cmd is cli:
            return cli.get_help_markdown(ctx)
        with click.Context(cmd, info_name=names[0], parent=ctx) as sub:
            return cmd.get_help_markdown(sub)


def test_a_command_page():
    md = markdown(run, "run")
    assert md == (
        "# cli run\n\n"
        "Usage: `cli run [OPTIONS] [TARGET]`\n\n"
        "Run the thing.\n\n"
        "## Options\n\n"
        "| Name | Description | Details |\n"
        "| --- | --- | --- |\n"
        "| `--count INTEGER` | How \\| many. | default: 1; required |\n"
        "| `--name TEXT` | The name. |  |\n"
        "| `--quiet` |  |  |\n"
        "| `--help` | Show this message and exit. |  |\n\n"
        "See the docs.\n"
    )


def test_a_group_page():
    md = markdown(cli)
    assert md.startswith("# cli\n\nUsage: `cli [OPTIONS] COMMAND [ARGS]...`\n\nThe tool.\n\nIt does things.\n\n## Options\n")
    assert "## Commands\n\n| Name | Description | Details |\n| --- | --- | --- |\n| `run` | Run the thing. |  |\n" in md
    assert md.index("## Options") < md.index("## Commands")
    assert "\x1b" not in md


def test_no_style_codes_and_no_wrapping():
    @click.command(help="x " * 100)
    @click.option("--long", help="word " * 60)
    def wide(long):
        pass

    with click.Context(wide, info_name="wide") as ctx:
        md = wide.get_help_markdown(ctx)
    assert ("| `--long TEXT` | " + ("word " * 60).strip() + " |  |\n") in md
    assert ("x " * 100).strip() + "\n" in md


def test_the_formatter():
    f = click.MarkdownHelpFormatter()
    assert isinstance(f, click.HelpFormatter)
    f.write_title("tool")
    f.write_usage("tool", "[OPTIONS]")
    with f.section("Extra"):
        f.write_dl([("--a", "Alpha.  [default: 1]"), ("--b", "[required]")])
    assert f.getvalue() == (
        "# tool\n\nUsage: `tool [OPTIONS]`\n\n## Extra\n\n| Name | Description | Details |\n| --- | --- | --- |\n"
        "| `--a` | Alpha. | default: 1 |\n| `--b` |  | required |\n"
    )
    assert f.split_help("Text.  [a; b]") == ("Text.", "a; b")
    assert f.split_help("Text.") == ("Text.", "")


def test_custom_sections_come_along():
    class WithNotes(click.Command):
        def format_epilog(self, ctx, formatter):
            with formatter.section("Notes"):
                formatter.write_dl([("A", "first note")])

    @click.command(cls=WithNotes)
    def noted():
        pass

    with click.Context(noted, info_name="noted") as ctx:
        md = noted.get_help_markdown(ctx)
    assert md.endswith("## Notes\n\n| Name | Description | Details |\n| --- | --- | --- |\n| `A` | first note |  |\n")
