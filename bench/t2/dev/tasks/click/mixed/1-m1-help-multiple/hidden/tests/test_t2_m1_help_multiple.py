import click
from click.testing import CliRunner


def record(**kwargs):
    opt = click.Option(["--tag"], help="A tag.", **kwargs)
    ctx = click.Context(click.Command("cli"))
    return opt.get_help_extra(ctx), opt.get_help_record(ctx)[1]


def test_multiple_is_shown_when_asked():
    extra, text = record(multiple=True, show_multiple=True)
    assert extra == {"multiple": "multiple"}
    assert text == "A tag.  [multiple]"


def test_multiple_sits_before_required():
    extra, text = record(multiple=True, show_multiple=True, default=("a", "b"), show_default=True, required=True)
    assert text == "A tag.  [default: a, b; multiple; required]"


def test_without_show_multiple_nothing_changes():
    assert record(multiple=True) == ({}, "A tag.")
    assert record(show_multiple=True) == ({}, "A tag.")


def test_help_page():
    @click.command()
    @click.option("--tag", multiple=True, show_multiple=True, help="A tag.")
    def cli(tag):
        pass

    result = CliRunner().invoke(cli, ["--help"])
    assert result.exit_code == 0, result.output
    assert "A tag.  [multiple]" in result.output
