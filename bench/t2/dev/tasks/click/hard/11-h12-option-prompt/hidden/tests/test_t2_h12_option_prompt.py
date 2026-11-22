import click
from click._option_prompt import OptionPrompt
from click.testing import CliRunner


def make(*decls, **kwargs):
    opt = click.Option(list(decls), **kwargs)
    cmd = click.Command("cli", params=[opt])
    return opt, click.Context(cmd)


def test_option_prompt_class_is_the_default():
    assert click.Option.prompt_class is OptionPrompt
    opt, ctx = make("--name", prompt=True)
    p = OptionPrompt(opt, ctx)
    assert p.option is opt and p.ctx is ctx


def test_get_default():
    opt, ctx = make("--name", prompt=True, default="bob")
    assert OptionPrompt(opt, ctx).get_default() == "bob"
    opt, ctx = make("--force/--no-force", prompt=True)
    assert OptionPrompt(opt, ctx).get_default() is False
    opt, ctx = make("--force/--no-force", prompt=True, default=None)
    assert OptionPrompt(opt, ctx).get_default() is None
    opt, ctx = make("--force", is_flag=True, prompt=True, default="yes")
    assert OptionPrompt(opt, ctx).get_default() is True


def test_the_keyword_arguments():
    opt, ctx = make("--size", prompt=True, type=click.Choice(["s", "m"]), show_choices=False,
                    confirmation_prompt=True, hide_input=True)
    kw = OptionPrompt(opt, ctx).prompt_kwargs(None)
    assert kw["default"] is None
    assert kw["type"] is opt.type
    assert (kw["hide_input"], kw["show_choices"], kw["confirmation_prompt"]) == (True, False, True)
    assert "show_default" not in kw
    assert kw["value_proc"]("m") == "m"
    opt, ctx = make("--n", prompt=True, type=int, show_default=False)
    kw = OptionPrompt(opt, ctx).prompt_kwargs(3)
    assert (kw["default"], kw["show_default"], kw["value_proc"]("7")) == (3, False, 7)
    opt, ctx = make("--force/--no-force", prompt=True)
    assert OptionPrompt(opt, ctx).confirm_kwargs(True)["default"] is True


def test_ask_uses_the_keyword_arguments():
    class Loud(OptionPrompt):
        def prompt_kwargs(self, default):
            return {**super().prompt_kwargs(default), "prompt_suffix": "!? "}

        def confirm_kwargs(self, default):
            return {**super().confirm_kwargs(default), "prompt_suffix": "?! "}

    class LoudOption(click.Option):
        prompt_class = Loud

    @click.command()
    @click.option("--name", prompt=True, cls=LoudOption)
    @click.option("--force/--no-force", prompt=True, cls=LoudOption)
    def cli(name, force):
        click.echo(f"{name} {force}")

    result = CliRunner().invoke(cli, input="bob\ny\n")
    assert result.output == "Name!? bob\nForce [y/N]?! y\nbob True\n"


def test_options_prompt_through_the_class(monkeypatch):
    monkeypatch.setattr(OptionPrompt, "get_default", lambda self: "patched")

    @click.command()
    @click.option("--name", prompt=True)
    def cli(name):
        click.echo(name)

    result = CliRunner().invoke(cli, input="\n")
    assert result.exception is None, result.output
    assert result.output == "Name [patched]: \npatched\n"


def test_the_old_method_still_works():
    opt, ctx = make("--n", prompt="Number", type=int, default=4)

    @click.command()
    def cli():
        click.echo(repr(opt.prompt_for_value(ctx)))

    assert CliRunner().invoke(cli, input="x\n\n").output == (
        "Number [4]: x\nError: 'x' is not a valid integer.\nNumber [4]: \n4\n")


def test_prompts_are_unchanged():
    @click.command()
    @click.option("--name", prompt="Your name", default="ann", show_default=False)
    @click.option("--force/--no-force", prompt=True, default=True)
    def cli(name, force):
        click.echo(f"{name} {force}")

    assert CliRunner().invoke(cli, input="\n\n").output == "Your name: \nForce [Y/n]: \nann True\n"
