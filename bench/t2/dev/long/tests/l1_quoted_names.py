import io

import pytest

import networkx as nx

FORMATS = ("edgelist", "adjlist", "multiline_adjlist")


def roundtrip(fmt, G, **kw):
    b = io.BytesIO()
    getattr(nx, f"write_{fmt}")(G, b, **kw)
    return b.getvalue().decode(), getattr(nx, f"read_{fmt}")(io.BytesIO(b.getvalue()), **kw)


def test_names_are_quoted_when_needed():
    G = nx.Graph()
    G.add_edge("New York", "Boston")
    assert list(nx.generate_edgelist(G, data=False)) == ['"New York" Boston']
    assert list(nx.generate_adjlist(G))[0] == '"New York" Boston'
    assert list(nx.generate_multiline_adjlist(G))[:2] == ['"New York" 1', "Boston {}"]


@pytest.mark.parametrize(
    ("name", "text"),
    [("", '""'), ("a b", '"a b"'), ("a\tb", '"a\tb"'), ("a#b", '"a#b"'), ('say "hi"', '"say \\"hi\\""'),
     ("back\\slash", '"back\\\\slash"'), ("plain", "plain"), ("é-1", "é-1")],
)
def test_the_quoting_rule(name, text):
    G = nx.Graph([(name, "x")])
    assert list(nx.generate_edgelist(G, data=False)) == [f"{text} x"]


def test_the_delimiter_is_quoted_too():
    G = nx.Graph([("a,b", "c")])
    assert list(nx.generate_edgelist(G, delimiter=",", data=False)) == ['"a,b",c']
    assert list(nx.generate_edgelist(G, data=False)) == ["a,b c"]
    assert list(nx.generate_adjlist(G, delimiter=","))[0] == '"a,b",c'


def test_readers_take_quoted_names():
    G = nx.parse_edgelist(['"New York" "Los Angeles" {"weight": 3}'], data=True)
    assert set(G.nodes) == {"New York", "Los Angeles"}
    assert G["New York"]["Los Angeles"] == {"weight": 3}
    G = nx.parse_adjlist(['"a b" c "d#e" # a comment', '"x\\"y" ""'])
    assert set(G.nodes) == {"a b", "c", "d#e", 'x"y', ""}
    G = nx.parse_edgelist(['"a,b",c'], delimiter=",", data=False)
    assert list(G.edges) == [("a,b", "c")]
    G = nx.parse_multiline_adjlist(iter(['"a b" 1', '"c d" {}']))
    assert list(G.edges) == [("a b", "c d")]


@pytest.mark.parametrize("fmt", FORMATS)
def test_round_trip(fmt):
    G = nx.Graph()
    G.add_edges_from([("New York", "Boston"), ("a#b", 'c"d'), ("", "e\\f"), ("p q", "p q")])
    text, H = roundtrip(fmt, G)
    assert '"New York"' in text
    assert set(H.nodes) == set(G.nodes)
    assert {frozenset(e) for e in H.edges} == {frozenset(e) for e in G.edges}


def test_other_names_and_data_as_before():
    G = nx.Graph()
    G.add_edge(1, 2, color="dark red")
    assert list(nx.generate_edgelist(G)) == ["1 2 {'color': 'dark red'}"]
    assert nx.parse_edgelist(["1 2 {'color': 'dark red'}"], nodetype=int)[1][2] == {"color": "dark red"}
    with pytest.raises(TypeError):
        nx.parse_adjlist(['"open a'])
