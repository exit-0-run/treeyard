import inspect

import click
import pytest
from click._prompter import Prompter as ModulePrompter
from click.testing import CliRunner


def run(func, answer):
    @click.command()
    def cli():
        click.echo(f"value={func()!r}")

    return CliRunner().invoke(cli, input=answer)


def params(func):
    return [(p.name, p.default) for p in inspect.signature(func).parameters.values()]


def test_the_class_lives_in_the_new_module_and_is_exported():
    assert click.Prompter is ModulePrompter
    assert click.termui.Prompter is ModulePrompter


def test_the_parameters_are_those_of_prompt_and_confirm():
    assert params(click.Prompter) == params(click.prompt)
    assert params(click.Prompter.confirm) == params(click.confirm)
    p = click.Prompter("Port", 8, type=int, err=True)
    assert (p.text, p.default, p.type, p.err, p.hide_input, p.prompt_suffix) == ("Port", 8, int, True, False, ": ")


def test_ask_does_what_prompt_does():
    result = run(lambda: click.Prompter("Port", type=int).ask(), "x\n8\n")
    assert result.output == "Port: x\nError: 'x' is not a valid integer.\nPort: 8\nvalue=8\n"
    result = run(lambda: click.Prompter("Code", confirmation_prompt=True).ask(), "a\nb\na\na\n")
    assert "Error: The two entered values do not match." in result.output
    assert result.output.endswith("value='a'\n")
    assert run(lambda: click.Prompter.confirm("Go?", default=True), "\n").output == "Go? [Y/n]: \nvalue=True\n"
    assert run(lambda: click.Prompter.confirm("Go?"), "maybe\nn\n").output == (
        "Go? [y/N]: maybe\nError: invalid input\nGo? [y/N]: n\nvalue=False\n")


def test_prompt_and_confirm_go_through_the_class(monkeypatch):
    monkeypatch.setattr(ModulePrompter, "ask", lambda self: ("asked", self.text, self.default, self.type))
    assert click.prompt("Q", "d", type=int) == ("asked", "Q", "d", int)
    monkeypatch.setattr(ModulePrompter, "confirm", classmethod(lambda cls, *a, **k: ("confirmed", a, k)))
    assert click.confirm("Go?", True)[0] == "confirmed"


def test_every_answer_goes_through_read(monkeypatch):
    answers = iter(["x", "3", "3"])
    seen = []

    def read(self, text):
        seen.append(text)
        return next(answers)

    monkeypatch.setattr(ModulePrompter, "read", read)

    @click.command()
    def cli():
        click.echo(f"value={click.prompt('N', type=int, confirmation_prompt=True)!r}")

    result = CliRunner().invoke(cli)
    assert result.output == "Error: 'x' is not a valid integer.\nvalue=3\n"
    assert seen == ["N: ", "N: ", "Repeat for confirmation: "]


def test_read_uses_the_prompt_functions_of_termui(monkeypatch):
    monkeypatch.setattr(click.termui, "visible_prompt_func", lambda text: "seen")
    monkeypatch.setattr(click.termui, "hidden_prompt_func", lambda text: "hidden")
    assert click.Prompter("A").read("A: ") == "seen"
    assert click.Prompter("A", hide_input=True).read("A: ") == "hidden"

    def eof(text):
        raise EOFError

    monkeypatch.setattr(click.termui, "visible_prompt_func", eof)
    with pytest.raises(click.Abort):
        click.Prompter("A").read("A: ")


def test_the_old_helper_is_kept_and_readline_is_used(monkeypatch):
    from click.termui import _readline_prompt

    assert _readline_prompt(lambda text: text.upper() + "!", "q: ", False) == "Q: !"
    assert click.Prompter.readline(lambda text: text + "?", "q: ", False) == "q: ?"
    calls = []
    real = ModulePrompter.readline

    def spy(func, text, err):
        calls.append(text)
        return real(func, text, err)

    monkeypatch.setattr(ModulePrompter, "readline", staticmethod(spy))
    result = run(lambda: (click.prompt("Name"), click.confirm("Sure?")), "bob\ny\n")
    assert result.output.endswith("value=('bob', True)\n")
    assert calls == ["Name: ", "Sure? [y/N]: "]
    assert _readline_prompt(lambda text: "x", "q: ", False) == "x"
    assert calls[-1] == "q: "
