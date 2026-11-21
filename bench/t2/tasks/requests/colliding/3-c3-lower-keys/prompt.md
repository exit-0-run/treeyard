# parse_header_links: parameter names in lower case

In `src/requests/utils.py`, make `parse_header_links` return parameter names in lower case. Example: `parse_header_links('<http://a.example/>; REL=next; Type="text/html"')` returns `[{"url": "http://a.example/", "rel": "next", "type": "text/html"}]`. Parameter values keep their case.

Keep the existing behaviour otherwise. Finish the work completely.
