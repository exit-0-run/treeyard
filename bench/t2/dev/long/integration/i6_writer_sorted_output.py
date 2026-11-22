import networkx as nx
from networkx.readwrite._linewriter import LineWriter


def test_the_writer_sorts():
    G = nx.Graph([("c", "a"), ("b", "a")])
    assert list(LineWriter(sort=True).adjlist(G)) == ["a b c", "b", "c"]
    assert list(LineWriter(sort=True).edgelist(G, data=False)) == ["a b", "a c"]
    assert list(LineWriter(sort=True).multiline_adjlist(G)) == ["a 2", "b {}", "c {}", "b 0", "c 0"]
