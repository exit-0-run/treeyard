import io

import networkx as nx


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
