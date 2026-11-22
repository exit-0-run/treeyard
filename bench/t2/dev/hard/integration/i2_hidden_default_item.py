import click


def test_the_default_item_method_hides_a_hidden_default():
    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--token"], default="hunter2", show_default=True, hide_input=True, help="Token.")
    assert opt.get_help_extra_default(ctx) == "(hidden)"
    secret = click.Option(["--key"], type=click.Secret(), default="abc", show_default=True)
    assert secret.get_help_extra_default(ctx) == "(hidden)"
    assert opt.get_help_record(ctx)[1] == "Token.  [default: (hidden)]"
