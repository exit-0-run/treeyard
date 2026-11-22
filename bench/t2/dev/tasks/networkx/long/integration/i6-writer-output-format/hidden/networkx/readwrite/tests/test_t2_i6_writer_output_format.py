import networkx as nx
from networkx.readwrite._linewriter import LineWriter


def test_the_writer_sorts():
    G = nx.Graph([("c", "a"), ("b", "a")])
    assert list(LineWriter(sort=True).adjlist(G)) == ["a b c", "b", "c"]
    assert list(LineWriter(sort=True).edgelist(G, data=False)) == ["a b", "a c"]
    assert list(LineWriter(sort=True).multiline_adjlist(G)) == ["a 2", "b {}", "c {}", "b 0", "c 0"]


def test_the_writer_formats_floats():
    G = nx.Graph()
    G.add_edge(1, 2, weight=2 / 3)
    assert list(LineWriter(float_format=".2f").edgelist(G)) == ["1 2 {'weight': 0.67}"]
    assert list(LineWriter(float_format=".1f").edgelist(G, data=["weight"])) == ["1 2 0.7"]
    assert list(LineWriter(float_format=".1f").multiline_adjlist(G))[1] == "2 {'weight': 0.7}"


def test_the_writer_sorts_and_formats_pajek():
    G = nx.Graph()
    G.add_edges_from([("c", "a", {"weight": 1 / 3}), ("b", "a", {"weight": 2.0})])
    assert list(LineWriter(sort=True, float_format=".2f").pajek(G))[-2:] == ["1 2 2.00", "1 3 0.33"]
