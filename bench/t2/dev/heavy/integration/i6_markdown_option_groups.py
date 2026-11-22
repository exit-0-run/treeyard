import click


def test_markdown_has_the_group_sections():
    @click.command()
    @click.option("--verbose", is_flag=True, help="Say more.")
    @click.option("--host", help="Host.", help_group="Network")
    def serve(verbose, host):
        pass

    with click.Context(serve, info_name="serve") as ctx:
        md = serve.get_help_markdown(ctx)
    assert "## Network\n\n| Name | Description | Details |\n| --- | --- | --- |\n| `--host TEXT` | Host. |  |\n" in md
    assert md.index("## Options") < md.index("## Network")
    assert "`--host TEXT`" not in md[: md.index("## Network")]
