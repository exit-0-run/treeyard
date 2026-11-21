from requests.utils import parse_header_links


def test_a_parameter_without_a_value_does_not_stop_parsing():
    assert parse_header_links("<http://a.example/f.css>; crossorigin; rel=preload") == [
        {"url": "http://a.example/f.css", "crossorigin": "", "rel": "preload"}
    ]
