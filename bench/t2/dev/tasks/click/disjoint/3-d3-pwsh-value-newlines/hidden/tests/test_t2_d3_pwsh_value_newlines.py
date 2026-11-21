import click
from click.shell_completion import CompletionItem
from click.shell_completion import PowerShellComplete


def test_newlines_in_the_value_are_collapsed():
    comp = PowerShellComplete(click.Command("cli"), {}, "cli", "_CLI_COMPLETE")
    assert comp.format_completion(CompletionItem("a\nb")) == "plain\na b\n_"
    assert comp.format_completion(CompletionItem("x\r\ny", help="h")) == "plain\nx  y\nh"
