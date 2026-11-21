import click
import pytest


def test_a_unique_prefix_of_three_characters_is_accepted():
    choice = click.Choice(["production", "staging", "stage-2"])
    assert choice.convert("prod", None, None) == "production"
    assert choice.convert("produc", None, None) == "production"
    with pytest.raises(click.BadParameter):
        choice.convert("pr", None, None)
    with pytest.raises(click.BadParameter):
        choice.convert("sta", None, None)
