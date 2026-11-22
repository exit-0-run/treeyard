import click


def test_markdown_details_show_sizes():
    @click.command()
    @click.option("--cache", type=click.ByteSize(max="1GiB"), default=10 * 1024**2, show_default=True, help="Cache.")
    def serve(cache):
        pass

    with click.Context(serve, info_name="serve") as ctx:
        md = serve.get_help_markdown(ctx)
    assert "| `--cache SIZE` | Cache. | default: 10 MiB; x<=1 GiB |" in md
