"""The `long` development set: 10 tasks for 10 Claude Code agents on networkx (dev/README.md, "Long set").

A set for long agent work: a repository whose suite takes minutes, so an agent that tests waits, and tasks that
need reading of three modules. All ten work on the text formats of `networkx/readwrite`: the edge list, the
adjacency list and the multiline adjacency list (`edgelist.py`, `adjlist.py`, `multiline_adjlist.py`).

- Features l1 to l7 each change all three formats: quoted names (l1), a graph header line (l2), errors with line
  numbers (l3), sorted output (l4), a float format (l5), byte order marks (l6), line ends (l7).
- Refactors l8 to l10 move code that those features change, each into a new module with a class, and keep the
  public functions: l8 the parse functions into `LineParser` (l1, l2, l3, l7 change those bodies), l9 the generate
  functions into `LineWriter` (l1, l4, l5), l10 the decoding and encoding of the read and write functions into
  `TextFile` (l2, l6, l7).

The refactors come last in the reference order: their steps move the features' code into the new classes. The
steps are written for arms A, A2 and A3; they do not keep other tasks' lines word for word, so arm B and BN
oracle runs hold them (`undoes`). The data is in `long/`: `patches/<k>-<id>.alone.patch` (the task on the pin),
`<k>-<id>.step.patch` (the task on tasks 1..k-1, where it differs), `without-<k>.patch` (trees of all tasks but
k, written by hand), `tests/`, `integration/`.
"""

from pathlib import Path

HERE = Path(__file__).resolve().parent / "long"

SUFFIX = "\n\nKeep the existing behaviour otherwise. Finish the work completely.\n"


def ops_from_patch(text):
    """The edit ops of a `git diff` text: `replace` (hunk context and old lines -> context and new lines)
    for a changed file, `create` for a new one."""
    ops, path, new_file, old, new = [], None, False, None, None

    def flush():
        if old is None:
            return
        if new_file:
            ops.append({"op": "create", "file": path, "text": "".join(new)})
        else:
            ops.append({"op": "replace", "file": path, "old": "".join(old), "new": "".join(new)})

    for line in text.splitlines(True):
        if line.startswith("diff --git "):
            flush()
            old = new = None
            path, new_file = line.split(" b/", 1)[1].strip(), False
        elif line.startswith("new file mode"):
            new_file = True
        elif line.startswith(("--- ", "+++ ", "index ", "similarity", "rename ")) and old is None:
            continue
        elif line.startswith("@@"):
            if not new_file:
                flush()
                old, new = [], []
            elif old is None:
                old, new = [], []
        elif old is not None and line.startswith("\\"):
            continue
        elif old is not None:
            tag, body = line[:1], line[1:]
            if tag in (" ", "-"):
                old.append(body)
            if tag in (" ", "+"):
                new.append(body)
    flush()
    return ops


def patch(k, tid, kind):
    f = HERE / "patches" / f"{k}-{tid}.{kind}.patch"
    return f.read_text() if f.is_file() else None


PROMPTS = {
    'l1-quoted-names': (
        'Line formats: node names with spaces',
        'The edge list, adjacency list and multiline adjacency list formats '
        '(`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) write a node as `str(node)` and split every line at the '
        'delimiter, so a node named `New York` is written as two words and read back as two nodes. Add '
        'quoting to all three formats, in their `generate_*`, `write_*`, `parse_*` and `read_*` '
        'functions.\n\n- Writing: a node name (`str(node)`) is written in double quotes when it is empty, '
        'or contains any whitespace character, the delimiter of the call, `#`, a double quote or a '
        'backslash. Inside the quotes a backslash is written as `\\\\` and a double quote as `\\"`. Every '
        'other name is written as today (`plain`, `é-1`). Only node names are quoted; edge data is written '
        'as today.\n- Reading: where a line holds a node name (both names of an edge list line, every token '
        'of an adjacency list line, the name at the start of each line of a multiline adjacency list), a '
        'token that starts with `"` is a quoted name. It runs to the next `"` that is not escaped; `\\\\` '
        'and `\\"` inside it stand for `\\` and `"`, and the name is the text between the quotes (`""` is '
        'the empty name). A delimiter or the comments string inside the quotes is part of the name, not a '
        'split or a comment. A quote that is not closed raises `TypeError`. Tokens are separated by the '
        'delimiter, or by runs of whitespace when it is `None`, as today, and the rest of an edge list or '
        'multiline line after the names is the edge data, read as today.\n- So a graph with string nodes '
        'round-trips: `read_x(write_x(G))` has the nodes and edges of `G`.\n\nDocument the quoting in the '
        'module docstring of each of the three modules.',
    ),
    'l2-graph-header': (
        'Line formats: a header line with the graph type',
        'Files in the edge list, adjacency list and multiline adjacency list formats '
        '(`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) do not say whether the graph is directed or a '
        'multigraph, so the reader has to know and pass `create_using`. Add an optional header line.\n\n- '
        '`write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and `write_multiline_adjlist` take '
        '`header=False`. With `header=True` the first line they write is `<comments> networkx-graph: '
        'directed=<True|False> multigraph=<True|False>`, with the `comments` argument of the call: with the '
        'default `"#"` it is `# networkx-graph: directed=True multigraph=False` for a `DiGraph`. The '
        'comment lines that the adjacency list writers write today come after it. Without `header=True` the '
        'output is as today.\n- `parse_edgelist`, `parse_adjlist`, `parse_multiline_adjlist` and the '
        '`read_*` functions: when `create_using` is `None` and a header line comes before the first data '
        'line, the graph is a `Graph`, `DiGraph`, `MultiGraph` or `MultiDiGraph` as the header says. A '
        'header line is a line whose text, stripped, is the comments string followed by exactly the three '
        'words `networkx-graph:`, `directed=<True|False>` and `multigraph=<True|False>`; any other comment '
        'line is a comment as before, and so is a header line after the first data line. Other comment '
        'lines may come before the header. With `create_using` given the header changes nothing, and '
        '`comments=None` (edge list) means no header.\n\nDocument the parameter in each writer and the '
        'header in the module docstring of each of the three modules.',
    ),
    'l3-line-errors': (
        'Line formats: errors that say which line',
        'When `parse_edgelist`, `parse_adjlist` or `parse_multiline_adjlist` '
        '(`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) cannot read a line, they raise a `TypeError` that does '
        'not say which line, and the `read_*` functions do not say which file. Add an exception for '
        'this.\n\n- `networkx.LineParseError(message, lineno, line, filename=None)` in '
        '`networkx/exception.py` (listed in its `__all__`, so it is `nx.LineParseError`): a subclass of '
        '`NetworkXException` and of `TypeError`, so code that catches `TypeError` keeps working. Its '
        'attributes are `message`, `lineno`, `line` (the text of the line without its line end) and '
        '`filename`. `str()` of it is `line <lineno>: <message>`, or `<filename>:<lineno>: <message>` when '
        '`filename` is set (it may be set after the error is made).\n- Every `TypeError` that the three '
        '`parse_*` functions raise today about their input (a node, edge data or degree that does not '
        'convert, a multiline adjacency list that ends before all neighbours of a node) is a '
        "`LineParseError` instead, with today's text as its `message` and the original error as its "
        '`__cause__`. `lineno` counts every line given, from 1, comment and blank lines included; for the '
        'missing neighbours of a multiline adjacency list it is the line of the node. The `IndexError` '
        'about the number of data keys stays as it is.\n- `read_edgelist`, `read_weighted_edgelist`, '
        '`read_adjlist` and `read_multiline_adjlist` set `filename` of the error to the name of the file '
        'they read (the `name` of the file object, which is the path for a path or file name; `None` for a '
        'file object without a name) before it reaches the caller: `graph.txt:3: Failed to convert nodes '
        "x,3 to type <class 'int'>.`\n\nDocument the exception in the module docstring of each of the three "
        'modules.',
    ),
    'l4-sorted-output': (
        'Line formats: sorted output',
        'The edge list, adjacency list and multiline adjacency list writers '
        '(`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) write nodes and edges in the order in which they were '
        'added, so two equal graphs can give different files. Add a `sort` option.\n\n- '
        '`generate_edgelist`, `generate_adjlist`, `generate_multiline_adjlist` and the writers '
        '`write_edgelist`, `write_weighted_edgelist`, `write_adjlist`, `write_multiline_adjlist` take '
        '`sort=False`. With `sort=True` the lines do not depend on the order of insertion:\n  - nodes come '
        'in the order of `str(node)`, and within a line or block the neighbours too; the edge list is '
        'ordered by the names of the two ends;\n  - in an undirected graph each edge is written once, from '
        'the end whose `str` sorts first: the edge list writes `a b`, never `b a`, and an adjacency list '
        'lists the edge on the line (or in the block) of that end;\n  - parallel edges of a multigraph come '
        'in the order of `str(key)`;\n  - nodes or keys with the same `str` keep the order of the graph.\n- '
        'For example the undirected edges `(c, a), (b, a), (c, b), (a, a)` give the edge list `a a`, `a b`, '
        '`a c`, `b c` and the adjacency list `a a b c`, `b c`, `c`. For a directed graph with edges `(3, '
        '1), (1, 3), (2, 1), (10, 2)` the edge list is `1 3`, `10 2`, `2 1`, `3 1`.\n- Without `sort=True` '
        'the output is as today.\n\nDocument the parameter in each function and the option in the module '
        'docstring of each of the three modules.',
    ),
    'l5-float-format': (
        'Edge lists: a format for float data',
        'The edge list and multiline adjacency list writers (`networkx/readwrite/edgelist.py`, '
        "`networkx/readwrite/multiline_adjlist.py`) write float edge data with `repr`, `{'weight': "
        '0.3333333333333333}`. Add a `float_format` option.\n\n- `generate_edgelist`, '
        '`generate_multiline_adjlist`, `write_edgelist`, `write_weighted_edgelist` and '
        '`write_multiline_adjlist` take `float_format=None`: a format spec such as `".3g"`. With it, every '
        'value of the edge data that is a `float` (or a subclass of it) is written as `format(value, '
        'float_format)`:\n  - in a data dict (`data=True`, and the multiline adjacency list) the dict is '
        "written as today, `{'weight': 0.333, 'n': 2, 'name': 'x', 'ok': True}`: keys and the other values "
        'with `repr`, in the order of the dict;\n  - with a list of keys (`data=["weight"]`, '
        '`write_weighted_edgelist`) the bare value: `1 2 0.333`.\n- Ints, booleans, strings and other '
        'values are written as today, and without `float_format` the output is as today.\n\nDocument the '
        'parameter in each function and the option in the module docstrings of both modules.',
    ),
    'l6-byte-order-mark': (
        'Line formats: byte order marks',
        'The readers of the edge list, adjacency list and multiline adjacency list formats '
        '(`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) take a UTF-8 byte order mark at the start of a file as '
        'part of the first node name, and cannot read a UTF-16 file whose encoding the caller does not '
        'know. Handle byte order marks.\n\n- `read_edgelist`, `read_weighted_edgelist`, `read_adjlist` and '
        '`read_multiline_adjlist`: when the encoding is UTF-8 (in any spelling, such as the default '
        '`"utf-8"`), a UTF-8 byte order mark at the start of the file is skipped.\n- The same functions '
        'accept `encoding=None`: the encoding is then taken from the byte order mark at the start of the '
        'file (UTF-8, UTF-16 LE or UTF-16 BE), which is skipped, and is UTF-8 when the file has none.\n- '
        '`write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and `write_multiline_adjlist` take '
        '`bom=False`. With `bom=True` they write the byte order mark of their encoding before anything '
        'else, once: for UTF-8, UTF-16 LE and UTF-16 BE (in any spelling); for another encoding they raise '
        '`ValueError` before writing anything.\n\nDocument the parameters and the reading rule in the '
        'module docstring of each of the three modules.',
    ),
    'l7-line-ends': (
        'Line formats: line ends',
        'The readers of the edge list, adjacency list and multiline adjacency list formats '
        '(`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) remove only `\\n` from a line, so with a delimiter such '
        'as `,` a Windows file leaves `\\r` in the last name, and a file with old Mac line ends (`\\r` '
        'only) is read as one line. The writers always write `\\n`. Handle line ends.\n\n- Reading: in the '
        '`parse_*` and `read_*` functions a line may end with `\\n`, `\\r\\n` or `\\r`. Whatever a reader '
        'does with a line (comments, names, data, degrees), it does with the text without its line end, so '
        '`parse_adjlist(["a,b\\r\\n"], delimiter=",")` has the nodes `a` and `b`. The `read_*` functions '
        'split the file at every `\\r\\n`, `\\r` and `\\n`, so a file with `\\r` line ends is read line by '
        'line.\n- Writing: `write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and '
        '`write_multiline_adjlist` take `newline="\\n"` and end every line they write with it, comment '
        'lines included: `"\\r\\n"` for Windows files, `"\\r"` for old Mac files. Any other value raises '
        '`ValueError` before anything is written.\n\nDocument the parameter in each writer and the line-end '
        'rules in the module docstring of each of the three modules.',
    ),
    'l8-line-parser': (
        'Refactor: one parser for the line formats',
        '`parse_edgelist`, `parse_adjlist` and `parse_multiline_adjlist` (`networkx/readwrite/edgelist.py`, '
        '`networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) each remove '
        'comments, split lines and convert node names with their own copy of the same code. Move their '
        'bodies into a class `LineParser` in a new module `networkx/readwrite/_lineparser.py`, and add a '
        'paragraph about it to the module docstring of each of the three modules.\n\n- '
        '`LineParser(comments="#", delimiter=None, nodetype=None)`, kept as attributes of the same '
        'names.\n- `strip(line)` returns the line without its comment, exactly as the parse functions '
        'remove it (`LineParser().strip("a b # c")` is `"a b "`; with `comments=None` the line is returned '
        'as it is).\n- `split(line)` returns the tokens of a line whose comment is already removed, exactly '
        'as the parse functions split it: without its line end, split at the delimiter '
        '(`LineParser().split("a b\\n")` is `["a", "b"]`).\n- `node(text)` returns the node for the name '
        '`text`: `nodetype(text)`, or `text` when `nodetype` is `None`; an error of `nodetype` propagates '
        '(the parse methods turn it into the error they raise today).\n- `parse_edgelist(lines, '
        'create_using=None, data=True)`, `parse_adjlist(lines, create_using=None)` and '
        '`parse_multiline_adjlist(lines, create_using=None, edgetype=None)` return the graph that the '
        'module functions return. Every line they read goes through `self.strip` and `self.split`, and with '
        'a `nodetype` every node name goes through `self.node`, so a subclass that overrides one of these '
        'changes all three formats.\n\nThe module functions do nothing but `return LineParser(comments, '
        'delimiter, nodetype).parse_x(lines, ...)` with their other arguments. This holds for every '
        'argument: whatever parameters the parse functions have now or get later, the parse methods have '
        'them too, and `LineParser` takes those that all three share. Keep `parse_edgelist`, '
        '`parse_adjlist` and `parse_multiline_adjlist`, with their signatures, in their modules. This is a '
        'pure refactor: every function returns and raises exactly what it does today.',
    ),
    'l9-line-writer': (
        'Refactor: one writer for the line formats',
        '`generate_edgelist`, `generate_adjlist` and `generate_multiline_adjlist` '
        '(`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) each turn nodes into text with their own code. Move '
        'their bodies into a class `LineWriter` in a new module `networkx/readwrite/_linewriter.py`, and '
        'add a paragraph about it to the module docstring of each of the three modules.\n\n- '
        '`LineWriter(delimiter=" ")`, kept as the attribute `delimiter`.\n- `name(node)` returns the text '
        'of a node in a line, exactly as the generate functions write it (`str(node)` today).\n- '
        '`edgelist(G, data=True)`, `adjlist(G)` and `multiline_adjlist(G)` yield the lines that '
        '`generate_edgelist`, `generate_adjlist` and `generate_multiline_adjlist` yield; every node name '
        'they write is `self.name(node)`, so a subclass that overrides `name` changes all three '
        'formats.\n\nThe generate functions do nothing but return what the `LineWriter` method gives, '
        '`LineWriter(delimiter, ...).adjlist(G)`. This holds for every option: whatever parameters the '
        'generate functions have now or get later, other than `G` and the `data` of the edge list, '
        '`LineWriter` takes them as keyword arguments with the same names and defaults. Keep '
        '`generate_edgelist`, `generate_adjlist` and `generate_multiline_adjlist`, with their signatures, '
        'in their modules. This is a pure refactor: every function yields exactly the lines it yields '
        'today.',
    ),
    'l10-text-file': (
        'Refactor: the bytes of the line formats in one class',
        'The `read_*` and `write_*` functions of the edge list, adjacency list and multiline adjacency list '
        'formats (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and '
        '`networkx/readwrite/multiline_adjlist.py`) each decode or encode lines with their own code. Move '
        'that into a class `TextFile` in a new module `networkx/readwrite/_textfile.py`, and add a '
        'paragraph about it to the module docstring of each of the three modules.\n\n- '
        '`TextFile(encoding="utf-8")`, kept as the attribute `encoding`.\n- `read_lines(f)` returns the '
        'lines of the open file `f` as text, each with its line end, exactly as the `read_*` functions read '
        'them (`["a b\\n", "c d\\n"]`).\n- `write_lines(f, lines)` writes each of `lines` (text without its '
        'line end) to the binary file `f`, followed by a line end (`\\n` today), as the `write_*` functions '
        'write them.\n- `read_edgelist`, `read_weighted_edgelist`, `read_adjlist` and '
        '`read_multiline_adjlist` read every line only through `TextFile(...).read_lines(path)`, with their '
        '`encoding`. `write_edgelist`, `write_weighted_edgelist`, `write_adjlist` and '
        '`write_multiline_adjlist` write every line, the comment lines of the adjacency list writers '
        'included, only through one call of `TextFile(...).write_lines(path, lines)`.\n\nThis holds for '
        'every option about how the text is stored as bytes: `TextFile` takes the `encoding` today, and any '
        'later option of the `read_*` or `write_*` functions of that kind (such as a byte order mark or a '
        'line end) is a keyword argument of `TextFile` with the same name and default, used by `read_lines` '
        'or `write_lines`. Keep all eight `read_*` and `write_*` functions, with their signatures, in their '
        'modules. This is a pure refactor: every file is read and written exactly as today.',
    ),
}

IDS = list(PROMPTS)


def task(k, tid):
    title, prompt = PROMPTS[tid]
    t = {"id": tid, "title": title, "prompt": prompt,
         "edits": ops_from_patch(patch(k, tid, "alone")),
         "test": (HERE / "tests" / f"{tid.replace('-', '_')}.py").read_text()}
    step = patch(k, tid, "step")
    if step is not None:
        t["combined_edits"] = ops_from_patch(step)
    return t


NETWORKX_LONG = [task(k, tid) for k, tid in enumerate(IDS, 1)]

# Integration tests (1-based task numbers): a feature seen through the new class of a refactor, or two features
# across one interface. Each rests only on behaviour that the prompts of both tasks state.
INTEGRATION = [
    {"id": "i1-parser-quoted-names", "pair": [8, 1], "why": "refactor: l1's quoted names must be read by LineParser"},
    {"id": "i2-parser-graph-header", "pair": [8, 2], "why": "refactor: l2's header must be read by LineParser"},
    {"id": "i3-parser-line-errors", "pair": [8, 3], "why": "refactor: LineParser's methods raise l3's LineParseError"},
    {"id": "i4-parser-line-ends", "pair": [8, 7], "why": "refactor: LineParser.split drops l7's line ends"},
    {"id": "i5-writer-quoted-names", "pair": [9, 1], "why": "refactor: LineWriter.name quotes l1's names"},
    {"id": "i6-writer-sorted-output", "pair": [9, 4], "why": "refactor: l4's sort is a LineWriter option"},
    {"id": "i7-writer-float-format", "pair": [9, 5], "why": "refactor: l5's float_format is a LineWriter option"},
    {"id": "i8-file-graph-header", "pair": [10, 2], "why": "refactor: l2's header line goes through TextFile.write_lines"},
    {"id": "i9-file-byte-order-mark", "pair": [10, 6], "why": "refactor: l6's byte order marks are TextFile's"},
    {"id": "i10-file-line-ends", "pair": [10, 7], "why": "refactor: l7's line ends are TextFile's"},
    {"id": "i11-header-with-bom", "pair": [2, 6], "why": "interface: the header comes after l6's byte order mark"},
    {"id": "i12-header-line-ends", "pair": [2, 7], "why": "interface: the header line ends with l7's newline"},
]
for it in INTEGRATION:
    it["test"] = (HERE / "integration" / f"{it['id'].replace('-', '_')}.py").read_text()

# Set-level settings (tasks/networkx/long/set.json): 10 agents, all Claude Code. Limits for long work: the
# suite takes minutes and agents run it several times, so an agent may need an hour or more of wall time.
SETTINGS = {
    "agents": 10,
    "mix": ["claude"] * 10,
    "limits": {"max_turns": 600, "max_tokens": 40_000_000, "timeout_s": 7200, "claude_budget_usd": 15.0},
    "sequential": True,
}

# Trees of all tasks but k, written by hand (t2.replay_without gives broken trees: the refactor steps are written
# on the features' code).
WITHOUT = {"networkx": {"long": {int(f.name.split("-")[1].split(".")[0]): ops_from_patch(f.read_text())
                                 for f in sorted((HERE / "patches").glob("without-*.patch"))}}}

TASKS = {"networkx": {"long": NETWORKX_LONG}}
INTEGRATIONS = {"networkx": {"long": INTEGRATION}}
SET_SETTINGS = {"long": SETTINGS}
SETS = ("long",)
