import click
from click.testing import CliRunner


def test_help_hides_the_default_of_a_hidden_input():
    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--password"], default="hunter2", show_default=True, hide_input=True, prompt=True)
    assert opt.get_help_extra(ctx) == {"default": "(hidden)"}
    assert "hunter2" not in opt.get_help_record(ctx)[1]
    assert "[default: (hidden)]" in opt.get_help_record(ctx)[1]


def test_a_show_default_string_stays():
    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--password"], default="x", show_default="from the keyring", hide_input=True, prompt=True)
    assert opt.get_help_extra(ctx) == {"default": "(from the keyring)"}
    plain = click.Option(["--user"], default="alice", show_default=True)
    assert plain.get_help_extra(ctx) == {"default": "alice"}


def test_prompt_hides_the_default():
    @click.command()
    def cli():
        click.echo(f"value={click.prompt('Password', default='hunter2', hide_input=True)}")

    result = CliRunner().invoke(cli, input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Password [(hidden)]: ")
    assert "value=hunter2" in result.output
    assert result.output.count("hunter2") == 1


def test_option_prompt_hides_the_default():
    @click.command()
    @click.option("--token", default="s3cret", prompt=True, hide_input=True)
    def cli(token):
        click.echo(f"token={token}")

    result = CliRunner().invoke(cli, [], input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Token [(hidden)]: ")
    assert result.output.count("s3cret") == 1


def test_visible_prompt_keeps_the_default():
    @click.command()
    def cli():
        click.echo(click.prompt("Name", default="alice"))

    assert CliRunner().invoke(cli, input="\n").output.startswith("Name [alice]: ")
