import enum

import click
from click._default_text import default_prompt_text


class Color(enum.Enum):
    RED = "r"
    GREEN = "g"


def test_the_prompt_text_of_an_enum_default():
    assert default_prompt_text(Color.GREEN, click.Choice(Color)) == "GREEN"
    assert default_prompt_text(Color.RED) == "RED"
