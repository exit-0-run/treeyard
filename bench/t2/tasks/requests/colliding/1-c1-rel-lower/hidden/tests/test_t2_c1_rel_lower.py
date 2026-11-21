from requests.utils import parse_header_links


def test_rel_values_are_lower_case():
    assert parse_header_links('<http://a.example/p2>; rel="NEXT"') == [
        {"url": "http://a.example/p2", "rel": "next"}
    ]
    assert parse_header_links("<http://a.example/p1>; rel=Prev")[0]["rel"] == "prev"
