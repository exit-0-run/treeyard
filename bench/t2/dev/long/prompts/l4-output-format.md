# Text formats: sorted output and a float format

The writers of the edge list, adjacency list, multiline adjacency list, Pajek and GML formats (`networkx/readwrite/edgelist.py`, `adjlist.py`, `multiline_adjlist.py`, `pajek.py` and `gml.py`) write nodes and edges in the order in which they were added, so two equal graphs can give different files, and they write float data with `repr` (`0.3333333333333333`). Add a `sort` option and a `float_format` option.

Sort:

- `generate_edgelist`, `generate_adjlist`, `generate_multiline_adjlist`, `generate_pajek`, `generate_gml` and the writers `write_edgelist`, `write_weighted_edgelist`, `write_adjlist`, `write_multiline_adjlist`, `write_pajek`, `write_gml` take `sort=False`. With `sort=True` the output does not depend on the order of insertion:
  - nodes come in the order of `str(node)`, and within a line or block the neighbours too; the edge list (and the edges of Pajek and GML) is ordered by the two ends, first end first;
  - in an undirected graph each edge is written once, from the end that sorts first: the edge list writes `a b`, never `b a`, and an adjacency list lists the edge on the line (or in the block) of that end;
  - parallel edges of a multigraph come in the order of `str(key)`;
  - nodes or keys that sort equal keep the order of the graph.
- `sort` may also be a function: then it is the key of the nodes instead of `str` (`sort=int` orders the nodes `1, 2, 10`), with the same rules. Keys of parallel edges still sort by `str(key)`.
- For example the undirected edges `(c, a), (b, a), (c, b), (a, a)` give the edge list `a a`, `a b`, `a c`, `b c` and the adjacency list `a a b c`, `b c`, `c`. For a directed graph with edges `(3, 1), (1, 3), (2, 1), (10, 2)` the edge list is `1 3`, `10 2`, `2 1`, `3 1`; with `sort=int` it is `1 3`, `2 1`, `3 1`, `10 2`.
- Pajek numbers the vertices in the sorted order; GML gives the nodes their ids (`0, 1, ...`) in the sorted order.

Float format:

- `generate_edgelist`, `generate_multiline_adjlist`, `generate_pajek`, `write_edgelist`, `write_weighted_edgelist`, `write_multiline_adjlist` and `write_pajek` take `float_format=None`: a format spec such as `".3g"`. With it, a value that is a `float` (or a subclass of it) is written as `format(value, float_format)`:
  - edge data in a data dict (`data=True`, and the multiline adjacency list): the dict is written as today, `{'weight': 0.333, 'n': 2, 'name': 'x', 'ok': True}`, keys and the other values with `repr`, in the order of the dict;
  - with a list of keys (`data=["weight"]`, `write_weighted_edgelist`) the bare value: `1 2 0.333`;
  - Pajek: the `x` and `y` of each vertex and the value (weight) of each edge: `1 a 0.12 0.00 ellipse`, `1 2 0.33`.
- Ints, booleans, strings and other values are written as today.

Without `sort` and `float_format` all output is as today. Document the parameters in each function and the options in the module docstrings.
