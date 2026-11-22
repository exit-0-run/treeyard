import networkx as nx
from networkx.readwrite._linewriter import LineWriter


def test_the_writer_formats_floats():
    G = nx.Graph()
    G.add_edge(1, 2, weight=2 / 3)
    assert list(LineWriter(float_format=".2f").edgelist(G)) == ["1 2 {'weight': 0.67}"]
    assert list(LineWriter(float_format=".1f").edgelist(G, data=["weight"])) == ["1 2 0.7"]
    assert list(LineWriter(float_format=".1f").multiline_adjlist(G))[1] == "2 {'weight': 0.7}"
