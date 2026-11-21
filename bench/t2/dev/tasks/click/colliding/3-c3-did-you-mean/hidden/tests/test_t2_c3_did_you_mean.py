import click
import pytest


def test_the_error_names_the_closest_choice():
    choice = click.Choice(["red", "green"])
    with pytest.raises(click.BadParameter) as e:
        choice.convert("gren", None, None)
    assert str(e.value) == "'gren' is not one of 'red', 'green'. Did you mean 'green'?"
    with pytest.raises(click.BadParameter) as e:
        choice.convert("blue", None, None)
    assert str(e.value) == "'blue' is not one of 'red', 'green'."
