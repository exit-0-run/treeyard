import io
import sys

import click
from click._default_text import default_help_text
from click._default_text import default_prompt_text
from click.testing import CliRunner


def make(*decls, ctx_show_default=None, **kwargs):
    opt = click.Option(list(decls), **kwargs)
    cmd = click.Command("cli", params=[opt])
    return opt, click.Context(cmd, show_default=ctx_show_default)


def test_default_help_text_follows_todays_rules():
    assert default_help_text(*make("--n", default=2, show_default=True)) == "2"
    assert default_help_text(*make("--n", default=2)) is None
    assert default_help_text(*make("--n", default=2, ctx_show_default=True)) == "2"
    assert default_help_text(*make("--n", default=2, show_default="two")) == "(two)"
    assert default_help_text(*make("--n", default=(1, 2), multiple=True, show_default=True)) == "1, 2"
    assert default_help_text(*make("--n", default="", show_default=True)) == '""'
    assert default_help_text(*make("--n", default=lambda: 3, show_default=True)) == "(dynamic)"
    assert default_help_text(*make("--shout/--no-shout", default=True, show_default=True)) == "shout"
    assert default_help_text(*make("--shout", is_flag=True, show_default=True)) is None


def patch_everywhere(monkeypatch, original, fake):
    """Replaces `original` wherever click holds it: as an attribute of any click module or of a class
    defined in one, under any name. So the test does not depend on how a solution imports it."""
    found = 0
    for name, mod in list(sys.modules.items()):
        if mod is None or not (name == "click" or name.startswith("click.")):
            continue
        for holder in [mod, *(v for v in vars(mod).values() if isinstance(v, type) and v.__module__ == name)]:
            for attr, value in list(vars(holder).items()):
                if value is original:
                    monkeypatch.setattr(holder, attr, fake)
                    found += 1
                elif isinstance(value, staticmethod) and value.__func__ is original:
                    monkeypatch.setattr(holder, attr, staticmethod(fake))
                    found += 1
    assert found


def test_the_help_bracket_gets_the_default_from_it(monkeypatch):
    opt, ctx = make("--n", default=2, show_default=True)
    assert opt.get_help_extra(ctx)["default"] == "2"
    patch_everywhere(monkeypatch, default_help_text, lambda option, ctx: "<two>")
    assert opt.get_help_extra(ctx)["default"] == "<two>"
    assert "[default: <two>]" in opt.get_help_record(ctx)[1]


def test_default_prompt_text():
    assert default_prompt_text("alice") == "alice"
    assert default_prompt_text(3, click.INT) == "3"
    f = io.StringIO()
    f.name = "notes.txt"
    assert default_prompt_text(f) == "notes.txt"


def test_prompts_show_the_default_through_it(monkeypatch):
    seen = []

    def fake(default, type=None):
        seen.append((default, type))
        return f"<{default}>"

    patch_everywhere(monkeypatch, default_prompt_text, fake)

    @click.command()
    def cli():
        click.echo(click.prompt("Count", default=4, type=int))

    result = CliRunner().invoke(cli, input="\n")
    assert result.output == "Count [<4>]: \n4\n"
    assert seen in ([(4, click.INT)], [(4, int)])


def test_the_old_helper_is_kept():
    from click.termui import _format_default

    assert _format_default("alice") == "alice"
    f = io.StringIO()
    f.name = "notes.txt"
    assert _format_default(f) == "notes.txt"


def test_help_and_prompts_are_unchanged():
    @click.command()
    @click.option("--n", default=3, show_default=True, help="Count.")
    def cli(n):
        click.echo(click.prompt("Name", default="ann"))

    assert "Count.  [default: 3]" in CliRunner().invoke(cli, ["--help"]).output
    assert CliRunner().invoke(cli, input="\n").output == "Name [ann]: \nann\n"
