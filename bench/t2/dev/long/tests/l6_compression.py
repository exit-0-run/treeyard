import bz2
import gzip
import io
import lzma

import pytest

import networkx as nx

LINE = ("edgelist", "adjlist", "multiline_adjlist", "pajek")
ALL = LINE + ("gml", "graph6", "sparse6", "graphml", "gexf")
MAGIC = {"gzip": b"\x1f\x8b", "bz2": b"BZh", "xz": b"\xfd7zXZ\x00"}
DECOMPRESS = {"gzip": gzip.decompress, "bz2": bz2.decompress, "xz": lzma.decompress}


def write(fmt, G, f, **kw):
    getattr(nx, f"write_{fmt}")(G, f, **kw)


def read(fmt, f):
    H = getattr(nx, f"read_{fmt}")(f)
    return H


def edges(H):
    return sorted(tuple(sorted(map(str, e))) for e in H.edges())


@pytest.mark.parametrize("fmt", ALL)
@pytest.mark.parametrize("comp", ["gzip", "bz2", "xz"])
def test_compress_into_an_open_file(fmt, comp):
    G = nx.path_graph(3)
    b = io.BytesIO()
    write(fmt, G, b, compression=comp)
    data = b.getvalue()
    assert data.startswith(MAGIC[comp])
    plain = io.BytesIO()
    write(fmt, G, plain)
    if fmt not in ("adjlist", "multiline_adjlist"):  # their comment lines hold the time
        assert DECOMPRESS[comp](data) == plain.getvalue()
    assert not b.closed
    assert edges(read(fmt, io.BytesIO(data))) == [("0", "1"), ("1", "2")]


@pytest.mark.parametrize("fmt", ALL)
def test_compressed_files_are_read_whatever_their_name(fmt, tmp_path):
    G = nx.path_graph(3)
    for comp in ("gzip", "bz2", "xz"):
        b = io.BytesIO()
        write(fmt, G, b, compression=comp)
        p = tmp_path / f"g-{comp}.txt"
        p.write_bytes(b.getvalue())
        assert edges(read(fmt, p)) == [("0", "1"), ("1", "2")]
        assert edges(read(fmt, str(p))) == [("0", "1"), ("1", "2")]


@pytest.mark.parametrize("fmt", ALL)
def test_xz_endings(fmt, tmp_path):
    G = nx.path_graph(3)
    p = tmp_path / "g.xz"
    write(fmt, G, p)
    assert p.read_bytes().startswith(MAGIC["xz"])
    assert edges(read(fmt, p)) == [("0", "1"), ("1", "2")]


@pytest.mark.parametrize("fmt", ALL)
def test_bad_compression_values(fmt, tmp_path):
    G = nx.path_graph(3)
    with pytest.raises(ValueError):
        write(fmt, G, io.BytesIO(), compression="zip")
    for name in ("g.gz", "g.bz2", "g.xz", "g.gzip"):
        with pytest.raises(ValueError):
            write(fmt, G, tmp_path / name, compression="gzip")
    write(fmt, G, tmp_path / "g.txt", compression=None)
    assert edges(read(fmt, tmp_path / "g.txt")) == [("0", "1"), ("1", "2")]


def test_twice_compressed_and_plain_files():
    G = nx.path_graph(3)
    b = io.BytesIO()
    nx.write_edgelist(G, b)
    twice = gzip.compress(bz2.compress(b.getvalue()))
    assert edges(nx.read_edgelist(io.BytesIO(twice))) == [("0", "1"), ("1", "2")]
    assert edges(nx.read_edgelist(io.BytesIO(b.getvalue()))) == [("0", "1"), ("1", "2")]


def test_weighted_edgelist_and_leda():
    G = nx.Graph()
    G.add_edge(1, 2, weight=3.0)
    b = io.BytesIO()
    nx.write_weighted_edgelist(G, b, compression="xz")
    assert list(nx.read_weighted_edgelist(io.BytesIO(b.getvalue()), nodetype=int).edges(data="weight")) == [(1, 2, 3.0)]
    leda = b"LEDA.GRAPH\nstring\nint\n-1\n2\n|{a}|\n|{b}|\n1\n1 2 0 |{x}|\n"
    for comp in ("gzip", "bz2", "xz"):
        H = nx.read_leda(io.BytesIO({"gzip": gzip.compress, "bz2": bz2.compress, "xz": lzma.compress}[comp](leda)))
        assert list(H.edges(data=True)) == [("a", "b", {"label": "x"})]
