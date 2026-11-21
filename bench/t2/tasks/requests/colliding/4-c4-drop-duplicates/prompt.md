# parse_header_links: drop duplicate links

In `src/requests/utils.py`, make `parse_header_links` drop a link that is equal to a link earlier in the same header (same URL and same parameters). Example: `parse_header_links('<http://a.example/2>; rel=next, <http://a.example/2>; rel=next, <http://a.example/9>; rel=last')` returns `[{"url": "http://a.example/2", "rel": "next"}, {"url": "http://a.example/9", "rel": "last"}]`.

Keep the existing behaviour otherwise. Finish the work completely.
