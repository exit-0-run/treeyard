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
