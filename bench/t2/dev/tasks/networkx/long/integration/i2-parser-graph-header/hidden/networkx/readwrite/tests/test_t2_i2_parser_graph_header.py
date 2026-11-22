import networkx as nx
from networkx.readwrite._lineparser import LineParser

HEADER = "# networkx-graph: directed=True multigraph=True"


def test_the_parser_reads_the_header():
    assert type(LineParser().parse_adjlist([HEADER, "1 2"])) is nx.MultiDiGraph
    assert type(LineParser().parse_edgelist([HEADER, "1 2"])) is nx.MultiDiGraph
    assert type(LineParser().parse_multiline_adjlist(iter([HEADER, "1 1", "2"]))) is nx.MultiDiGraph
    assert type(LineParser().parse_adjlist([HEADER, "1 2"], create_using=nx.Graph)) is nx.Graph
