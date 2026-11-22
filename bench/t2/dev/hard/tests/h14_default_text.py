import io

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


def test_the_help_bracket_gets_the_default_from_it(monkeypatch):
    opt, ctx = make("--n", default=2, show_default=True)
    assert opt.get_help_extra(ctx)["default"] == "2"
    import click._default_text as module
    import click.core

    fake = lambda option, ctx: "<two>"  # noqa: E731
    monkeypatch.setattr(module, "default_help_text", fake)
    monkeypatch.setattr(click.core, "default_help_text", fake, raising=False)
    assert opt.get_help_extra(ctx)["default"] == "<two>"
    assert "[default: <two>]" in opt.get_help_record(ctx)[1]


def test_default_prompt_text():
    assert default_prompt_text("alice") == "alice"
    assert default_prompt_text(3, click.INT) == "3"
    f = io.StringIO()
    f.name = "notes.txt"
    assert default_prompt_text(f) == "notes.txt"


def test_prompts_show_the_default_through_it(monkeypatch):
    import click._default_text as module

    seen = []

    def fake(value, type=None):
        seen.append((value, type))
        return f"<{value}>"

    monkeypatch.setattr(module, "default_prompt_text", fake)
    for mod in ("click.termui", "click._prompt"):
        try:
            monkeypatch.setattr(mod + ".default_prompt_text", fake)
        except (AttributeError, ImportError):
            pass

    @click.command()
    def cli():
        click.echo(click.prompt("Count", default=4, type=int))

    result = CliRunner().invoke(cli, input="\n")
    assert result.output == "Count [<4>]: \n4\n"
    assert seen == [(4, click.INT)] or seen == [(4, int)]


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
