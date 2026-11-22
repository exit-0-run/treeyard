# Line formats: node names with spaces

The edge list, adjacency list and multiline adjacency list formats (`networkx/readwrite/edgelist.py`, `networkx/readwrite/adjlist.py` and `networkx/readwrite/multiline_adjlist.py`) write a node as `str(node)` and split every line at the delimiter, so a node named `New York` is written as two words and read back as two nodes. Add quoting to all three formats, in their `generate_*`, `write_*`, `parse_*` and `read_*` functions.

- Writing: a node name (`str(node)`) is written in double quotes when it is empty, or contains any whitespace character, the delimiter of the call, `#`, a double quote or a backslash. Inside the quotes a backslash is written as `\\` and a double quote as `\"`. Every other name is written as today (`plain`, `é-1`). Only node names are quoted; edge data is written as today.
- Reading: where a line holds a node name (both names of an edge list line, every token of an adjacency list line, the name at the start of each line of a multiline adjacency list), a token that starts with `"` is a quoted name. It runs to the next `"` that is not escaped; `\\` and `\"` inside it stand for `\` and `"`, and the name is the text between the quotes (`""` is the empty name). A delimiter or the comments string inside the quotes is part of the name, not a split or a comment. A quote that is not closed raises `TypeError`. Tokens are separated by the delimiter, or by runs of whitespace when it is `None`, as today, and the rest of an edge list or multiline line after the names is the edge data, read as today.
- So a graph with string nodes round-trips: `read_x(write_x(G))` has the nodes and edges of `G`.

Document the quoting in the module docstring of each of the three modules.

Keep the existing behaviour otherwise. Finish the work completely.
