# Edge lists: a format for float data

The edge list and multiline adjacency list writers (`networkx/readwrite/edgelist.py`, `networkx/readwrite/multiline_adjlist.py`) write float edge data with `repr`, `{'weight': 0.3333333333333333}`. Add a `float_format` option.

- `generate_edgelist`, `generate_multiline_adjlist`, `write_edgelist`, `write_weighted_edgelist` and `write_multiline_adjlist` take `float_format=None`: a format spec such as `".3g"`. With it, every value of the edge data that is a `float` (or a subclass of it) is written as `format(value, float_format)`:
  - in a data dict (`data=True`, and the multiline adjacency list) the dict is written as today, `{'weight': 0.333, 'n': 2, 'name': 'x', 'ok': True}`: keys and the other values with `repr`, in the order of the dict;
  - with a list of keys (`data=["weight"]`, `write_weighted_edgelist`) the bare value: `1 2 0.333`.
- Ints, booleans, strings and other values are written as today, and without `float_format` the output is as today.

Document the parameter in each function and the option in the module docstrings of both modules.

Keep the existing behaviour otherwise. Finish the work completely.
