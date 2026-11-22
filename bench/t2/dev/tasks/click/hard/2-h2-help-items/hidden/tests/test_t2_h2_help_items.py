import click
from click.formatting import append_help_extra
from click.testing import CliRunner


def ctx(**kwargs):
    return click.Context(click.Command("cli"), **kwargs)


def test_the_keys_and_their_order():
    assert click.Option.help_extra_keys == ("envvars", "default", "range", "required")


def test_each_item_has_its_method():
    c = ctx(auto_envvar_prefix="APP")
    opt = click.Option(["--count"], type=click.IntRange(1, 5), default=2, show_default=True,
                       required=True, show_envvar=True)
    assert opt.get_help_extra_envvars(c) == ("APP_COUNT",)
    assert opt.get_help_extra_default(c) == "2"
    assert opt.get_help_extra_range(c) == "1<=x<=5"
    assert opt.get_help_extra_required(c) == "required"
    plain = click.Option(["--name"])
    assert plain.get_help_extra_envvars(c) is None
    assert plain.get_help_extra_default(c) is None
    assert plain.get_help_extra_range(c) is None
    assert plain.get_help_extra_required(c) is None
    assert click.Option(["--n"], envvar=["A", "B"], show_envvar=True).get_help_extra_envvars(c) == ("A", "B")


def test_get_help_extra_and_the_help_line_stay_the_same():
    c = ctx()
    opt = click.Option(["--count"], type=click.IntRange(1, 5), default=2, show_default=True,
                       required=True, envvar="N", show_envvar=True, help="Count.")
    assert opt.get_help_extra(c) == {"envvars": ("N",), "default": "2", "range": "1<=x<=5", "required": "required"}
    assert opt.get_help_record(c) == ("--count INTEGER RANGE", "Count.  [env var: N; default: 2; 1<=x<=5; required]")
    assert click.Option(["--name"], help="Name.").get_help_record(c) == ("--name TEXT", "Name.")
    assert click.Option(["--name"], required=True).get_help_record(c) == ("--name TEXT", "[required]")
    flag = click.Option(["--shout/--no-shout"], default=True, show_default=True)
    assert flag.get_help_extra(c) == {"default": "shout"}
    assert click.Option(["--x"], default=lambda: 1, show_default=True).get_help_extra(c) == {"default": "(dynamic)"}


def test_format_help_extra():
    c = ctx()
    opt = click.Option(["--n"])
    extra = {"required": "required", "default": "1", "envvars": ("A", "B"), "other": "x"}
    assert opt.format_help_extra(c, extra) == ["env var: A, B", "default: 1", "required"]
    assert opt.format_help_extra_item("envvars", ("A",)) == "env var: A"
    assert opt.format_help_extra_item("default", "1") == "default: 1"
    assert opt.format_help_extra_item("range", "x>=0") == "x>=0"
    assert opt.format_help_extra_item("required", "required") == "required"
    assert opt.format_help_extra_item("unit", "s") == "unit: s"
    assert opt.format_help_extra_item("tags", ("a", "b")) == "tags: a, b"


class UnitOption(click.Option):
    help_extra_keys = (*click.Option.help_extra_keys, "unit")

    def __init__(self, *args, unit=None, **kwargs):
        super().__init__(*args, **kwargs)
        self.unit = unit

    def get_help_extra_unit(self, ctx):
        return self.unit


def test_a_subclass_adds_an_item():
    c = ctx()
    opt = UnitOption(["--timeout"], unit="s", default=30, show_default=True, help="Wait.")
    assert opt.get_help_extra(c) == {"default": "30", "unit": "s"}
    assert opt.get_help_record(c)[1] == "Wait.  [default: 30; unit: s]"
    assert UnitOption(["--timeout"]).get_help_record(c)[1] == ""


def test_a_subclass_drops_an_item_and_the_help_page_shows_it():
    class NoDefault(click.Option):
        def get_help_extra_default(self, ctx):
            return None

    @click.command()
    @click.option("--a", cls=NoDefault, default=1, show_default=True, required=True, help="A.")
    @click.option("--b", cls=UnitOption, unit="ms", help="B.")
    def cli(a, b):
        pass

    out = CliRunner().invoke(cli, ["--help"]).output
    assert "A.  [required]\n" in out
    assert "B.  [unit: ms]\n" in out


def test_append_help_extra():
    assert append_help_extra("Name.", ["default: x", "required"]) == "Name.  [default: x; required]"
    assert append_help_extra("", ["required"]) == "[required]"
    assert append_help_extra("Name.", []) == "Name."
    assert append_help_extra("", []) == ""
