import click
from click.testing import CliRunner


def help_of(cli, args=("--help",)):
    result = CliRunner().invoke(cli, list(args))
    assert result.exit_code == 0, result.output
    return result.output


def test_groups_become_sections_after_options():
    @click.command()
    @click.option("--verbose", is_flag=True, help="Say more.")
    @click.option("--host", help="Server host.", help_group="Network")
    @click.option("--color", is_flag=True, help="Use colors.", help_group="Output")
    @click.option("--port", type=int, help="Server port.", help_group="Network")
    def serve(verbose, host, color, port):
        pass

    out = help_of(serve)
    assert out == (
        "Usage: serve [OPTIONS]\n\n"
        "Options:\n"
        "  --verbose       Say more.\n"
        "  --help          Show this message and exit.\n\n"
        "Network:\n"
        "  --host TEXT     Server host.\n"
        "  --port INTEGER  Server port.\n\n"
        "Output:\n"
        "  --color         Use colors.\n"
    )


def test_the_order_of_the_command_comes_first():
    @click.command(help_group_order=["Output", "Network"])
    @click.option("--host", help="Host.", help_group="Network")
    @click.option("--debug", help="Debug.", help_group="Debug")
    @click.option("--color", help="Color.", help_group="Output")
    def serve(host, debug, color):
        pass

    out = help_of(serve)
    assert out.index("Output:") < out.index("Network:") < out.index("Debug:")
    assert out.index("Options:") < out.index("Output:")
    assert serve.help_group_order == ("Output", "Network")


def test_columns_line_up_across_sections():
    @click.command()
    @click.option("--a", help="First.")
    @click.option("--a-much-longer-name", help="Second.", help_group="More")
    def cli(a, a_much_longer_name):
        pass

    lines = help_of(cli).splitlines()
    first = next(line for line in lines if "First." in line)
    second = next(line for line in lines if "Second." in line)
    assert first.index("First.") == second.index("Second.")


def test_hidden_options_and_empty_groups():
    @click.command(add_help_option=False)
    @click.option("--secret", hidden=True, help_group="Hidden")
    @click.option("--x", help="X.", help_group="Shown")
    def cli(secret, x):
        pass

    out = cli.get_help(click.Context(cli))
    assert "Hidden" not in out
    assert "Options:" not in out
    assert "Shown:\n  --x TEXT  X." in out


def test_the_option_attribute_and_get_help_groups():
    opt = click.Option(["--host"], help_group="Network")
    assert opt.help_group == "Network"
    assert click.Option(["--host"]).help_group is None

    @click.command()
    @click.option("--host", help="Host.", help_group="Network")
    @click.argument("name")
    def cli(host, name):
        pass

    groups = cli.get_help_groups(click.Context(cli))
    assert list(groups) == [None, "Network"]
    assert groups["Network"] == [("--host TEXT", "Host.")]
    assert [row[0] for row in groups[None]] == ["--help"]


def test_write_dl_with_a_first_column_width():
    f = click.HelpFormatter(width=60)
    f.write_dl([("-a", "one")], first_col_width=10)
    assert f.getvalue() == "-a          one\n"
    f = click.HelpFormatter(width=60)
    f.write_dl([("-a", "one")], first_col_width=50)
    assert f.getvalue() == "-a" + " " * 30 + "one\n"
    f = click.HelpFormatter(width=60)
    f.write_dl([("-a", "one")])
    assert f.getvalue() == "-a  one\n"


def test_command_groups_of_a_group():
    @click.group(help_group_order=["Admin"])
    def cli():
        pass

    @cli.command(help="Run it.")
    def run():
        pass

    @cli.command(help="Back up.", help_group="Data")
    def backup():
        pass

    @cli.command(help="Migrate the database.", help_group="Admin")
    def migrate():
        pass

    out = help_of(cli)
    assert "Commands:\n  run      Run it.\n" in out
    assert "Admin:\n  migrate  Migrate the database.\n" in out
    assert "Data:\n  backup   Back up.\n" in out
    assert out.index("Commands:") < out.index("Admin:") < out.index("Data:")
