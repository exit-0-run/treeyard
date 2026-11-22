# Refactor: one parser for the line formats

`parse_edgelist`, `parse_adjlist` and `parse_multiline_adjlist` (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) each remove comments, split lines and convert node names with their own copy of the same code. Move their bodies into a class `LineParser` in a new module `networkx/readwrite/_lineparser.py`, and add a paragraph about it to the module docstring of each of the three modules.

- `LineParser(comments="#", delimiter=None, nodetype=None)`, kept as attributes of the same names.
- `strip(line)` returns the line without its comment, exactly as the parse functions remove it (`LineParser().strip("a b # c")` is `"a b "`; with `comments=None` the line is returned as it is).
- `split(line)` returns the tokens of a line whose comment is already removed, exactly as the parse functions split it: without its line end, split at the delimiter (`LineParser().split("a b\n")` is `["a", "b"]`).
- `node(text)` returns the node for the name `text`: `nodetype(text)`, or `text` when `nodetype` is `None`; an error of `nodetype` propagates (the parse methods turn it into the error they raise today).
- `parse_edgelist(lines, create_using=None, data=True)`, `parse_adjlist(lines, create_using=None)` and `parse_multiline_adjlist(lines, create_using=None, edgetype=None)` return the graph that the module functions return. Every line they read goes through `self.strip` and `self.split`, and with a `nodetype` every node name goes through `self.node`, so a subclass that overrides one of these changes all three formats.

The module functions do nothing but `return LineParser(comments, delimiter, nodetype).parse_x(lines, ...)` with their other arguments. This holds for every argument: whatever parameters the parse functions have now or get later, the parse methods have them too, and `LineParser` takes those that all three share. Keep `parse_edgelist`, `parse_adjlist` and `parse_multiline_adjlist`, with their signatures, in their modules. This is a pure refactor: every function returns and raises exactly what it does today, messages included.
