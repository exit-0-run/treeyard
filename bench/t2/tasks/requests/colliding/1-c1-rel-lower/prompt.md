# parse_header_links: rel values in lower case

In `src/requests/utils.py`, make `parse_header_links` return the value of the `rel` parameter in lower case. Example: `parse_header_links('<http://a.example/p2>; rel="NEXT"')` returns `[{"url": "http://a.example/p2", "rel": "next"}]`. Other parameter values keep their case.

Keep the existing behaviour otherwise. Finish the work completely.
