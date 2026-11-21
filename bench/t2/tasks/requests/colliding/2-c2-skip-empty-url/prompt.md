# parse_header_links: skip links with an empty URL

In `src/requests/utils.py`, make `parse_header_links` skip a link whose URL is empty. Example: `parse_header_links('<>; rel=self, <http://a.example/p2>; rel=next')` returns `[{"url": "http://a.example/p2", "rel": "next"}]`.

Keep the existing behaviour otherwise. Finish the work completely.
