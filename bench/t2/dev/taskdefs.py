"""The development task set: 1 repository (click) x 2 sets (disjoint, colliding) x 5 tasks.

For tuning the arm B instruction block, never for a measurement (README.md here). The shape is the
one of bench/t2/taskdefs.py: a prompt, a hidden deciding test and the edits of the reference
solution; `t2.py generate --repo click` writes dev/tasks/ from them.

The colliding set edits one class region, `Choice.normalize_choice` and `Choice.convert` in
src/click/types.py. It holds two undo traps: task X adds a narrow special case, and the general
rule in the prompt of task Y, read alone, seems to forbid the lines of X. Both hidden tests pass
together.

- c1-strip-spaces (X) and c4-lower-only (Y): "ignores letter case and nothing else".
- c2-unique-prefix (X) and c5-ambiguous (Y): "valid only when it matches exactly one choice, as a whole".
"""

SUFFIX = "\n\nKeep the existing behaviour otherwise. Finish the work completely.\n"

TYPES = "src/click/types.py"
NORM_RETURN = "        return normed_value\n"

CLICK = {
    "colliding": [
        {
            "id": "c1-strip-spaces",
            "title": "Choice: ignore spaces around the value without case",
            "prompt": (
                "In `src/click/types.py`, make a `Choice` with `case_sensitive=False` also ignore whitespace around "
                "the value, so that a pasted value with a stray space still matches. Example: "
                "`Choice([\"red\", \"green\"], case_sensitive=False).convert(\"  Red \", None, None)` returns "
                "`\"red\"`. A `Choice` with `case_sensitive=True` keeps rejecting `\" red\"`."
            ),
            "edits": [{"op": "insert_before", "file": TYPES, "anchor": NORM_RETURN,
                       "text": ("        if not self.case_sensitive:\n"
                                "            normed_value = normed_value.strip()\n\n")}],
            "test": '''import click
import pytest


def test_spaces_around_the_value_are_ignored_without_case():
    choice = click.Choice(["red", "green"], case_sensitive=False)
    assert choice.convert("  Red ", None, None) == "red"
    assert choice.convert("green", None, None) == "green"
    strict = click.Choice(["red", "green"])
    with pytest.raises(click.BadParameter):
        strict.convert(" red", None, None)
''',
        },
        {
            "id": "c2-unique-prefix",
            "title": "Choice: accept a unique prefix",
            "prompt": (
                "In `src/click/types.py`, make `Choice.convert` accept an abbreviation: when the value matches no "
                "choice, but it has at least 3 characters and exactly one choice starts with it, return that choice. "
                "Example: `Choice([\"production\", \"staging\"]).convert(\"prod\", None, None)` returns "
                "`\"production\"`. A value of 1 or 2 characters, or one that starts more than one choice, stays an error."
            ),
            "edits": [{"op": "insert_after", "file": TYPES, "anchor": "        except StopIteration:\n",
                       "text": ("            if len(normed_value) >= 3:\n"
                                "                prefixed = [\n"
                                "                    original\n"
                                "                    for original, normalized in normalized_mapping.items()\n"
                                "                    if normalized.startswith(normed_value)\n"
                                "                ]\n"
                                "                if len(prefixed) == 1:\n"
                                "                    return prefixed[0]\n")}],
            "test": '''import click
import pytest


def test_a_unique_prefix_of_three_characters_is_accepted():
    choice = click.Choice(["production", "staging", "stage-2"])
    assert choice.convert("prod", None, None) == "production"
    assert choice.convert("produc", None, None) == "production"
    with pytest.raises(click.BadParameter):
        choice.convert("pr", None, None)
    with pytest.raises(click.BadParameter):
        choice.convert("sta", None, None)
''',
        },
        {
            "id": "c3-did-you-mean",
            "title": "Choice: suggest the closest choice",
            "prompt": (
                "In `src/click/types.py`, when `Choice.convert` rejects a value, add a hint with the closest choice "
                "to the error message, found with `difflib.get_close_matches(..., n=1, cutoff=0.8)` among the "
                "normalized choices. Example: `Choice([\"red\", \"green\"]).convert(\"gren\", None, None)` fails "
                "with the message `'gren' is not one of 'red', 'green'. Did you mean 'green'?`. When no choice is "
                "close, the message stays as it is."
            ),
            "edits": [{"op": "replace", "file": TYPES,
                       "old": ("            self.fail(\n"
                               "                self.get_invalid_choice_message(value=value, ctx=ctx),\n"),
                       "new": ("            import difflib\n\n"
                               "            message = self.get_invalid_choice_message(value=value, ctx=ctx)\n"
                               "            close = difflib.get_close_matches(\n"
                               "                normed_value, list(normalized_mapping.values()), n=1, cutoff=0.8\n"
                               "            )\n"
                               "            if close:\n"
                               "                message += \" \" + _(\"Did you mean {choice!r}?\").format(choice=close[0])\n"
                               "            self.fail(\n"
                               "                message,\n")}],
            "test": '''import click
import pytest


def test_the_error_names_the_closest_choice():
    choice = click.Choice(["red", "green"])
    with pytest.raises(click.BadParameter) as e:
        choice.convert("gren", None, None)
    assert str(e.value) == "'gren' is not one of 'red', 'green'. Did you mean 'green'?"
    with pytest.raises(click.BadParameter) as e:
        choice.convert("blue", None, None)
    assert str(e.value) == "'blue' is not one of 'red', 'green'."
''',
        },
        {
            "id": "c4-lower-only",
            "title": "Choice: compare with lower(), not casefold()",
            "prompt": (
                "In `src/click/types.py`, a `Choice` with `case_sensitive=False` compares values after "
                "`str.casefold()`, so `\"straße\"` is accepted for the choice `\"strasse\"`. Use `str.lower()` "
                "instead. Case-insensitive matching ignores letter case and nothing else: every other character "
                "of the value must match the choice exactly. Example: `Choice([\"strasse\"], case_sensitive=False)` "
                "accepts `\"STRASSE\"` and rejects `\"straße\"`."
            ),
            "edits": [{"op": "replace", "file": TYPES,
                       "old": "            normed_value = normed_value.casefold()\n",
                       "new": "            normed_value = normed_value.lower()\n"}],
            "test": '''import click
import pytest


def test_case_insensitive_matching_uses_lower_not_casefold():
    choice = click.Choice(["strasse", "Weg"], case_sensitive=False)
    assert choice.convert("STRASSE", None, None) == "strasse"
    assert choice.convert("weg", None, None) == "Weg"
    with pytest.raises(click.BadParameter):
        choice.convert("straße", None, None)
''',
        },
        {
            "id": "c5-ambiguous",
            "title": "Choice: refuse a value that matches two choices",
            "prompt": (
                "In `src/click/types.py`, `Choice.convert` returns the first choice when a value matches more than "
                "one choice (possible with `case_sensitive=False` and the choices `\"a\"` and `\"A\"`). Make it fail "
                "instead, with the message `'a' matches more than one choice: 'a', 'A'.` A value is valid only when "
                "it matches exactly one choice, as a whole."
            ),
            "edits": [{"op": "insert_before", "file": TYPES, "anchor": "        try:\n            return next(\n",
                       "text": ("        matches = [\n"
                                "            original\n"
                                "            for original, normalized in normalized_mapping.items()\n"
                                "            if normalized == normed_value\n"
                                "        ]\n"
                                "        if len(matches) > 1:\n"
                                "            self.fail(\n"
                                "                _(\"{value!r} matches more than one choice: {choices}.\").format(\n"
                                "                    value=value, choices=\", \".join(map(repr, matches))\n"
                                "                ),\n"
                                "                param=param,\n"
                                "                ctx=ctx,\n"
                                "            )\n\n")}],
            "test": '''import click
import pytest


def test_a_value_that_matches_two_choices_is_an_error():
    choice = click.Choice(["a", "A", "b"], case_sensitive=False)
    with pytest.raises(click.BadParameter) as e:
        choice.convert("a", None, None)
    assert str(e.value) == "'a' matches more than one choice: 'a', 'A'."
    assert choice.convert("B", None, None) == "b"
''',
        },
    ],
    "disjoint": [
        {
            "id": "d1-hex-colors",
            "title": "style: hex colours",
            "prompt": (
                "In `src/click/termui.py`, make `style` accept a hex colour string `#rrggbb` (hex digits in either "
                "case) for `fg` and `bg`, rendered as a 24-bit colour, the same as an RGB tuple. Example: "
                "`click.style(\"x\", fg=\"#ff8800\")` is `\"\\x1b[38;2;255;136;0mx\\x1b[0m\"`, and `bg=\"#0000FF\"` "
                "gives `\\x1b[48;2;0;0;255m`. Other strings that are not colour names stay an error."
            ),
            "edits": [{"op": "insert_before", "file": "src/click/termui.py",
                       "anchor": "    # bool is an int subclass: without the exclusion, True and False would\n",
                       "text": ("    elif (\n"
                                "        isinstance(color, str)\n"
                                "        and len(color) == 7\n"
                                "        and color[0] == \"#\"\n"
                                "        and all(c in \"0123456789abcdefABCDEF\" for c in color[1:])\n"
                                "    ):\n"
                                "        r, g, b = (int(color[i : i + 2], 16) for i in (1, 3, 5))\n"
                                "        return f\"{38 + offset};2;{r:d};{g:d};{b:d}\"\n\n")}],
            "test": '''import click
import pytest


def test_hex_colours():
    assert click.style("x", fg="#ff8800") == "\\x1b[38;2;255;136;0mx\\x1b[0m"
    assert click.style("x", bg="#0000FF") == "\\x1b[48;2;0;0;255mx\\x1b[0m"
    with pytest.raises(ValueError):
        click.style("x", fg="#12345")
    with pytest.raises(ValueError):
        click.style("x", fg="#gg0000")
''',
        },
        {
            "id": "d2-empty-xdg",
            "title": "get_app_dir: empty XDG_CONFIG_HOME",
            "prompt": (
                "In `src/click/utils.py`, `get_app_dir` on Linux and other Unix systems uses `XDG_CONFIG_HOME` even "
                "when it is set to an empty string, and then returns a relative path. Treat an empty "
                "`XDG_CONFIG_HOME` as unset, as the XDG Base Directory spec says: `get_app_dir(\"Foo Bar\")` then "
                "returns `~/.config/foo-bar` with `~` expanded."
            ),
            "edits": [{"op": "replace", "file": "src/click/utils.py",
                       "old": "        os.environ.get(\"XDG_CONFIG_HOME\", os.path.expanduser(\"~/.config\")),\n",
                       "new": "        os.environ.get(\"XDG_CONFIG_HOME\") or os.path.expanduser(\"~/.config\"),\n"}],
            "test": '''import os
import sys

import click


def test_an_empty_xdg_config_home_counts_as_unset(monkeypatch, tmp_path):
    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setattr(click.utils, "WIN", False)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("XDG_CONFIG_HOME", "")
    assert click.get_app_dir("Foo Bar") == os.path.join(str(tmp_path), ".config", "foo-bar")
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "cfg"))
    assert click.get_app_dir("Foo Bar") == os.path.join(str(tmp_path / "cfg"), "foo-bar")
''',
        },
        {
            "id": "d3-pwsh-value-newlines",
            "title": "PowerShell completion: newlines in the value",
            "prompt": (
                "In `src/click/shell_completion.py`, `PowerShellComplete.format_completion` collapses newlines in "
                "the help text but not in the value, so a value with a newline breaks the framing of three lines "
                "per item. Replace `\\r` and `\\n` in the value with spaces too. Example: for "
                "`CompletionItem(\"a\\nb\")` it returns `\"plain\\na b\\n_\"`."
            ),
            "edits": [{"op": "replace", "file": "src/click/shell_completion.py",
                       "old": "        return f\"{item.type}\\n{item.value}\\n{help_}\"\n",
                       "new": ("        value = str(item.value).replace(\"\\r\", \" \").replace(\"\\n\", \" \")\n"
                               "        return f\"{item.type}\\n{value}\\n{help_}\"\n")}],
            "test": '''import click
from click.shell_completion import CompletionItem
from click.shell_completion import PowerShellComplete


def test_newlines_in_the_value_are_collapsed():
    comp = PowerShellComplete(click.Command("cli"), {}, "cli", "_CLI_COMPLETE")
    assert comp.format_completion(CompletionItem("a\\nb")) == "plain\\na b\\n_"
    assert comp.format_completion(CompletionItem("x\\r\\ny", help="h")) == "plain\\nx  y\\nh"
''',
        },
        {
            "id": "d4-argument-dots",
            "title": "Argument: dots in the name",
            "prompt": (
                "In `src/click/core.py`, an argument declared with a dot in its name, such as "
                "`@click.argument(\"src.path\")`, gets the Python name `src.path`, which cannot be a keyword "
                "argument of the callback. Replace dots with underscores, the same way as dashes: "
                "`click.Argument([\"src.path\"]).name` is `\"src_path\"`."
            ),
            "edits": [{"op": "replace", "file": "src/click/core.py",
                       "old": "            name = name.replace(\"-\", \"_\").lower()\n",
                       "new": "            name = name.replace(\"-\", \"_\").replace(\".\", \"_\").lower()\n"}],
            "test": '''import click
from click.testing import CliRunner


def test_dots_in_an_argument_name_become_underscores():
    assert click.Argument(["src.path"]).name == "src_path"

    @click.command()
    @click.argument("src.path")
    def cli(src_path):
        click.echo(src_path)

    result = CliRunner().invoke(cli, ["a.txt"])
    assert result.exit_code == 0, result.output
    assert result.output == "a.txt\\n"
''',
        },
        {
            "id": "d5-blank-editor",
            "title": "Editor: skip a blank VISUAL or EDITOR",
            "prompt": (
                "In `src/click/_termui_impl.py`, `Editor.get_editor` returns the value of `VISUAL` or `EDITOR` even "
                "when it holds only whitespace, and starting it then fails. Skip a value that is empty after "
                "stripping whitespace and go on with the next variable. Example: with `VISUAL=\"  \"` and "
                "`EDITOR=\"nano\"`, `Editor().get_editor()` returns `\"nano\"`."
            ),
            "edits": [{"op": "replace", "file": "src/click/_termui_impl.py",
                       "old": "            rv = os.environ.get(key)\n            if rv:\n                return rv\n",
                       "new": "            rv = os.environ.get(key)\n            if rv and rv.strip():\n                return rv\n"}],
            "test": '''from click._termui_impl import Editor


def test_blank_editor_variables_are_skipped(monkeypatch):
    monkeypatch.setenv("VISUAL", "  ")
    monkeypatch.setenv("EDITOR", "nano")
    assert Editor().get_editor() == "nano"
    monkeypatch.setenv("VISUAL", "vim")
    assert Editor().get_editor() == "vim"
''',
        },
    ],
}

TASKS = {"click": CLICK}
SETS = ("disjoint", "colliding")
