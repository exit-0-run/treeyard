import click
import pytest


def test_case_insensitive_matching_uses_lower_not_casefold():
    choice = click.Choice(["strasse", "Weg"], case_sensitive=False)
    assert choice.convert("STRASSE", None, None) == "strasse"
    assert choice.convert("weg", None, None) == "Weg"
    with pytest.raises(click.BadParameter):
        choice.convert("straße", None, None)
