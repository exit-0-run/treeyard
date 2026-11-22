import json

import click
from click.shell_completion import get_completion_class
from click.shell_completion import NuComplete
from click.shell_completion import shell_complete


@click.group()
def cli():
    pass


@cli.command(help="Deploy the app.")
@click.option("--region", type=click.Choice(["eu", "us"]), help="Where to.")
@click.option("--config", type=click.Path())
@click.option("--quiet", is_flag=True)
def deploy(region, config, quiet):
    pass


@cli.command()
def status():
    pass


def complete(monkeypatch, words):
    monkeypatch.setenv("COMP_WORDS", "\n".join(words))
    comp = NuComplete(cli, {}, "foo-bar", "_FOO_BAR_COMPLETE")
    return comp.complete()


def test_registered():
    assert get_completion_class("nu") is NuComplete
    assert NuComplete.name == "nu"


def test_source_defines_the_command(monkeypatch):
    monkeypatch.setattr("shutil.which", lambda name: None)
    comp = NuComplete(cli, {}, "foo-bar", "_FOO_BAR_COMPLETE")
    script = comp.source()
    assert 'def "_foo_bar_completion" [spans: list<string>]' in script
    assert "_FOO_BAR_COMPLETE" in script and "nu_complete" in script
    assert "COMP_WORDS" in script
    assert "^foo-bar" in script
    assert "from json" in script


def test_completion_args(monkeypatch):
    monkeypatch.setenv("COMP_WORDS", "foo-bar\ndeploy\n--re")
    comp = NuComplete(cli, {}, "foo-bar", "_FOO_BAR_COMPLETE")
    assert comp.get_completion_args() == (["deploy"], "--re")
    monkeypatch.setenv("COMP_WORDS", "foo-bar\n")
    assert comp.get_completion_args() == ([], "")
    monkeypatch.setenv("COMP_WORDS", "foo-bar\nmy file.txt\n")
    assert comp.get_completion_args() == (["my file.txt"], "")


def test_commands_and_descriptions(monkeypatch):
    out = json.loads(complete(monkeypatch, ["foo-bar", ""]))
    assert out == [{"value": "deploy", "description": "Deploy the app."}, {"value": "status"}]
    out = json.loads(complete(monkeypatch, ["foo-bar", "deploy", "--r"]))
    assert out == [{"value": "--region", "description": "Where to."}]
    out = json.loads(complete(monkeypatch, ["foo-bar", "deploy", "--region", ""]))
    assert out == [{"value": "eu"}, {"value": "us"}]
    assert json.loads(complete(monkeypatch, ["foo-bar", "nothing", "--zz"])) == []


def test_files_fall_back_to_nushell(monkeypatch):
    assert complete(monkeypatch, ["foo-bar", "deploy", "--config", ""]).strip() == "null"


def test_multi_line_help_is_one_line(monkeypatch):
    @click.command()
    @click.option("--long", help="First line.\n\nSecond line.")
    def one(long):
        pass

    monkeypatch.setenv("COMP_WORDS", "one\n--lo")
    out = json.loads(NuComplete(one, {}, "one", "_ONE_COMPLETE").complete())
    assert out == [{"value": "--long", "description": "First line. Second line."}]


def test_through_the_instruction(monkeypatch, capsys):
    monkeypatch.setenv("COMP_WORDS", "foo-bar\nst")
    assert shell_complete(cli, {}, "foo-bar", "_FOO_BAR_COMPLETE", "nu_complete") == 0
    assert json.loads(capsys.readouterr().out) == [{"value": "status"}]
