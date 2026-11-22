import io

import networkx as nx


def shuffled():
    G = nx.Graph()
    G.add_edges_from([("c", "a"), ("b", "a"), ("c", "b"), ("a", "a")])
    H = nx.Graph()
    H.add_edges_from([("a", "b"), ("a", "a"), ("b", "c"), ("a", "c")])
    return G, H


def test_edgelist_sorted():
    G, H = shuffled()
    want = ["a a", "a b", "a c", "b c"]
    assert list(nx.generate_edgelist(G, data=False, sort=True)) == want
    assert list(nx.generate_edgelist(H, data=False, sort=True)) == want


def test_adjlist_sorted():
    G, H = shuffled()
    want = ["a a b c", "b c", "c"]
    assert list(nx.generate_adjlist(G, sort=True)) == want
    assert list(nx.generate_adjlist(H, sort=True)) == want


def test_multiline_sorted():
    G, H = shuffled()
    want = ["a 3", "a {}", "b {}", "c {}", "b 1", "c {}", "c 0"]
    assert list(nx.generate_multiline_adjlist(G, sort=True)) == want
    assert list(nx.generate_multiline_adjlist(H, sort=True)) == want


def test_directed_and_multigraph():
    D = nx.DiGraph([(3, 1), (1, 3), (2, 1), (10, 2)])
    assert list(nx.generate_edgelist(D, data=False, sort=True)) == ["1 3", "10 2", "2 1", "3 1"]
    M = nx.MultiGraph()
    M.add_edge("b", "a", key="z", w=1)
    M.add_edge("a", "b", key="k", w=2)
    assert list(nx.generate_edgelist(M, data=["w"], sort=True)) == ["a b 2", "a b 1"]
    assert list(nx.generate_adjlist(M, sort=True)) == ["a b b", "b"]


def test_write_functions_take_sort():
    G, H = shuffled()
    for name in ("edgelist", "adjlist", "multiline_adjlist"):
        a, b = io.BytesIO(), io.BytesIO()
        getattr(nx, f"write_{name}")(G, a, sort=True)
        getattr(nx, f"write_{name}")(H, b, sort=True)
        lines = lambda x: [l for l in x.getvalue().decode().split("\n") if not l.startswith("#")]  # noqa: E731
        assert lines(a) == lines(b), name
    a, b = io.BytesIO(), io.BytesIO()
    nx.write_weighted_edgelist(nx.Graph([(2, 1, {"weight": 1.0})]), a, sort=True)
    assert a.getvalue() == b"1 2 1.0\n"


def test_without_sort_nothing_changes():
    G, _ = shuffled()
    assert list(nx.generate_edgelist(G, data=False)) == ["c a", "c b", "a b", "a a"]
