import click
from click._prompt import PromptText as ModulePromptText
from click.testing import CliRunner


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


def test_the_class_lives_in_the_new_module_and_is_exported():
    assert click.PromptText is ModulePromptText
    assert click.termui.PromptText is ModulePromptText


def test_render_builds_the_parts_in_order():
    assert click.PromptText("Name").render() == "Name: "
    assert click.PromptText("Name", ": ", True, "alice").render() == "Name [alice]: "
    assert click.PromptText("Name", "> ", True, None).render() == "Name> "
    assert click.PromptText("Name", ": ", "from the env", "x").render() == "Name [(from the env)]: "
    size = click.Choice(["s", "m"])
    assert click.PromptText("Size", ": ", True, "m", True, size).render() == "Size (s, m) [m]: "
    assert click.PromptText("Size", ": ", True, "m", False, size).render() == "Size [m]: "
    assert str(click.PromptText("Size", ": ", False, "m", True, size)) == "Size (s, m): "


def test_the_parts():
    text = click.PromptText("Size", ": ", True, "m", True, click.Choice(["s", "m"]))
    assert text.choices_part() == " (s, m)"
    assert text.default_part() == " [m]"
    assert click.PromptText("Size").choices_part() == ""
    assert click.PromptText("Size").default_part() == ""
    assert click.PromptText("N", ": ", True, 3, True, click.INT).choices_part() == ""


def test_for_confirm():
    assert click.PromptText.for_confirm("Go?").render() == "Go? [y/N]: "
    assert click.PromptText.for_confirm("Go?", True).render() == "Go? [Y/n]: "
    assert click.PromptText.for_confirm("Go?", None).render() == "Go? [y/n]: "
    assert click.PromptText.for_confirm("Go?", True, "? ", False).render() == "Go?? "


def test_the_old_function_still_works():
    from click.termui import _build_prompt

    assert _build_prompt("Name", ": ", True, "alice") == "Name [alice]: "
    assert _build_prompt("Size", ": ", True, "m", True, click.Choice(["s", "m"])) == "Size (s, m) [m]: "
    assert _build_prompt("Again", ": ") == "Again: "


def test_prompt_and_confirm_text_is_unchanged():
    result = ask("\n", text="Size", type=click.Choice(["s", "m"]), default="m")
    assert result.output.startswith("Size (s, m) [m]: ")
    assert ask("bob\n", text="Name").output.startswith("Name: ")
    assert ask("\n", text="Name", default="x", show_default="env").output.startswith("Name [(env)]: ")

    @click.command()
    def cli():
        click.echo(click.confirm("Continue?", default=True))

    assert CliRunner().invoke(cli, input="\n").output == "Continue? [Y/n]: \nTrue\n"


def test_prompt_and_confirm_go_through_the_class(monkeypatch):
    monkeypatch.setattr(ModulePromptText, "choices_part", lambda self: " <any>")
    result = ask("x\n", text="Pick", default="x")
    assert result.output.startswith("Pick <any> [x]: ")

    @click.command()
    def cli():
        click.confirm("Go?")

    assert CliRunner().invoke(cli, input="y\n").output.startswith("Go? <any> [y/N]: ")


def test_the_confirmation_prompt_goes_through_the_class(monkeypatch):
    monkeypatch.setattr(ModulePromptText, "default_part", lambda self: " {d}")
    result = ask("a\na\n", text="Code", confirmation_prompt=True)
    assert result.exit_code == 0, result.output
    assert "Code {d}: " in result.output
    assert "Repeat for confirmation {d}: " in result.output
