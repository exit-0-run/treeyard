from requests.utils import parse_header_links


def test_links_with_an_empty_url_are_skipped():
    assert parse_header_links("<>; rel=self, <http://a.example/p2>; rel=next") == [
        {"url": "http://a.example/p2", "rel": "next"}
    ]
