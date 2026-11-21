"""The `mixed` development set: 10 tasks for 10 agents on click (dev/README.md, "Mixed set").

Each task is a small feature with edits in 2 to 6 places. The interactions are designed (table in
dev/README.md):

- Conflict cluster A, `Option.get_help_record` / `get_help_extra` in src/click/core.py: m1, m3, m5.
- Conflict cluster B, `_build_prompt` / `prompt` in src/click/termui.py: m2, m3, m7, m9.
- Undo trap: m1 (X) adds the bare help item `multiple`; the rule in m5 (Y), read alone, says the only
  bare item is `required`. Both hidden tests pass together.
- Same line: m2 and m7 both rewrite the choices line of `_build_prompt`; m3, m7 and m9 all edit the
  `_build_prompt(...)` call in `prompt`. Every edit must survive.
- Interface: m9 shows the range with `_describe_range()`, which m5 extends with a unit; m10 measures
  the heading with `term_len`, whose `strip_ansi` m4 extends to hyperlinks.
- Registry-like surfaces: `OptionHelpExtra` (m1), `prompt` / `_build_prompt` parameter lists (m3, m7,
  m9), `click/__init__.py` exports and `docs/api.md` (m8), `convert_type` (m8), a parametrised test
  table (m5).
- Control: m6 (`click.testing`), no shared file.

Edit ops are those of taskdefs.py (`replace`, `insert_before`, `insert_after`), applied one task after
the other (`t2.apply_seq`). An op may carry `alts`: the same edit written for a tree where another task
is already in. The combined tree tries the alts first, then the op itself; the reference patch of a
task alone uses only the op itself. `resolved-<k>.patch` is the step from tasks 1..k-1 to 1..k.
"""

SUFFIX = "\n\nKeep the existing behaviour otherwise. Finish the work completely.\n"

CORE = "src/click/core.py"
TYPES = "src/click/types.py"
TERMUI = "src/click/termui.py"
FORMATTING = "src/click/formatting.py"
COMPAT = "src/click/_compat.py"
INIT = "src/click/__init__.py"
API = "docs/api.md"
TESTING = "src/click/testing.py"

PROMPT_CALL = "    prompt = _build_prompt(\n        text, prompt_suffix, show_default, default, show_choices, type\n    )\n"
CHOICES_LINE = "        prompt += f\" ({', '.join(map(str, type.choices))})\"\n"
CHOICES_M2 = ("        names = [c.name if isinstance(c, enum.Enum) else str(c) for c in type.choices]\n"
              "        prompt += f\" ({', '.join(names)})\"\n")
BUILD_SIG_END = ") -> str:\n    prompt = text\n"
PROMPT_DOC_END = "\n    .. versionchanged:: 8.5.0\n        Generically typed: the return type is narrowed by ``type``,\n"


def call_with(kw):
    return ("    prompt = _build_prompt(\n        text,\n        prompt_suffix,\n        show_default,\n        default,\n"
            f"        show_choices,\n        type,\n        {kw},\n    )\n")


def overload_params(text):
    """The same parameter after `show_choices` in the three signatures of `prompt`."""
    return [{"op": "insert_before", "file": TERMUI, "anchor": a, "text": text}
            for a in ("\n) -> str: ...\n", "\n) -> V: ...\n", "\n) -> V:\n    \"\"\"Prompts a user for input.")]


CLICK_MIXED = [
    {
        "id": "m1-help-multiple",
        "title": "Option help: say that an option can be repeated",
        "prompt": (
            "An option declared with `multiple=True` can be given several times, but its help line does not say "
            "so. Add a parameter `show_multiple: bool = False` to `click.Option`, documented in its docstring next "
            "to `show_envvar`. When it is true and the option has `multiple=True`, the bracket at the end of the "
            "option's help line gets the item `multiple`, after the range and before `required`. Example: "
            "`@click.option(\"--tag\", multiple=True, show_multiple=True, help=\"A tag.\")` shows `A tag.  [multiple]` "
            "in `--help`; with `default=(\"a\", \"b\"), show_default=True, required=True` added it shows "
            "`[default: a, b; multiple; required]`. `Option.get_help_extra(ctx)` returns the item under the key "
            "`\"multiple\"` with the value `\"multiple\"`; declare the key in the `OptionHelpExtra` typed dict in "
            "`src/click/types.py`. Without `show_multiple` the help stays as it is."
        ),
        "edits": [
            {"op": "insert_after", "file": CORE, "anchor": "    show_choices: bool\n    show_envvar: bool\n",
             "text": "    show_multiple: bool\n"},
            {"op": "insert_after", "file": CORE, "anchor": "        show_envvar: bool = False,\n",
             "text": "        show_multiple: bool = False,\n"},
            {"op": "insert_before", "file": CORE, "anchor": "    :param prompt: If set to ``True`` or a non empty string then the\n",
             "text": ("    :param show_multiple: Show ``multiple`` in the help of an option\n"
                      "        that can be given several times (``multiple=True``).\n")},
            {"op": "insert_after", "file": CORE, "anchor": "        self.show_envvar = show_envvar\n",
             "text": "        self.show_multiple = show_multiple\n"},
            {"op": "insert_before", "file": CORE, "anchor": "        if self.required:\n            extra[\"required\"] = \"required\"\n",
             "text": "        if self.show_multiple and self.multiple:\n            extra[\"multiple\"] = \"multiple\"\n\n"},
            {"op": "insert_before", "file": CORE, "anchor": "        if \"required\" in extra:\n",
             "text": "        if \"multiple\" in extra:\n            extra_items.append(_(extra[\"multiple\"]))\n"},
            {"op": "insert_before", "file": TYPES, "anchor": "    range: str\n    required: str\n",
             "text": "    multiple: str\n"},
        ],
        "test": r'''import click
from click.testing import CliRunner


def record(**kwargs):
    opt = click.Option(["--tag"], help="A tag.", **kwargs)
    ctx = click.Context(click.Command("cli"))
    return opt.get_help_extra(ctx), opt.get_help_record(ctx)[1]


def test_multiple_is_shown_when_asked():
    extra, text = record(multiple=True, show_multiple=True)
    assert extra == {"multiple": "multiple"}
    assert text == "A tag.  [multiple]"


def test_multiple_sits_before_required():
    extra, text = record(multiple=True, show_multiple=True, default=("a", "b"), show_default=True, required=True)
    assert text == "A tag.  [default: a, b; multiple; required]"


def test_without_show_multiple_nothing_changes():
    assert record(multiple=True) == ({}, "A tag.")
    assert record(show_multiple=True) == ({}, "A tag.")


def test_help_page():
    @click.command()
    @click.option("--tag", multiple=True, show_multiple=True, help="A tag.")
    def cli(tag):
        pass

    result = CliRunner().invoke(cli, ["--help"])
    assert result.exit_code == 0, result.output
    assert "A tag.  [multiple]" in result.output
''',
    },
    {
        "id": "m2-prompt-enum-names",
        "title": "Prompts: show enum choices by name",
        "prompt": (
            "A `click.Choice` built from an enum accepts the member names on the command line (`RED`), but "
            "`click.prompt` lists the choices with `str()`, so a plain `enum.Enum` shows `(Color.RED, Color.GREEN)`, "
            "and a default member shows as `[Color.GREEN]`. In `src/click/termui.py`, show enum members by their "
            "name in both places: `click.prompt(\"Color\", type=click.Choice(Color), default=Color.GREEN)` shows "
            "`Color (RED, GREEN, BLUE) [GREEN]: `. Choices and defaults that are not enum members show as before."
        ),
        "edits": [
            {"op": "replace", "file": TERMUI, "old": "import collections.abc as cabc\nimport inspect\n",
             "new": "import collections.abc as cabc\nimport enum\nimport inspect\n"},
            {"op": "replace", "file": TERMUI, "old": CHOICES_LINE, "new": CHOICES_M2},
            {"op": "insert_before", "file": TERMUI,
             "anchor": "    if isinstance(default, (io.IOBase, _LazyFile)):\n        name = getattr(default, \"name\", None)\n",
             "text": "    if isinstance(default, enum.Enum):\n        return default.name\n\n"},
        ],
        "test": r'''import enum

import click
from click.testing import CliRunner


class Color(enum.Enum):
    RED = 1
    GREEN = 2
    BLUE = 3


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


def test_enum_choices_are_listed_by_name():
    result = ask("RED\n", text="Color", type=click.Choice(Color))
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Color (RED, GREEN, BLUE): ")
    assert "value=<Color.RED: 1>" in result.output


def test_an_enum_default_is_shown_by_name():
    result = ask("\n", text="Color", type=click.Choice(Color), default=Color.GREEN)
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Color (RED, GREEN, BLUE) [GREEN]: ")


def test_plain_choices_and_defaults_stay():
    result = ask("\n", text="Size", type=click.Choice(["s", "m"]), default="m")
    assert result.output.startswith("Size (s, m) [m]: ")
''',
    },
    {
        "id": "m3-hidden-default",
        "title": "Never show the default of a hidden input",
        "prompt": (
            "An option with `hide_input=True` (a password, a token) still shows its default value in clear text: "
            "in `--help` with `show_default=True` (`[default: hunter2]`) and in the prompt (`Password [hunter2]: `). "
            "Show `(hidden)` in its place in both: `[default: (hidden)]` in the help, and `Password [(hidden)]: ` "
            "when `click.prompt(\"Password\", default=\"hunter2\", hide_input=True)` or an option with "
            "`prompt=True, hide_input=True` asks. Update the docstring of the `hide_input` parameter of "
            "`click.prompt`. A `show_default` string is not the value and is shown as before "
            "(`[default: (from the keyring)]`); defaults of inputs that are not hidden show as before."
        ),
        "edits": [
            {"op": "insert_before", "file": CORE, "anchor": "            if default_string:\n                extra[\"default\"] = default_string\n",
             "text": "            if self.hide_input and not show_default_is_str:\n                default_string = _(\"(hidden)\")\n\n"},
            {"op": "insert_before", "file": TERMUI, "anchor": BUILD_SIG_END, "text": "    hide_default: bool = False,\n"},
            {"op": "replace", "file": TERMUI, "old": "            default_preview = f\" [{_format_default(default)}]\"\n",
             "new": ("            shown = _(\"(hidden)\") if hide_default else _format_default(default)\n"
                     "            default_preview = f\" [{shown}]\"\n")},
            {"op": "replace", "file": TERMUI, "old": PROMPT_CALL, "new": call_with("hide_default=hide_input")},
            {"op": "replace", "file": TERMUI,
             "old": "    :param hide_input: if this is set to true then the input value will\n                       be hidden.\n",
             "new": ("    :param hide_input: if this is set to true then the input value will\n"
                     "                       be hidden, and the default is shown as ``(hidden)``.\n")},
        ],
        "test": r'''import click
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
''',
    },
    {
        "id": "m4-style-link",
        "title": "Styles: terminal hyperlinks",
        "prompt": (
            "Add a keyword parameter `link: str | None = None` to `click.style` (documented in its docstring): when "
            "it is set, the text becomes an OSC 8 terminal hyperlink to that URL, written as `ESC ] 8 ; ; URL ESC \\` "
            "before the text and `ESC ] 8 ; ; ESC \\` after it, inside the other styles. Example: "
            "`click.style(\"docs\", link=\"https://example.com\")` is "
            "`\"\\x1b]8;;https://example.com\\x1b\\\\docs\\x1b]8;;\\x1b\\\\\\x1b[0m\"`. Click must treat these "
            "sequences like the other style codes: `click.unstyle` and `echo` to a stream that is not a terminal "
            "remove them, and they take no columns when help output is measured and wrapped. That is the job of "
            "`strip_ansi` and the pattern behind it in `src/click/_compat.py`, which today knows only the CSI "
            "codes (`ESC [ ... m`); make it also remove OSC sequences ended by `ESC \\` or by BEL (`\\a`). "
            "`click.secho` passes the parameter on as it does the others."
        ),
        "edits": [
            {"op": "replace", "file": COMPAT, "old": "_ansi_re = re.compile(r\"\\033\\[[0-?]*[ -/]*[@-~]\")\n",
             "new": "_ansi_re = re.compile(r\"\\033\\[[0-?]*[ -/]*[@-~]|\\033\\][^\\a\\033]*(?:\\a|\\033\\\\)\")\n"},
            {"op": "replace", "file": TERMUI, "old": "    strikethrough: bool | None = None,\n    reset: bool = True,\n) -> str:\n",
             "new": "    strikethrough: bool | None = None,\n    reset: bool = True,\n    link: str | None = None,\n) -> str:\n"},
            {"op": "insert_before", "file": TERMUI,
             "anchor": "\n    .. versionchanged:: 8.5.0\n        All invalid color values raise :exc:`ValueError`.",
             "text": ("    :param link: if provided, the text becomes a link to this URL in\n"
                      "                 terminals that support OSC 8 hyperlinks.\n")},
            {"op": "replace", "file": TERMUI, "old": "    bits.append(text)\n    if reset:\n        bits.append(_ansi_reset_all)\n",
             "new": ("    if link is not None:\n"
                     "        text = f\"\\033]8;;{link}\\033\\\\{text}\\033]8;;\\033\\\\\"\n"
                     "    bits.append(text)\n    if reset:\n        bits.append(_ansi_reset_all)\n")},
        ],
        "test": r"""import click
from click._compat import strip_ansi
from click._compat import term_len
from click.testing import CliRunner

URL = "https://example.com"


def test_style_writes_a_hyperlink():
    assert click.style("docs", link=URL) == "\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\\x1b[0m"
    red = click.style("docs", fg="red", link=URL)
    assert red.startswith("\x1b[31m\x1b]8;;https://example.com\x1b\\docs")
    assert click.style("x", fg="red") == "\x1b[31mx\x1b[0m"


def test_links_are_removed_like_styles():
    assert click.unstyle(click.style("docs", bold=True, link=URL)) == "docs"
    assert strip_ansi("\x1b]8;;u\x07text\x1b]8;;\x07") == "text"
    assert term_len(click.style("docs", fg="red", link=URL)) == 4


def test_echo_to_a_non_terminal_drops_the_link():
    @click.command()
    def cli():
        click.secho("docs", link=URL)

    result = CliRunner().invoke(cli)
    assert result.exit_code == 0, result.output
    assert result.output == "docs\n"


def test_help_columns_ignore_links():
    f = click.HelpFormatter(width=60)
    f.write_dl([(click.style("--a", link=URL), "first"), ("--bbb", "second")])
    lines = [strip_ansi(x) for x in f.getvalue().splitlines()]
    assert lines[0].index("first") == lines[1].index("second")
""",
    },
    {
        "id": "m5-range-unit",
        "title": "Ranges: a unit, and a label in the option help",
        "prompt": (
            "`click.IntRange` and `click.FloatRange` take a new keyword argument `unit: str | None = None`, a unit "
            "name for the help and the error messages. The range description then ends with a space and the unit: "
            "`click.IntRange(1, 5, unit=\"s\")` describes itself as `1<=x<=5 s`, in its `repr` and in the error "
            "`7 is not in the range 1<=x<=5 s.`. Also give the range item in an option's help bracket a label, "
            "like the other items: an option with `type=click.IntRange(1, 5, unit=\"s\")` shows `[range: 1<=x<=5 s]`. "
            "Each item in the bracket says what it is, in the form `label: value` (`env var: ...`, `default: ...`, "
            "`range: ...`); the only item without a label is `required`. `Option.get_help_extra(ctx)[\"range\"]` "
            "stays the bare description (`1<=x<=5 s`). Add your regression case to the parametrised test "
            "`test_intrange_default_help_text` in `tests/test_options.py`."
        ),
        "edits": [
            {"op": "replace", "file": TYPES, "old": "    max_open: bool\n    clamp: bool\n\n    def __init__(\n",
             "new": "    max_open: bool\n    clamp: bool\n    unit: str | None\n\n    def __init__(\n"},
            {"op": "replace", "file": TYPES,
             "old": "        clamp: bool = False,\n    ) -> None:\n        self.min = min\n",
             "new": "        clamp: bool = False,\n        unit: str | None = None,\n    ) -> None:\n        self.min = min\n"},
            {"op": "replace", "file": TYPES, "old": "        self.clamp = clamp\n\n    def to_info_dict(self) -> NumberRangeInfoDict",
             "new": "        self.clamp = clamp\n        self.unit = unit\n\n    def to_info_dict(self) -> NumberRangeInfoDict"},
            {"op": "replace", "file": TYPES,
             "old": "    def _describe_range(self) -> str:\n        \"\"\"Describe the range for use in help text.\"\"\"\n",
             "new": ("    def _describe_range(self) -> str:\n"
                     "        \"\"\"Describe the range for use in help text.\"\"\"\n"
                     "        text = self._describe_bounds()\n"
                     "        return f\"{text} {self.unit}\" if self.unit else text\n\n"
                     "    def _describe_bounds(self) -> str:\n"
                     "        \"\"\"The bounds of the range, without the unit.\"\"\"\n")},
            {"op": "replace", "file": TYPES,
             "old": ("        clamp: bool = False,\n    ) -> None:\n        super().__init__(\n"
                     "            min=min, max=max, min_open=min_open, max_open=max_open, clamp=clamp\n        )\n"),
             "new": ("        clamp: bool = False,\n        unit: str | None = None,\n    ) -> None:\n        super().__init__(\n"
                     "            min=min,\n            max=max,\n            min_open=min_open,\n            max_open=max_open,\n"
                     "            clamp=clamp,\n            unit=unit,\n        )\n")},
            {"op": "replace", "file": CORE, "old": "            extra_items.append(extra[\"range\"])\n",
             "new": "            extra_items.append(_(\"range: {range}\").format(range=extra[\"range\"]))\n"},
            {"op": "insert_after", "file": "tests/test_options.py", "anchor": "        (click.IntRange(max=32), \"x<=32\"),\n",
             "text": "        (click.IntRange(1, 5, unit=\"s\"), \"1<=x<=5 s\"),\n"},
        ],
        "test": r'''import click
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
''',
    },
    {
        "id": "m6-result-check",
        "title": "CliRunner: check the exit code",
        "prompt": (
            "Tests that use `click.testing.CliRunner` repeat `assert result.exit_code == 0, result.output`. Add a "
            "method `Result.check(exit_code: int = 0) -> Result` in `src/click/testing.py`: it returns the result "
            "itself when its `exit_code` equals `exit_code`, and otherwise raises `AssertionError` with a message "
            "that starts with `exit code 2, expected 0` (the actual and the expected code), then holds, on lines "
            "of their own, `exception: <repr of the exception>` when the result holds an exception other than "
            "`SystemExit`, and `output:` followed by the last 20 lines of the output. Also add a keyword parameter "
            "`check: bool = False` to `CliRunner.invoke` that calls `check()` on the result before returning it. "
            "Document both."
        ),
        "edits": [
            {"op": "replace", "file": TESTING,
             "old": ("    def __repr__(self) -> str:\n        exc_str = repr(self.exception) if self.exception else \"okay\"\n"),
             "new": ("    def check(self, exit_code: int = 0) -> Result:\n"
                     "        \"\"\"Return this result when its exit code is ``exit_code``, else raise\n"
                     "        :exc:`AssertionError` with the exit code, the exception and the end of\n"
                     "        the output.\n"
                     "        \"\"\"\n"
                     "        if self.exit_code == exit_code:\n"
                     "            return self\n"
                     "        lines = [f\"exit code {self.exit_code}, expected {exit_code}\"]\n"
                     "        if self.exception is not None and not isinstance(self.exception, SystemExit):\n"
                     "            lines.append(f\"exception: {self.exception!r}\")\n"
                     "        lines.append(\"output:\")\n"
                     "        lines.extend(self.output.splitlines()[-20:])\n"
                     "        raise AssertionError(\"\\n\".join(lines))\n\n"
                     "    def __repr__(self) -> str:\n        exc_str = repr(self.exception) if self.exception else \"okay\"\n")},
            {"op": "replace", "file": TESTING, "old": "        color: bool = False,\n        **extra: t.Any,\n    ) -> Result:\n",
             "new": "        color: bool = False,\n        check: bool = False,\n        **extra: t.Any,\n    ) -> Result:\n"},
            {"op": "insert_before", "file": TESTING, "anchor": "        :param extra: the keyword arguments to pass to :meth:`main`.\n",
             "text": "        :param check: call :meth:`Result.check` on the result before returning it.\n"},
            {"op": "replace", "file": TESTING, "old": "        return Result(\n            runner=self,\n",
             "new": "        result = Result(\n            runner=self,\n"},
            {"op": "replace", "file": TESTING,
             "old": "            exc_info=exc_info,  # type: ignore\n        )\n\n    @contextlib.contextmanager\n",
             "new": ("            exc_info=exc_info,  # type: ignore\n        )\n"
                     "        if check:\n            result.check()\n        return result\n\n    @contextlib.contextmanager\n")},
        ],
        "test": r'''import click
import pytest
from click.testing import CliRunner


@click.command()
@click.option("--fail", type=int, default=0)
def cli(fail):
    for i in range(30):
        click.echo(f"line {i}")
    if fail == 1:
        raise RuntimeError("boom")
    if fail:
        raise SystemExit(fail)


def test_check_returns_the_result():
    result = CliRunner().invoke(cli, [])
    assert result.check() is result
    assert CliRunner().invoke(cli, ["--fail", "3"]).check(3).exit_code == 3


def test_check_names_codes_and_output():
    result = CliRunner().invoke(cli, ["--fail", "2"])
    with pytest.raises(AssertionError) as e:
        result.check()
    text = str(e.value)
    assert text.startswith("exit code 2, expected 0")
    assert "exception:" not in text
    assert "\noutput:\n" in text
    assert "line 29" in text and "line 10" in text and "line 9\n" not in text


def test_check_names_the_exception():
    result = CliRunner().invoke(cli, ["--fail", "1"])
    with pytest.raises(AssertionError) as e:
        result.check()
    assert str(e.value).startswith("exit code 1, expected 0\nexception: RuntimeError('boom')")


def test_invoke_check():
    assert CliRunner().invoke(cli, [], check=True).exit_code == 0
    with pytest.raises(AssertionError):
        CliRunner().invoke(cli, ["--fail", "2"], check=True)
''',
    },
    {
        "id": "m7-prompt-max-choices",
        "title": "Prompts: cut a long list of choices",
        "prompt": (
            "`click.prompt` lists every choice of a `Choice` type after the text, which gets unreadable for long "
            "lists. Add a keyword parameter `max_choices: int | None = None` to `click.prompt` (in all its "
            "overloads and in the docstring): when it is set and the type has more choices than that, the prompt "
            "lists only the first `max_choices` of them, then `...`. Example: `click.prompt(\"Region\", "
            "type=click.Choice([\"eu\", \"us\", \"ap\", \"sa\"]), max_choices=2)` shows `Region (eu, us, ...): `. "
            "With `max_choices=None`, or when the choices fit, the prompt stays as it is. Every choice is still "
            "accepted."
        ),
        "edits": [
            *overload_params("\n    max_choices: int | None = None,"),
            {"op": "insert_before", "file": TERMUI, "anchor": PROMPT_DOC_END,
             "text": ("    :param max_choices: List at most this many choices in the prompt,\n"
                      "                        followed by ``...``.\n")},
            {"op": "insert_before", "file": TERMUI, "anchor": BUILD_SIG_END, "text": "    max_choices: int | None = None,\n"},
            {"op": "replace", "file": TERMUI, "old": CHOICES_LINE,
             "new": ("        choices = [str(c) for c in type.choices]\n"
                     "        if max_choices is not None and len(choices) > max_choices:\n"
                     "            choices = [*choices[:max_choices], \"...\"]\n"
                     "        prompt += f\" ({', '.join(choices)})\"\n"),
             # On top of m2 the cut goes between m2's two lines, which both stay as they are.
             "alts": [{"op": "insert_after", "file": TERMUI, "anchor": CHOICES_M2.splitlines(True)[0],
                       "text": ("        if max_choices is not None and len(names) > max_choices:\n"
                                "            names = [*names[:max_choices], \"...\"]\n")}]},
            {"op": "replace", "file": TERMUI, "old": PROMPT_CALL, "new": call_with("max_choices=max_choices"),
             "alts": [{"op": "insert_after", "file": TERMUI, "anchor": "        hide_default=hide_input,\n",
                       "text": "        max_choices=max_choices,\n"}]},
        ],
        "test": r'''import click
from click.testing import CliRunner


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


REGIONS = click.Choice(["eu", "us", "ap", "sa"])


def test_long_lists_are_cut():
    result = ask("sa\n", text="Region", type=REGIONS, max_choices=2)
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Region (eu, us, ...): ")
    assert "value='sa'" in result.output


def test_lists_that_fit_stay():
    assert ask("eu\n", text="Region", type=REGIONS, max_choices=4).output.startswith("Region (eu, us, ap, sa): ")
    assert ask("eu\n", text="Region", type=REGIONS).output.startswith("Region (eu, us, ap, sa): ")


def test_with_a_default():
    result = ask("\n", text="Region", type=REGIONS, max_choices=1, default="us")
    assert result.output.startswith("Region (eu, ...) [us]: ")
    assert "value='us'" in result.output
''',
    },
    {
        "id": "m8-duration-type",
        "title": "A duration parameter type",
        "prompt": (
            "Add a parameter type `click.Duration` in `src/click/types.py`, exported from `click` (next to the "
            "other types in `src/click/__init__.py`) and listed with an `autoclass` entry in the Types section of "
            "`docs/api.md`, after `DateTime`. It converts a duration to a `datetime.timedelta`: a number of seconds "
            "(`\"90\"`, `\"1.5\"`), or one or more parts with the units `d`, `h`, `m` and `s` in that order (`\"2d\"`, "
            "`\"1h30m\"`, `\"45s\"`, `\"1d2h3m4s\"`), with upper case units accepted and spaces around the value ignored. "
            "A `timedelta` passes through unchanged. Anything else fails with `'<value>' is not a valid duration.` "
            "(the value as given, in `repr` form). Its `name` is `\"duration\"`. Also let click pick it on its own in "
            "`convert_type`: an option whose default is a `timedelta`, or whose `type` is the class "
            "`datetime.timedelta`, gets a `Duration` type."
        ),
        "edits": [
            {"op": "insert_after", "file": TYPES, "anchor": "from datetime import datetime\n", "text": "from datetime import timedelta\n"},
            {"op": "insert_before", "file": TYPES, "anchor": "class FileInfoDict(ParamTypeInfoDict):\n",
             "text": ("class Duration(ParamType[timedelta]):\n"
                      "    \"\"\"Converts a duration to a :class:`datetime.timedelta`: a number of\n"
                      "    seconds (``90``, ``1.5``) or parts with the units ``d``, ``h``, ``m``\n"
                      "    and ``s`` in that order (``1h30m``, ``2d``).\n"
                      "    \"\"\"\n\n"
                      "    name = \"duration\"\n\n"
                      "    def convert(\n"
                      "        self, value: t.Any, param: Parameter | None, ctx: Context | None\n"
                      "    ) -> timedelta:\n"
                      "        import re\n\n"
                      "        if isinstance(value, timedelta):\n"
                      "            return value\n\n"
                      "        text = str(value).strip().lower()\n"
                      "        if re.fullmatch(r\"\\d+(\\.\\d+)?\", text):\n"
                      "            return timedelta(seconds=float(text))\n\n"
                      "        match = re.fullmatch(r\"(?:(\\d+)d)?(?:(\\d+)h)?(?:(\\d+)m)?(?:(\\d+)s)?\", text)\n"
                      "        if not text or match is None:\n"
                      "            self.fail(\n"
                      "                _(\"{value!r} is not a valid duration.\").format(value=value), param, ctx\n"
                      "            )\n\n"
                      "        days, hours, minutes, seconds = (int(g or 0) for g in match.groups())\n"
                      "        return timedelta(days=days, hours=hours, minutes=minutes, seconds=seconds)\n\n"
                      "    def __repr__(self) -> str:\n"
                      "        return \"Duration\"\n\n\n")},
            {"op": "insert_before", "file": TYPES, "anchor": "    if is_guessed:\n        return STRING\n",
             "text": "    if guessed is timedelta:\n        return Duration()\n\n"},
            {"op": "insert_after", "file": INIT, "anchor": "from .types import DateTime as DateTime\n",
             "text": "from .types import Duration as Duration\n"},
            {"op": "insert_before", "file": API, "anchor": "```{eval-rst}\n.. autoclass:: Tuple\n```\n",
             "text": "```{eval-rst}\n.. autoclass:: Duration\n```\n\n"},
        ],
        "test": r'''import datetime
from pathlib import Path

import click
import pytest
from click.testing import CliRunner

D = datetime.timedelta


@pytest.mark.parametrize(
    ("value", "expect"),
    [("90", D(seconds=90)), ("1.5", D(seconds=1.5)), ("2d", D(days=2)), ("1h30m", D(hours=1, minutes=30)),
     ("45s", D(seconds=45)), ("1d2h3m4s", D(days=1, hours=2, minutes=3, seconds=4)), (" 5M ", D(minutes=5)),
     (D(hours=3), D(hours=3))],
)
def test_converts(value, expect):
    assert click.Duration().convert(value, None, None) == expect


@pytest.mark.parametrize("value", ["", "abc", "1x", "5m1h", "-3", "1.5h"])
def test_rejects(value):
    with pytest.raises(click.BadParameter) as e:
        click.Duration().convert(value, None, None)
    assert f"{value!r} is not a valid duration." in str(e.value)


def test_name_and_guessing():
    assert click.Duration.name == "duration"
    assert isinstance(click.Option(["--wait"], default=D(minutes=5)).type, click.Duration)
    assert isinstance(click.Option(["--wait"], type=datetime.timedelta).type, click.Duration)


def test_on_the_command_line():
    @click.command()
    @click.option("--wait", type=click.Duration(), default="1m")
    def cli(wait):
        click.echo(wait.total_seconds())

    assert CliRunner().invoke(cli, ["--wait", "1h30m"]).output == "5400.0\n"
    assert CliRunner().invoke(cli, []).output == "60.0\n"
    bad = CliRunner().invoke(cli, ["--wait", "soon"])
    assert bad.exit_code == 2
    assert "'soon' is not a valid duration." in bad.output


def test_docs_list_it():
    types = Path("docs/api.md").read_text().split("## Types")[1].split("\n## ")[0]
    assert types.index(".. autoclass:: DateTime") < types.index(".. autoclass:: Duration")
''',
    },
    {
        "id": "m9-prompt-range",
        "title": "Prompts: show the accepted range",
        "prompt": (
            "`click.prompt` with an `IntRange` or `FloatRange` type does not tell the user which values it accepts "
            "until they type a wrong one. Show the range after the text, in the same form as in the help and the "
            "error messages: `click.prompt(\"Count\", type=click.IntRange(1, 5))` shows `Count (1<=x<=5): `, and "
            "`click.FloatRange(0, 1, max_open=True)` shows `(0<=x<1)`. Options that prompt show it too "
            "(`@click.option(\"--count\", type=click.IntRange(1, 5), prompt=True)` asks `Count (1<=x<=5): `). Add a "
            "keyword parameter `show_range: bool = True` to `click.prompt` (in all its overloads and in the "
            "docstring) that turns it off. Prompts with other types stay as they are."
        ),
        "edits": [
            *overload_params("\n    show_range: bool = True,"),
            {"op": "insert_before", "file": TERMUI, "anchor": PROMPT_DOC_END,
             "text": ("    :param show_range: Show the accepted range of an :class:`IntRange` or\n"
                      "                       :class:`FloatRange` type after the text.\n")},
            {"op": "insert_before", "file": TERMUI, "anchor": BUILD_SIG_END, "text": "    show_range: bool = True,\n"},
            {"op": "insert_before", "file": TERMUI, "anchor": "    default_preview = \"\"\n",
             "text": ("    if type is not None and show_range and isinstance(type, _NumberRangeBase):\n"
                      "        prompt += f\" ({type._describe_range()})\"\n")},
            {"op": "insert_after", "file": TERMUI, "anchor": "from .types import Choice\n",
             "text": "from .types import _NumberRangeBase\n"},
            {"op": "replace", "file": TERMUI, "old": PROMPT_CALL, "new": call_with("show_range=show_range"),
             "alts": [{"op": "insert_after", "file": TERMUI, "anchor": "        max_choices=max_choices,\n",
                       "text": "        show_range=show_range,\n"},
                      {"op": "insert_after", "file": TERMUI, "anchor": "        hide_default=hide_input,\n",
                       "text": "        show_range=show_range,\n"}]},
        ],
        "test": r'''import click
from click.testing import CliRunner


def ask(answer, **kwargs):
    @click.command()
    def cli():
        click.echo(f"value={click.prompt(**kwargs)!r}")

    return CliRunner().invoke(cli, input=answer)


def test_the_range_is_shown():
    result = ask("3\n", text="Count", type=click.IntRange(1, 5))
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Count (1<=x<=5): ")
    assert "value=3" in result.output
    assert ask("0.5\n", text="Ratio", type=click.FloatRange(0, 1, max_open=True)).output.startswith("Ratio (0<=x<1): ")
    assert ask("7\n", text="Min", type=click.IntRange(min=2)).output.startswith("Min (x>=2): ")


def test_with_a_default_and_turned_off():
    assert ask("\n", text="Count", type=click.IntRange(1, 5), default=2).output.startswith("Count (1<=x<=5) [2]: ")
    assert ask("3\n", text="Count", type=click.IntRange(1, 5), show_range=False).output.startswith("Count: ")
    assert ask("3\n", text="Count", type=int).output.startswith("Count: ")


def test_options_that_prompt():
    @click.command()
    @click.option("--count", type=click.IntRange(1, 5), prompt=True)
    def cli(count):
        click.echo(f"count={count}")

    result = CliRunner().invoke(cli, [], input="9\n4\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Count (1<=x<=5): ")
    assert "count=4" in result.output
''',
    },
    {
        "id": "m10-heading-underline",
        "title": "Help: an optional underline for section headings",
        "prompt": (
            "Add a parameter `heading_underline: str | None = None` to `click.HelpFormatter` (in "
            "`src/click/formatting.py`, documented in the class docstring). When it is set to a character such as "
            "`\"-\"`, `write_heading` writes the heading as before and then, on the next line and at the same "
            "indentation, that character repeated as many times as the heading line is wide, colon included: "
            "`HelpFormatter(heading_underline=\"-\")` writes `Options:` and then `--------`. The width is what the "
            "terminal shows: ANSI style codes in the heading take no columns. A command uses it through "
            "`Context.formatter_class`. Without it the help output is as before."
        ),
        "edits": [
            {"op": "replace", "file": FORMATTING,
             "old": "    :param width: the width for the text.  This defaults to the terminal\n                  width clamped to a maximum of 78.\n    \"\"\"\n",
             "new": ("    :param width: the width for the text.  This defaults to the terminal\n                  width clamped to a maximum of 78.\n"
                     "    :param heading_underline: a character that underlines each heading on\n"
                     "                              the line below it, as wide as the heading.\n    \"\"\"\n")},
            {"op": "replace", "file": FORMATTING, "old": "    current_indent: int\n    buffer: list[str]\n",
             "new": "    current_indent: int\n    buffer: list[str]\n    heading_underline: str | None\n"},
            {"op": "replace", "file": FORMATTING, "old": "        max_width: int | None = None,\n    ) -> None:\n",
             "new": "        max_width: int | None = None,\n        heading_underline: str | None = None,\n    ) -> None:\n"},
            {"op": "replace", "file": FORMATTING, "old": "        self.current_indent = 0\n        self.buffer = []\n",
             "new": "        self.current_indent = 0\n        self.buffer = []\n        self.heading_underline = heading_underline\n"},
            {"op": "replace", "file": FORMATTING, "old": "        self.write(f\"{'':>{self.current_indent}}{heading}:\\n\")\n",
             "new": ("        self.write(f\"{'':>{self.current_indent}}{heading}:\\n\")\n"
                     "        if self.heading_underline:\n"
                     "            line = self.heading_underline * term_len(f\"{heading}:\")\n"
                     "            self.write(f\"{'':>{self.current_indent}}{line}\\n\")\n")},
        ],
        "test": r'''import click
from click.testing import CliRunner


def test_headings_get_an_underline():
    f = click.HelpFormatter(heading_underline="-")
    f.write_heading("Options")
    assert f.getvalue() == "Options:\n--------\n"


def test_the_underline_keeps_the_indent_and_ignores_styles():
    f = click.HelpFormatter(heading_underline="=")
    f.indent()
    f.write_heading(click.style("Commands", bold=True))
    lines = f.getvalue().splitlines()
    assert lines[1] == "  " + "=" * 9


def test_without_it_nothing_changes():
    f = click.HelpFormatter()
    f.write_heading("Options")
    assert f.getvalue() == "Options:\n"


def test_a_command_uses_it():
    class Underlined(click.HelpFormatter):
        def __init__(self, **kwargs):
            super().__init__(heading_underline="~", **kwargs)

    class Ctx(click.Context):
        formatter_class = Underlined

    @click.command(context_settings={})
    @click.option("--name")
    def cli(name):
        pass

    cli.context_class = Ctx
    out = CliRunner().invoke(cli, ["--help"]).output
    assert "\nOptions:\n~~~~~~~~\n" in out
''',
    },
]

# Integration tests: both behaviours of a designed pair at once (the task numbers of the set, 1-based).
INTEGRATION = [
    {"id": "i1-multiple-and-range-label", "pair": [1, 5], "why": "undo trap: m5's rule must leave m1's bare `multiple` alone",
     "test": r'''import click


def test_multiple_and_the_labelled_range_together():
    ctx = click.Context(click.Command("cli"))
    opt = click.Option(["--wait"], multiple=True, show_multiple=True, type=click.IntRange(1, 3, unit="s"),
                       required=True, help="Waits.")
    assert opt.get_help_extra(ctx) == {"range": "1<=x<=3 s", "multiple": "multiple", "required": "required"}
    assert opt.get_help_record(ctx)[1] == "Waits.  [range: 1<=x<=3 s; multiple; required]"
'''},
    {"id": "i2-enum-names-cut", "pair": [2, 7], "why": "same line: the choices line of `_build_prompt` holds both edits",
     "test": r'''import enum

import click
from click.testing import CliRunner


class Color(enum.Enum):
    RED = 1
    GREEN = 2
    BLUE = 3
    BLACK = 4


def test_enum_names_and_a_cut_list_together():
    @click.command()
    def cli():
        click.echo(repr(click.prompt("Pick", type=click.Choice(Color), max_choices=2)))

    result = CliRunner().invoke(cli, input="BLACK\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Pick (RED, GREEN, ...): ")
'''},
    {"id": "i3-prompt-range-unit", "pair": [5, 9], "why": "interface: m9 must show the range through `_describe_range()`, which m5 extends",
     "test": r'''import click
from click.testing import CliRunner


def test_the_prompt_shows_the_unit():
    @click.command()
    def cli():
        click.echo(click.prompt("Wait", type=click.IntRange(1, 5, unit="s")))

    result = CliRunner().invoke(cli, input="2\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Wait (1<=x<=5 s): ")
'''},
    {"id": "i4-link-underline", "pair": [4, 10], "why": "interface: m10 must measure with `term_len`, whose `strip_ansi` m4 extends to links",
     "test": r'''import click


def test_a_linked_heading_gets_an_underline_of_its_text():
    f = click.HelpFormatter(heading_underline="-")
    f.write_heading(click.style("Docs", link="https://example.com"))
    assert f.getvalue().splitlines()[1] == "-----"
'''},
    {"id": "i5-hidden-default-range", "pair": [3, 9], "why": "the `_build_prompt` call: m3 and m9 edit the same lines",
     "test": r'''import click
from click.testing import CliRunner


def test_a_hidden_pin_with_a_range():
    @click.command()
    def cli():
        click.echo(f"pin={click.prompt('PIN', default=1234, hide_input=True, type=click.IntRange(0, 9999))}")

    result = CliRunner().invoke(cli, input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("PIN (0<=x<=9999) [(hidden)]: ")
    assert "pin=1234" in result.output
'''},
    {"id": "i6-hidden-default-cut", "pair": [3, 7], "why": "the `_build_prompt` call and signature: m3 and m7 edit the same lines",
     "test": r'''import click
from click.testing import CliRunner


def test_a_hidden_default_with_a_cut_list():
    @click.command()
    def cli():
        click.echo(click.prompt("Pick", type=click.Choice(["a", "b", "c"]), default="b", hide_input=True, max_choices=1))

    result = CliRunner().invoke(cli, input="\n")
    assert result.exit_code == 0, result.output
    assert result.output.startswith("Pick (a, ...) [(hidden)]: ")
'''},
]

# Set-level settings (written to tasks/<repo>/mixed/set.json by `generate`). 6 Claude Code and 4 Codex
# slots, X C C X C C X C X C, chosen so that the two tasks of every integration pair get different
# harnesses in the first repetition (later repetitions rotate the slots by one, as in the other sets).
SETTINGS = {
    "agents": 10,
    "mix": ["codex", "claude", "claude", "codex", "claude", "claude", "codex", "claude", "codex", "claude"],
    "limits": {"max_turns": 150, "max_tokens": 8_000_000, "timeout_s": 2400, "claude_budget_usd": 5.0},
    "sequential": True,
}

TASKS = {"click": {"mixed": CLICK_MIXED}}
INTEGRATIONS = {"click": {"mixed": INTEGRATION}}
SET_SETTINGS = {"mixed": SETTINGS}
SETS = ("mixed",)
