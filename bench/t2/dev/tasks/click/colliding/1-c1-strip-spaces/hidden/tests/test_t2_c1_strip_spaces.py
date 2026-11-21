import click
import pytest


def test_spaces_around_the_value_are_ignored_without_case():
    choice = click.Choice(["red", "green"], case_sensitive=False)
    assert choice.convert("  Red ", None, None) == "red"
    assert choice.convert("green", None, None) == "green"
    strict = click.Choice(["red", "green"])
    with pytest.raises(click.BadParameter):
        strict.convert(" red", None, None)
