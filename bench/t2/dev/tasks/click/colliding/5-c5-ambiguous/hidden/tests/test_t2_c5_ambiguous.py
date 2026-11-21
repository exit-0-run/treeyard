import click
import pytest


def test_a_value_that_matches_two_choices_is_an_error():
    choice = click.Choice(["a", "A", "b"], case_sensitive=False)
    with pytest.raises(click.BadParameter) as e:
        choice.convert("a", None, None)
    assert str(e.value) == "'a' matches more than one choice: 'a', 'A'."
    assert choice.convert("B", None, None) == "b"
