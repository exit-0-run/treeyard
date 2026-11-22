import click


class UnitOption(click.Option):
    help_extra_keys = (*click.Option.help_extra_keys, "unit")

    def __init__(self, *args, unit=None, **kwargs):
        super().__init__(*args, **kwargs)
        self.unit = unit

    def get_help_extra_unit(self, ctx):
        return self.unit


def test_markdown_details_hold_custom_items():
    @click.command()
    @click.option("--timeout", cls=UnitOption, unit="s", default=30, show_default=True, help="Wait.")
    def cli(timeout):
        pass

    with click.Context(cli, info_name="cli") as ctx:
        md = cli.get_help_markdown(ctx)
    assert "| `--timeout INTEGER` | Wait. | default: 30; unit: s |" in md
