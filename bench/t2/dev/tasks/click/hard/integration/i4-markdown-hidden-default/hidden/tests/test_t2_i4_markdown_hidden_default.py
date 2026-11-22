import click


def test_markdown_details_hide_the_default():
    @click.command()
    @click.option("--token", type=click.Secret(), default="s3cret", show_default=True, help="API token.")
    @click.option("--pin", default="1234", show_default=True, hide_input=True)
    def login(token, pin):
        pass

    with click.Context(login, info_name="login") as ctx:
        md = login.get_help_markdown(ctx)
    assert "| `--token SECRET` | API token. | default: (hidden) |" in md
    assert "| `--pin TEXT` |  | default: (hidden) |" in md
    assert "s3cret" not in md and "1234" not in md
