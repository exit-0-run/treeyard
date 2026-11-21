import click


def test_a_linked_heading_gets_an_underline_of_its_text():
    f = click.HelpFormatter(heading_underline="-")
    f.write_heading(click.style("Docs", link="https://example.com"))
    assert f.getvalue().splitlines()[1] == "-----"
