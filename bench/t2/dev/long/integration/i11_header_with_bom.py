import codecs
import io

import networkx as nx


def test_a_header_after_a_byte_order_mark():
    for write, read in [(nx.write_edgelist, nx.read_edgelist), (nx.write_adjlist, nx.read_adjlist),
                        (nx.write_multiline_adjlist, nx.read_multiline_adjlist)]:
        b = io.BytesIO()
        write(nx.DiGraph([(1, 2)]), b, header=True, bom=True, encoding="utf-16-le")
        data = b.getvalue()
        assert data.startswith(codecs.BOM_UTF16_LE)
        assert data[2:].decode("utf-16-le").startswith("# networkx-graph: directed=True")
        assert type(read(io.BytesIO(data), encoding=None)) is nx.DiGraph, write.__name__
