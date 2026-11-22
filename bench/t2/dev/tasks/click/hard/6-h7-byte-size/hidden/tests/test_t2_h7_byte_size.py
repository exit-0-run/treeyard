import click
import pytest
from click.testing import CliRunner


@pytest.mark.parametrize(
    ("value", "expect"),
    [("512", 512), ("0", 0), ("10KB", 10_000), ("10 kb", 10_000), ("2k", 2000), ("1.5MiB", 1_572_864),
     ("1.5 kib", 1536), ("3G", 3 * 10**9), ("1TiB", 1024**4), ("7b", 7), (" 4 MB ", 4_000_000), ("1.9B", 1),
     (2048, 2048)],
)
def test_converts(value, expect):
    assert click.ByteSize().convert(value, None, None) == expect


@pytest.mark.parametrize("value", ["", "abc", "10XB", "-5", "1.2.3", "MB", "5 M B"])
def test_rejects(value):
    with pytest.raises(click.BadParameter) as e:
        click.ByteSize().convert(value, None, None)
    assert f"{value!r} is not a valid size." in str(e.value)


def test_bounds_and_their_description():
    t = click.ByteSize(max="1GiB")
    assert t.max == 1024**3
    assert t.convert("512MiB", None, None) == 512 * 1024**2
    with pytest.raises(click.BadParameter) as e:
        t.convert("2GiB", None, None)
    assert "is not in the range x<=1 GiB." in str(e.value)
    assert repr(click.ByteSize(min=1024, max="10MiB")) == "<ByteSize 1 KiB<=x<=10 MiB>"
    assert repr(click.ByteSize(min="1.5KiB")) == "<ByteSize x>=1.5 KiB>"
    assert click.ByteSize(max=100, clamp=True).convert("1KB", None, None) == 100
    assert click.ByteSize.name == "size"


def test_format_value():
    t = click.ByteSize()
    assert t.format_value(512) == "512 B"
    assert t.format_value(1536) == "1.5 KiB"
    assert t.format_value(10 * 1024**2) == "10 MiB"
    assert t.format_value(1024**3) == "1 GiB"
    assert t.format_value(3 * 1024**4) == "3 TiB"
    assert t.format_value("10MB") == "10MB"
    assert click.INT.format_value(5) == "5"
    assert click.STRING.format_value("x") == "x"


def test_help_shows_sizes():
    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--cache"], type=click.ByteSize(max="1GiB"), default=10 * 1024**2, show_default=True,
                       help="Cache.")
    assert opt.get_help_extra(ctx) == {"default": "10 MiB", "range": "x<=1 GiB"}
    assert opt.get_help_record(ctx) == ("--cache SIZE", "Cache.  [default: 10 MiB; x<=1 GiB]")
    many = click.Option(["--part"], type=click.ByteSize(), multiple=True, default=[1024, 2048], show_default=True)
    assert many.get_help_extra(ctx) == {"default": "1 KiB, 2 KiB"}
    assert click.Option(["--any"], type=click.ByteSize()).get_help_extra(ctx) == {}
    assert click.Option(["--n"], default=3, show_default=True).get_help_extra(ctx) == {"default": "3"}


def test_custom_types_show_their_values():
    class Hex(click.ParamType):
        name = "hex"

        def convert(self, value, param, ctx):
            return int(value, 16) if isinstance(value, str) else value

        def format_value(self, value):
            return hex(value)

    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--mask"], type=Hex(), default=255, show_default=True)
    assert opt.get_help_extra(ctx) == {"default": "0xff"}

    @click.command()
    def cli():
        click.echo(click.prompt("Mask", type=Hex(), default=255))

    assert CliRunner().invoke(cli, input="\n").output.startswith("Mask [0xff]: ")


def test_prompts_show_sizes():
    @click.command()
    def cli():
        click.echo(click.prompt("Size", type=click.ByteSize(), default=10 * 1024**2))

    result = CliRunner().invoke(cli, input="\n")
    assert result.output == "Size [10 MiB]: \n10485760\n"
    result = CliRunner().invoke(cli, input="2KB\n")
    assert result.output.endswith("\n2000\n")


def test_on_the_command_line():
    @click.command()
    @click.option("--cache", type=click.ByteSize(max="1GiB"), default="64MiB")
    def cli(cache):
        click.echo(cache)

    assert CliRunner().invoke(cli, []).output == "67108864\n"
    assert CliRunner().invoke(cli, ["--cache", "1k"]).output == "1000\n"
    bad = CliRunner().invoke(cli, ["--cache", "lots"])
    assert bad.exit_code == 2
    assert "'lots' is not a valid size." in bad.output
