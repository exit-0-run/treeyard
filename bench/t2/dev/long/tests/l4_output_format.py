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

def graph():
    G = nx.Graph()
    G.add_edge(1, 2, weight=1 / 3, n=2, name="x", ok=True)
    return G


def test_edgelist_dict_data():
    assert list(nx.generate_edgelist(graph(), float_format=".3g")) == ["1 2 {'weight': 0.333, 'n': 2, 'name': 'x', 'ok': True}"]
    assert list(nx.generate_edgelist(graph())) == [f"1 2 {graph()[1][2]}"]


def test_edgelist_keys():
    assert list(nx.generate_edgelist(graph(), data=["weight", "n"], float_format=".2f")) == ["1 2 0.33 2"]
    assert list(nx.generate_edgelist(graph(), data=["weight"])) == [f"1 2 {1 / 3}"]


def test_multiline():
    out = list(nx.generate_multiline_adjlist(graph(), float_format=".1f"))
    assert out == ["1 1", "2 {'weight': 0.3, 'n': 2, 'name': 'x', 'ok': True}", "2 0"]


def test_write_functions():
    b = io.BytesIO()
    nx.write_edgelist(graph(), b, float_format=".2g")
    assert b.getvalue() == b"1 2 {'weight': 0.33, 'n': 2, 'name': 'x', 'ok': True}\n"
    b = io.BytesIO()
    nx.write_weighted_edgelist(nx.Graph([(1, 2, {"weight": 2 / 3})]), b, float_format=".3f")
    assert b.getvalue() == b"1 2 0.667\n"
    b = io.BytesIO()
    nx.write_multiline_adjlist(graph(), b, float_format=".1e")
    assert "{'weight': 3.3e-01, 'n': 2, 'name': 'x', 'ok': True}" in b.getvalue().decode()


def test_round_trip_reads_the_numbers():
    b = io.BytesIO()
    nx.write_edgelist(graph(), b, float_format=".3g")
    H = nx.read_edgelist(io.BytesIO(b.getvalue()), nodetype=int)
    assert H[1][2]["weight"] == 0.333 and H[1][2]["n"] == 2


def test_sort_with_a_key_function():
    H = nx.Graph([(10, 2), (3, 1)])
    assert list(nx.generate_edgelist(H, data=False, sort=True)) == ["1 3", "10 2"]
    assert list(nx.generate_edgelist(H, data=False, sort=int)) == ["1 3", "2 10"]
    assert list(nx.generate_adjlist(H, sort=lambda n: -n)) == ["10 2", "3 1", "2", "1"]
    assert list(nx.generate_multiline_adjlist(H, sort=int))[:2] == ["1 1", "3 {}"]
    b = io.BytesIO()
    nx.write_edgelist(H, b, data=False, sort=int)
    assert b.getvalue() == b"1 3\n2 10\n"


def pajek_graph():
    G = nx.Graph()
    G.add_edges_from([("c", "a", {"weight": 1 / 3}), ("b", "a", {"weight": 2.0})])
    G.nodes["a"]["x"] = 0.123456
    return G


def test_pajek_sort_and_float_format():
    G = pajek_graph()
    assert list(nx.generate_pajek(G, sort=True, float_format=".2f")) == [
        "*vertices 3", "1 a 0.12 0.00 ellipse", "2 b 0.00 0.00 ellipse", "3 c 0.00 0.00 ellipse",
        "*edges", "1 2 2.00", "1 3 0.33"]
    assert list(nx.generate_pajek(G)) == [
        "*vertices 3", "1 c 0.0 0.0 ellipse", "2 a 0.123456 0.0 ellipse", "3 b 0.0 0.0 ellipse",
        "*edges", "1 2 0.3333333333333333", "2 3 2.0"]
    b = io.BytesIO()
    nx.write_pajek(G, b, sort=True, float_format=".1f")
    assert b.getvalue().decode().split("\n")[1] == "1 a 0.1 0.0 ellipse"
    D = nx.DiGraph([(3, 1), (1, 3), (2, 1)])
    assert list(nx.generate_pajek(D, sort=True))[-3:] == ["1 3 1.0", "2 1 1.0", "3 1 1.0"]


def test_gml_sort():
    G = nx.Graph([("c", "a"), ("b", "a")])
    lines = list(nx.generate_gml(G, sort=True))
    assert [x.strip() for x in lines if "label" in x] == ['label "a"', 'label "b"', 'label "c"']
    assert [x.strip() for x in lines if "source" in x or "target" in x] == [
        "source 0", "target 1", "source 0", "target 2"]
    H = nx.Graph([(10, 2), (3, 1)])
    assert [x.strip() for x in nx.generate_gml(H, sort=int) if "label" in x] == [
        'label "1"', 'label "2"', 'label "3"', 'label "10"']
    assert list(nx.generate_gml(G)) == list(nx.generate_gml(G, sort=False))
    b = io.BytesIO()
    nx.write_gml(G, b, sort=True)
    assert b.getvalue().decode().split("\n")[:4] == ["graph [", "  node [", "    id 0", '    label "a"']
