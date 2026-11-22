import networkx as nx
from networkx.readwrite._lineparser import LineParser


def test_the_parser_reads_quoted_names():
    G = LineParser().parse_edgelist(['"New York" Boston'], data=False)
    assert list(G.edges) == [("New York", "Boston")]
    assert set(LineParser().parse_adjlist(['"a b" "c#d" e']).nodes) == {"a b", "c#d", "e"}
    assert list(LineParser().parse_multiline_adjlist(iter(['"x y" 1', '"z w" {}'])).edges) == [("x y", "z w")]
    assert LineParser().strip('"a#b" c # x') == '"a#b" c '
