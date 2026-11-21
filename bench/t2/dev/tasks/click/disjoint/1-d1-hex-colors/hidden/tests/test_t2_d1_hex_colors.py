import click
import pytest


def test_hex_colours():
    assert click.style("x", fg="#ff8800") == "\x1b[38;2;255;136;0mx\x1b[0m"
    assert click.style("x", bg="#0000FF") == "\x1b[48;2;0;0;255mx\x1b[0m"
    with pytest.raises(ValueError):
        click.style("x", fg="#12345")
    with pytest.raises(ValueError):
        click.style("x", fg="#gg0000")
