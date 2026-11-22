import click
from click.testing import CliRunner


def ctx():
    return click.Context(click.Command("cli"))


def test_secret_str():
    s = click.SecretStr("hunter2")
    assert s.reveal() == "hunter2"
    assert str(s) == "***"
    assert repr(s) == "SecretStr('***')"
    assert f"token={s}" == "token=***"
    assert s == click.SecretStr("hunter2")
    assert s != click.SecretStr("other")
    assert len(s) == 7
    assert bool(s) and not bool(click.SecretStr(""))
    assert hash(s) == hash(click.SecretStr("hunter2"))


def test_the_secret_type():
    t = click.Secret()
    assert t.name == "secret"
    value = t.convert("abc", None, None)
    assert isinstance(value, click.SecretStr) and value.reveal() == "abc"
    assert t.convert(value, None, None) is value
    opt = click.Option(["--token"], type=click.Secret())
    assert opt.hide_input is True
    assert opt.get_help_record(ctx())[0] == "--token SECRET"


def test_a_secret_option_on_the_command_line():
    @click.command()
    @click.option("--token", type=click.Secret(), envvar="TOKEN")
    def cli(token):
        click.echo(f"{token} {token.reveal()}")

    assert CliRunner().invoke(cli, ["--token", "abc"]).output == "*** abc\n"
    assert CliRunner().invoke(cli, [], env={"TOKEN": "xyz"}).output == "*** xyz\n"


def test_help_hides_the_default_of_a_hidden_input():
    c = ctx()
    opt = click.Option(["--password"], default="hunter2", show_default=True, hide_input=True, help="Pw.")
    assert opt.get_help_extra(c) == {"default": "(hidden)"}
    assert opt.get_help_record(c)[1] == "Pw.  [default: (hidden)]"
    secret = click.Option(["--token"], type=click.Secret(), default="abc", show_default=True)
    assert secret.get_help_extra(c) == {"default": "(hidden)"}
    named = click.Option(["--k"], default="x", show_default="from the keyring", hide_input=True)
    assert named.get_help_extra(c) == {"default": "(from the keyring)"}
    assert click.Option(["--user"], default="alice", show_default=True).get_help_extra(c) == {"default": "alice"}
    assert click.Option(["--p"], hide_input=True, show_default=True).get_help_extra(c) == {}


def test_info_dict_hides_the_default():
    assert click.Option(["--p"], default="hunter2", hide_input=True).to_info_dict()["default"] == "(hidden)"
    assert click.Option(["--p"], hide_input=True).to_info_dict()["default"] is None
    assert click.Option(["--u"], default="alice").to_info_dict()["default"] == "alice"


def test_prompts_hide_the_default():
    @click.command()
    def cli():
        click.echo(f"value={click.prompt('Password', default='hunter2', hide_input=True)}")

    result = CliRunner().invoke(cli, input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Password [(hidden)]: ")
    assert result.output.count("hunter2") == 1

    @click.command()
    @click.option("--token", type=click.Secret(), default="s3cret", prompt=True)
    def cli2(token):
        click.echo(f"token={token.reveal()}")

    result = CliRunner().invoke(cli2, [], input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Token [(hidden)]: ")
    assert result.output.count("s3cret") == 1


def test_a_prompt_with_a_secret_type_hides_the_input():
    @click.command()
    def cli():
        value = click.prompt("Key", type=click.Secret())
        click.echo(f"got {value.reveal()}")

    result = CliRunner(echo_stdin=True).invoke(cli, input="topsecret\n")
    assert result.exit_code == 0, result.output
    assert result.output.count("topsecret") == 1


def test_errors_do_not_show_a_hidden_value():
    @click.command()
    @click.option("--pin", type=int, hide_input=True)
    def cli(pin):
        pass

    result = CliRunner().invoke(cli, ["--pin", "abc9"])
    assert result.exit_code == 2
    assert "abc9" not in result.output
    assert "'***' is not a valid integer." in result.output

    @click.command()
    @click.option("--n", type=int)
    def plain(n):
        pass

    assert "'abc9' is not a valid integer." in CliRunner().invoke(plain, ["--n", "abc9"]).output
