import click


def test_multiple_and_the_labelled_range_together():
    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--wait"], multiple=True, show_multiple=True, type=click.IntRange(1, 3, unit="s"),
                       required=True, help="Waits.")
    assert opt.get_help_extra(ctx) == {"range": "1<=x<=3 s", "multiple": "multiple", "required": "required"}
    assert opt.get_help_record(ctx)[1] == "Waits.  [range: 1<=x<=3 s; multiple; required]"
