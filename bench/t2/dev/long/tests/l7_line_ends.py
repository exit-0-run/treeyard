import io

import pytest

import networkx as nx

FORMATS = ("edgelist", "adjlist", "multiline_adjlist")


@pytest.mark.parametrize("fmt", FORMATS)
@pytest.mark.parametrize("nl", ["\r\n", "\r", "\n"])
def test_round_trip_with_any_line_end(fmt, nl):
    G = nx.Graph([("a", "b"), ("b", "c")])
    b = io.BytesIO()
    getattr(nx, f"write_{fmt}")(G, b, delimiter=",", newline=nl)
    data = b.getvalue()
    assert data.count(nl.encode()) >= 2
    if nl != "\n":
        assert b"\n" not in data.replace(b"\r\n", b"")
    H = getattr(nx, f"read_{fmt}")(io.BytesIO(data), delimiter=",")
    assert set(H.nodes) == {"a", "b", "c"}
    assert H.number_of_edges() == 2


def test_parse_strips_crlf():
    assert set(nx.parse_adjlist(["a,b\r\n", "c,d\r\n"], delimiter=",").nodes) == {"a", "b", "c", "d"}
    assert list(nx.parse_edgelist(["a,b\r\n"], delimiter=",", data=False).edges) == [("a", "b")]
    G = nx.parse_multiline_adjlist(iter(["a,1\r\n", "b\r\n"]), delimiter=",")
    assert list(G.edges) == [("a", "b")]


def test_old_mac_files_are_read_line_by_line():
    H = nx.read_edgelist(io.BytesIO(b"1 2\r2 3\r3 4"), nodetype=int, data=False)
    assert sorted(H.edges) == [(1, 2), (2, 3), (3, 4)]
    H = nx.read_adjlist(io.BytesIO(b"1 2\r\n2 3\r"), nodetype=int)
    assert sorted(H.edges) == [(1, 2), (2, 3)]


def test_newline_values():
    b = io.BytesIO()
    nx.write_edgelist(nx.path_graph(2), b, data=False, newline="\r\n")
    assert b.getvalue() == b"0 1\r\n"
    b = io.BytesIO()
    nx.write_weighted_edgelist(nx.Graph([(1, 2, {"weight": 1.0})]), b, newline="\r")
    assert b.getvalue() == b"1 2 1.0\r"
    for fmt in FORMATS:
        with pytest.raises(ValueError):
            getattr(nx, f"write_{fmt}")(nx.path_graph(2), io.BytesIO(), newline="\n\n")


def test_comment_header_lines_use_the_newline():
    b = io.BytesIO()
    nx.write_adjlist(nx.path_graph(2), b, newline="\r\n")
    assert b"\n" not in b.getvalue().replace(b"\r\n", b"")
