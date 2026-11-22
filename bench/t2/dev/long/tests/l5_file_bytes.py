import codecs
import io

import pytest

import networkx as nx

FORMATS = ("edgelist", "adjlist", "multiline_adjlist")


def text_of(fmt):
    b = io.BytesIO()
    getattr(nx, f"write_{fmt}")(nx.path_graph(3), b)
    return b.getvalue().decode()


@pytest.mark.parametrize("fmt", FORMATS)
def test_a_utf8_bom_is_skipped(fmt):
    data = codecs.BOM_UTF8 + text_of(fmt).encode()
    H = getattr(nx, f"read_{fmt}")(io.BytesIO(data), nodetype=int)
    assert sorted(H.nodes) == [0, 1, 2]


@pytest.mark.parametrize("fmt", FORMATS)
@pytest.mark.parametrize(("bom", "enc"), [(codecs.BOM_UTF8, "utf-8"), (codecs.BOM_UTF16_LE, "utf-16-le"),
                                          (codecs.BOM_UTF16_BE, "utf-16-be"), (b"", "utf-8")])
def test_encoding_none_detects_the_encoding(fmt, bom, enc):
    data = bom + text_of(fmt).encode(enc)
    H = getattr(nx, f"read_{fmt}")(io.BytesIO(data), nodetype=int, encoding=None)
    assert sorted(H.nodes) == [0, 1, 2]
    assert H.number_of_edges() == 2


@pytest.mark.parametrize("fmt", FORMATS)
def test_writers_write_a_bom(fmt):
    b = io.BytesIO()
    getattr(nx, f"write_{fmt}")(nx.path_graph(3), b, bom=True)
    assert b.getvalue().startswith(codecs.BOM_UTF8) and not b.getvalue()[3:].startswith(codecs.BOM_UTF8)
    b = io.BytesIO()
    getattr(nx, f"write_{fmt}")(nx.path_graph(3), b, bom=True, encoding="utf-16-le")
    assert b.getvalue().startswith(codecs.BOM_UTF16_LE)
    H = getattr(nx, f"read_{fmt}")(io.BytesIO(b.getvalue()), nodetype=int, encoding=None)
    assert sorted(H.nodes) == [0, 1, 2]
    with pytest.raises(ValueError):
        getattr(nx, f"write_{fmt}")(nx.path_graph(3), io.BytesIO(), bom=True, encoding="latin-1")


def test_weighted_and_defaults():
    b = io.BytesIO()
    nx.write_weighted_edgelist(nx.Graph([(1, 2, {"weight": 1.0})]), b, bom=True)
    assert b.getvalue() == codecs.BOM_UTF8 + b"1 2 1.0\n"
    H = nx.read_weighted_edgelist(io.BytesIO(b.getvalue()), nodetype=int)
    assert H[1][2]["weight"] == 1.0
    b = io.BytesIO()
    nx.write_edgelist(nx.path_graph(2), b, data=False)
    assert b.getvalue() == b"0 1\n"


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


PAJEK = "*vertices 2\n1 a\n2 b\n*edges\n1 2 1.0\n"
LEDA_TEXT = "LEDA.GRAPH\nstring\nint\n-1\n2\n|{a}|\n|{b}|\n1\n1 2 0 |{x}|\n"


@pytest.mark.parametrize("nl", ["\n", "\r\n", "\r"])
@pytest.mark.parametrize(("bom", "enc"), [(b"", "utf-8"), (codecs.BOM_UTF8, "utf-8"),
                                          (codecs.BOM_UTF16_LE, "utf-16-le"), (codecs.BOM_UTF16_BE, "utf-16-be")])
def test_pajek_and_leda_read_boms_and_line_ends(nl, bom, enc):
    data = bom + PAJEK.replace("\n", nl).encode(enc)
    G = nx.read_pajek(io.BytesIO(data), encoding=None)
    assert sorted(G) == ["a", "b"] and G.number_of_edges() == 1
    data = bom + LEDA_TEXT.replace("\n", nl).encode(enc)
    G = nx.read_leda(io.BytesIO(data), encoding=None)
    assert list(G.edges(data=True)) == [("a", "b", {"label": "x"})]
    if enc == "utf-8":
        assert sorted(nx.read_pajek(io.BytesIO(bom + PAJEK.replace("\n", nl).encode()))) == ["a", "b"]
        assert sorted(nx.read_leda(io.BytesIO(bom + LEDA_TEXT.replace("\n", nl).encode()))) == ["a", "b"]


def test_parse_pajek_strips_crlf():
    G = nx.parse_pajek(["*vertices 2\r\n", "1 a\r\n", "2 b\r\n", "*edges\r\n", "1 2 2.0\r\n"])
    assert sorted(G) == ["a", "b"] and list(G.edges(data="weight")) == [("a", "b", 2.0)]


@pytest.mark.parametrize("nl", ["\n", "\r\n", "\r"])
def test_write_pajek_bom_and_newline(nl):
    G = nx.path_graph(2)
    b = io.BytesIO()
    nx.write_pajek(G, b, bom=True, newline=nl, encoding="utf-16-le")
    data = b.getvalue()
    assert data.startswith(codecs.BOM_UTF16_LE)
    text = data[2:].decode("utf-16-le")
    assert text == nl.join(nx.generate_pajek(G)) + nl
    assert sorted(nx.read_pajek(io.BytesIO(data), encoding=None)) == ["0", "1"]
    b = io.BytesIO()
    nx.write_pajek(G, b)
    assert b.getvalue() == ("\n".join(nx.generate_pajek(G)) + "\n").encode()


@pytest.mark.parametrize("fmt", ["edgelist", "adjlist", "multiline_adjlist", "pajek"])
def test_a_bad_newline_writes_nothing(fmt):
    b = io.BytesIO()
    with pytest.raises(ValueError):
        getattr(nx, f"write_{fmt}")(nx.path_graph(2), b, bom=True, newline="\n\n")
    assert b.getvalue() == b""
    with pytest.raises(ValueError):
        getattr(nx, f"write_{fmt}")(nx.path_graph(2), b, bom=True, encoding="latin-1")
    assert b.getvalue() == b""
