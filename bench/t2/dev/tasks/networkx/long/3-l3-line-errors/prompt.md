# Line formats: errors that say which line

When `parse_edgelist`, `parse_adjlist` or `parse_multiline_adjlist` (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) cannot read a line, they raise a `TypeError` that does not say which line, and the `read_*` functions do not say which file. Add an exception for this.

- `networkx.LineParseError(message, lineno, line, filename=None)` in `networkx/exception.py` (listed in its `__all__`, so it is `nx.LineParseError`): a subclass of `NetworkXException` and of `TypeError`, so code that catches `TypeError` keeps working. Its attributes are `message`, `lineno`, `line` (the text of the line without its line end) and `filename`. `str()` of it is `line <lineno>: <message>`, or `<filename>:<lineno>: <message>` when `filename` is set (it may be set after the error is made).
- Every `TypeError` that the three `parse_*` functions raise today about their input (a node, edge data or degree that does not convert, a multiline adjacency list that ends before all neighbours of a node) is a `LineParseError` instead, with today's text as its `message` and the original error as its `__cause__`. `lineno` counts every line given, from 1, comment and blank lines included; for the missing neighbours of a multiline adjacency list it is the line of the node. The `IndexError` about the number of data keys stays as it is.
- `read_edgelist`, `read_weighted_edgelist`, `read_adjlist` and `read_multiline_adjlist` set `filename` of the error to the name of the file they read (the `name` of the file object, which is the path for a path or file name; `None` for a file object without a name) before it reaches the caller: `graph.txt:3: Failed to convert nodes x,3 to type <class 'int'>.`

Document the exception in the module docstring of each of the three modules.

Keep the existing behaviour otherwise. Finish the work completely.
