from requests.utils import parse_header_links


def test_parameter_names_are_lower_case():
    assert parse_header_links('<http://a.example/>; REL=next; Type="text/html"') == [
        {"url": "http://a.example/", "rel": "next", "type": "text/html"}
    ]
