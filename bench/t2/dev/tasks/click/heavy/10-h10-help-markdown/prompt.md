# Help pages as Markdown

Projects copy their `--help` output into their docs by hand. Add a method `Command.get_help_markdown(ctx) -> str` and a formatter class `click.MarkdownHelpFormatter` (a subclass of `HelpFormatter` in `src/click/formatting.py`, exported from `click`, an `autoclass` entry in the Formatting section of `docs/api.md` after `HelpFormatter`, and a section "Help Pages as Markdown" at the end of `docs/documentation.md` with an example).

`get_help_markdown` writes a title line `# <command path>` (`# cli run`), then lets the command format its help as usual (`format_help`) with a `MarkdownHelpFormatter`. So the Markdown has every section of `--help`, in the same order, including sections that a command writes in its own `format_help` or `format_epilog`. The formatter writes:

- the usage line as ``Usage: `cli run [OPTIONS] [TARGET]` `` (the prefix, a space, the program and its arguments in backticks);
- each paragraph of a text (the help of the command, the epilog) as a paragraph, lines stripped, not wrapped;
- a section heading as `## Options`;
- a definition list (options, arguments, commands, any `write_dl`) as a table with the header `| Name | Description | Details |` and the line `| --- | --- | --- |`, then one row per entry: the term in backticks, the description, and the details: the items of the bracket at the end of the help line, as `--help` shows them between `[` and `]` (`default: 1; required`), empty when there is none. An empty cell is a single space between the bars (`| `--quiet` |  |  |`).

Blocks (title, usage, paragraphs, headings, tables) are separated by one blank line and the result ends with a newline. Style codes are removed everywhere (as `click.unstyle` does), whitespace inside a cell is collapsed to single spaces, and `|` in a cell is escaped as `\|`. `MarkdownHelpFormatter` also has `write_title(title)` (the `# title` line) and `split_help(text)`, which returns the description and the details of a help line: `split_help("Text.  [a; b]")` is `("Text.", "a; b")`, `split_help("Text.")` is `("Text.", "")`.

Keep the existing behaviour otherwise. Finish the work completely.
