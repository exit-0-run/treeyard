from requests.utils import parse_header_links


def test_duplicate_links_are_dropped():
    value = (
        "<http://a.example/2>; rel=next, <http://a.example/2>; rel=next, "
        "<http://a.example/9>; rel=last"
    )
    assert parse_header_links(value) == [
        {"url": "http://a.example/2", "rel": "next"},
        {"url": "http://a.example/9", "rel": "last"},
    ]
