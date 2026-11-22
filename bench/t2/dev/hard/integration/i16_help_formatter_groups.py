import click
import click._help_formatter as module


def test_the_moved_formatter_lines_up_lists():
    assert click.formatting.HelpFormatter is module.HelpFormatter
    f = module.HelpFormatter(width=60)
    f.write_dl([("-a", "First.")], first_col_width=10)
    assert f.getvalue() == "-a          First.\n"
