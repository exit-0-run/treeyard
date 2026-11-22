import io

import networkx as nx
from networkx.readwrite._textfile import TextFile


def test_the_header_is_written_through_the_file(monkeypatch):
    written = []
    real = TextFile.write_lines

    def spy(self, f, lines):
        lines = list(lines)
        written.append(lines)
        return real(self, f, lines)

    monkeypatch.setattr(TextFile, "write_lines", spy)
    G = nx.DiGraph([(1, 2)])
    for write in (nx.write_edgelist, nx.write_adjlist, nx.write_multiline_adjlist):
        write(G, io.BytesIO(), header=True)
        assert written[-1][0] == "# networkx-graph: directed=True multigraph=False", write.__name__
