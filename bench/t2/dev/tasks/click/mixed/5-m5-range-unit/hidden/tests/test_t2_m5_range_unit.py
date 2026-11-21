import click
import pytest


def test_the_unit_ends_the_description():
    assert repr(click.IntRange(1, 5, unit="s")) == "<IntRange 1<=x<=5 s>"
    assert repr(click.FloatRange(0, 1, max_open=True, unit="GB")) == "<FloatRange 0<=x<1 GB>"
    assert repr(click.IntRange(1, 5)) == "<IntRange 1<=x<=5>"
    with pytest.raises(click.BadParameter) as e:
        click.IntRange(1, 5, unit="s").convert("7", None, None)
    assert "7 is not in the range 1<=x<=5 s." in str(e.value)


def test_the_help_item_has_a_label():
    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--wait"], type=click.IntRange(1, 5, unit="s"), help="Wait.")
    assert opt.get_help_extra(ctx) == {"range": "1<=x<=5 s"}
    assert opt.get_help_record(ctx)[1] == "Wait.  [range: 1<=x<=5 s]"
    opt = click.Option(["--n"], type=click.IntRange(0, 3), default=1, show_default=True, required=True)
    assert opt.get_help_record(ctx)[1] == "[default: 1; range: 0<=x<=3; required]"


def test_clamped_float_range_keeps_working():
    t = click.FloatRange(0, 10, clamp=True, unit="%")
    assert t.convert("12", None, None) == 10
    assert t.to_info_dict()["clamp"] is True
