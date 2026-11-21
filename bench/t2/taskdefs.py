"""The 30 T2 tasks: 3 repositories x 2 sets (disjoint, colliding) x 5 tasks.

Each task has a frozen prompt, a hidden deciding test (the runner adds it to the tree only at
scoring) and a list of edits that make the reference solution. `t2.py generate` turns the edits
into reference.patch (pinned commit -> task alone) and, for colliding sets, resolved-<k>.patch
(tasks 1..k-1 applied -> tasks 1..k applied, in the fixed oracle order 1..5).

Edit kinds:
- {"op": "replace", "file", "old", "new"}: `old` occurs once in the pinned file.
- {"op": "insert_before" | "insert_after", "file", "anchor", "text"}: inserts `text` next to the
  anchor. When several tasks of a set use the same anchor, their texts go there in task order;
  two branches that insert at one place give a real git conflict.

Colliding sets edit one function on purpose; disjoint sets edit 5 different files.
"""

SUFFIX = "\n\nKeep the existing behaviour otherwise. Finish the work completely.\n"

# ----------------------------------------------------------------------------------- requests

RQ_LINKS_ANCHOR = "        links.append(link)\n"

REQUESTS = {
    "colliding": [
        {
            "id": "c1-rel-lower",
            "title": "parse_header_links: rel values in lower case",
            "prompt": (
                "In `src/requests/utils.py`, make `parse_header_links` return the value of the `rel` "
                "parameter in lower case. Example: `parse_header_links('<http://a.example/p2>; rel=\"NEXT\"')` "
                "returns `[{\"url\": \"http://a.example/p2\", \"rel\": \"next\"}]`. Other parameter values keep their case."
            ),
            "edits": [{"op": "insert_before", "file": "src/requests/utils.py", "anchor": RQ_LINKS_ANCHOR,
                       "text": "        if \"rel\" in link:\n            link[\"rel\"] = link[\"rel\"].lower()\n"}],
            "test": '''from requests.utils import parse_header_links


def test_rel_values_are_lower_case():
    assert parse_header_links('<http://a.example/p2>; rel="NEXT"') == [
        {"url": "http://a.example/p2", "rel": "next"}
    ]
    assert parse_header_links("<http://a.example/p1>; rel=Prev")[0]["rel"] == "prev"
''',
        },
        {
            "id": "c2-skip-empty-url",
            "title": "parse_header_links: skip links with an empty URL",
            "prompt": (
                "In `src/requests/utils.py`, make `parse_header_links` skip a link whose URL is empty. "
                "Example: `parse_header_links('<>; rel=self, <http://a.example/p2>; rel=next')` returns "
                "`[{\"url\": \"http://a.example/p2\", \"rel\": \"next\"}]`."
            ),
            "edits": [{"op": "insert_before", "file": "src/requests/utils.py", "anchor": RQ_LINKS_ANCHOR,
                       "text": "        if not link[\"url\"]:\n            continue\n"}],
            "test": '''from requests.utils import parse_header_links


def test_links_with_an_empty_url_are_skipped():
    assert parse_header_links("<>; rel=self, <http://a.example/p2>; rel=next") == [
        {"url": "http://a.example/p2", "rel": "next"}
    ]
''',
        },
        {
            "id": "c3-lower-keys",
            "title": "parse_header_links: parameter names in lower case",
            "prompt": (
                "In `src/requests/utils.py`, make `parse_header_links` return parameter names in lower case. "
                "Example: `parse_header_links('<http://a.example/>; REL=next; Type=\"text/html\"')` returns "
                "`[{\"url\": \"http://a.example/\", \"rel\": \"next\", \"type\": \"text/html\"}]`. "
                "Parameter values keep their case."
            ),
            "edits": [{"op": "insert_before", "file": "src/requests/utils.py", "anchor": RQ_LINKS_ANCHOR,
                       "text": "        link = {key.lower(): val for key, val in link.items()}\n"}],
            "test": '''from requests.utils import parse_header_links


def test_parameter_names_are_lower_case():
    assert parse_header_links('<http://a.example/>; REL=next; Type="text/html"') == [
        {"url": "http://a.example/", "rel": "next", "type": "text/html"}
    ]
''',
        },
        {
            "id": "c4-drop-duplicates",
            "title": "parse_header_links: drop duplicate links",
            "prompt": (
                "In `src/requests/utils.py`, make `parse_header_links` drop a link that is equal to a link "
                "earlier in the same header (same URL and same parameters). Example: "
                "`parse_header_links('<http://a.example/2>; rel=next, <http://a.example/2>; rel=next, <http://a.example/9>; rel=last')` "
                "returns `[{\"url\": \"http://a.example/2\", \"rel\": \"next\"}, {\"url\": \"http://a.example/9\", \"rel\": \"last\"}]`."
            ),
            "edits": [{"op": "insert_before", "file": "src/requests/utils.py", "anchor": RQ_LINKS_ANCHOR,
                       "text": "        if link in links:\n            continue\n"}],
            "test": '''from requests.utils import parse_header_links


def test_duplicate_links_are_dropped():
    value = (
        "<http://a.example/2>; rel=next, <http://a.example/2>; rel=next, "
        "<http://a.example/9>; rel=last"
    )
    assert parse_header_links(value) == [
        {"url": "http://a.example/2", "rel": "next"},
        {"url": "http://a.example/9", "rel": "last"},
    ]
''',
        },
        {
            "id": "c5-valueless-params",
            "title": "parse_header_links: parameters without a value",
            "prompt": (
                "In `src/requests/utils.py`, `parse_header_links` stops at a parameter that has no `=`, so "
                "the parameters after it are lost. Make it keep such a parameter with the value `\"\"` and "
                "go on with the next ones. Example: "
                "`parse_header_links('<http://a.example/f.css>; crossorigin; rel=preload')` returns "
                "`[{\"url\": \"http://a.example/f.css\", \"crossorigin\": \"\", \"rel\": \"preload\"}]`."
            ),
            "edits": [{"op": "replace", "file": "src/requests/utils.py",
                       "old": "            try:\n                key, value = param.split(\"=\")\n            except ValueError:\n                break\n",
                       "new": "            if \"=\" not in param:\n                if param.strip(replace_chars):\n                    link[param.strip(replace_chars)] = \"\"\n                continue\n            key, value = param.split(\"=\", 1)\n"}],
            "test": '''from requests.utils import parse_header_links


def test_a_parameter_without_a_value_does_not_stop_parsing():
    assert parse_header_links("<http://a.example/f.css>; crossorigin; rel=preload") == [
        {"url": "http://a.example/f.css", "crossorigin": "", "rel": "preload"}
    ]
''',
        },
    ],
    "disjoint": [
        {
            "id": "d1-cid-union",
            "title": "CaseInsensitiveDict: | and |= operators",
            "prompt": (
                "In `src/requests/structures.py`, give `CaseInsensitiveDict` the `|` and `|=` operators of "
                "`dict`. `a | other` returns a new `CaseInsensitiveDict` with the items of `a` updated by the "
                "mapping `other` (keys still match without case); `a |= other` updates `a` in place."
            ),
            "edits": [{"op": "insert_before", "file": "src/requests/structures.py",
                       "anchor": "    def __repr__(self) -> str:\n        return str(dict(self.items()))\n",
                       "text": (
                           "    def __or__(self, other: object) -> CaseInsensitiveDict[_VT]:\n"
                           "        if not isinstance(other, Mapping):\n"
                           "            return NotImplemented\n"
                           "        new = self.copy()\n"
                           "        new.update(other)\n"
                           "        return new\n\n"
                           "    def __ior__(self, other: object) -> CaseInsensitiveDict[_VT]:\n"
                           "        if not isinstance(other, Mapping):\n"
                           "            return NotImplemented\n"
                           "        self.update(other)\n"
                           "        return self\n\n")}],
            "test": '''from requests.structures import CaseInsensitiveDict


def test_union_operators():
    a = CaseInsensitiveDict({"Accept": "text/html", "X-A": "1"})
    b = a | {"accept": "application/json", "X-B": "2"}
    assert isinstance(b, CaseInsensitiveDict)
    assert b["ACCEPT"] == "application/json"
    assert b["x-b"] == "2" and b["x-a"] == "1"
    assert a["accept"] == "text/html"
    a |= {"X-C": "3"}
    assert a["x-c"] == "3"
''',
        },
        {
            "id": "d2-status-reason",
            "title": "status_codes: reason(code)",
            "prompt": (
                "In `src/requests/status_codes.py`, add a function `reason(code)` that returns the first name "
                "of an HTTP status code, or `None` for a code that is not known. Examples: `reason(404)` is "
                "`\"not_found\"`, `reason(200)` is `\"ok\"`, `reason(999)` is `None`."
            ),
            "edits": [{"op": "insert_before", "file": "src/requests/status_codes.py",
                       "anchor": "\n_init()\n",
                       "text": (
                           "\ndef reason(code: int) -> str | None:\n"
                           "    \"\"\"Return the first name of an HTTP status code, e.g. ``reason(404) == \"not_found\"``.\"\"\"\n"
                           "    titles = _codes.get(code)\n"
                           "    return titles[0] if titles else None\n\n")}],
            "test": '''from requests.status_codes import reason


def test_reason_returns_the_first_name():
    assert reason(404) == "not_found"
    assert reason(200) == "ok"
    assert reason(999) is None
''',
        },
        {
            "id": "d3-basic-auth-repr",
            "title": "HTTPBasicAuth: repr without the password",
            "prompt": (
                "In `src/requests/auth.py`, give `HTTPBasicAuth` a `repr` that does not show the password: "
                "`repr(HTTPBasicAuth(\"user\", \"pw\"))` is `HTTPBasicAuth(username='user', password='***')`."
            ),
            "edits": [{"op": "insert_after", "file": "src/requests/auth.py",
                       "anchor": "    def __ne__(self, other: Any) -> bool:\n        return not self == other\n\n    def __call__(self, r: PreparedRequest) -> PreparedRequest:\n        r.headers[\"Authorization\"] = _basic_auth_str(self.username, self.password)\n        return r\n",
                       "text": (
                           "\n    def __repr__(self) -> str:\n"
                           "        return f\"{type(self).__name__}(username={self.username!r}, password='***')\"\n")}],
            "test": '''from requests.auth import HTTPBasicAuth


def test_repr_hides_the_password():
    assert repr(HTTPBasicAuth("user", "pw")) == "HTTPBasicAuth(username='user', password='***')"
    assert "pw" not in repr(HTTPBasicAuth("someone", "pw"))
''',
        },
        {
            "id": "d4-netmask-range",
            "title": "dotted_netmask: refuse masks outside 0..32",
            "prompt": (
                "In `src/requests/utils.py`, make `dotted_netmask(mask)` raise `ValueError` when `mask` is not "
                "between 0 and 32. Today `dotted_netmask(-1)` returns a wrong value. `dotted_netmask(24)` stays "
                "`\"255.255.255.0\"`."
            ),
            "edits": [{"op": "replace", "file": "src/requests/utils.py",
                       "old": "    bits = 0xFFFFFFFF ^ (1 << 32 - mask) - 1\n",
                       "new": "    if not 0 <= mask <= 32:\n        raise ValueError(f\"mask must be between 0 and 32, not {mask}\")\n    bits = 0xFFFFFFFF ^ (1 << 32 - mask) - 1\n"}],
            "test": '''import pytest

from requests.utils import dotted_netmask


def test_mask_outside_the_range_is_refused():
    with pytest.raises(ValueError):
        dotted_netmask(-1)
    with pytest.raises(ValueError):
        dotted_netmask(33)
    assert dotted_netmask(24) == "255.255.255.0"
    assert dotted_netmask(0) == "0.0.0.0"
''',
        },
        {
            "id": "d5-native-memoryview",
            "title": "to_native_string: accept memoryview",
            "prompt": (
                "In `src/requests/_internal_utils.py`, make `to_native_string` accept a `memoryview` of bytes "
                "too: `to_native_string(memoryview(b\"abc\"))` returns `\"abc\"`."
            ),
            "edits": [{"op": "replace", "file": "src/requests/_internal_utils.py",
                       "old": "    if isinstance(string, builtin_str):\n        out = string\n    else:\n",
                       "new": "    if isinstance(string, builtin_str):\n        out = string\n    elif isinstance(string, memoryview):\n        out = string.tobytes().decode(encoding)\n    else:\n"}],
            "test": '''from requests._internal_utils import to_native_string


def test_memoryview_is_accepted():
    assert to_native_string(memoryview(b"abc")) == "abc"
    assert to_native_string(b"xyz") == "xyz"
''',
        },
    ],
}

# ----------------------------------------------------------------------------------- ripgrep

RG_HUMAN = "crates/cli/src/human.rs"
RG_ARM_ANCHOR = "        \"G\" => value.checked_mul(1 << 30),\n"


def _rg_size_test(name, body):
    return "use grep_cli::parse_human_readable_size;\n\n#[test]\nfn " + name + "() {\n" + body + "}\n"


RIPGREP = {
    "colliding": [
        {
            "id": "c1-size-lowercase",
            "title": "parse_human_readable_size: lower-case suffixes",
            "prompt": (
                "In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffixes `k`, `m` "
                "and `g` in lower case, with the same meaning as `K`, `M` and `G`. Example: `\"2k\"` is 2048."
            ),
            "edits": [{"op": "insert_after", "file": RG_HUMAN, "anchor": RG_ARM_ANCHOR,
                       "text": "        \"k\" => value.checked_mul(1 << 10),\n        \"m\" => value.checked_mul(1 << 20),\n        \"g\" => value.checked_mul(1 << 30),\n"}],
            "test_crate": "grep-cli",
            "test": _rg_size_test("lower_case_suffixes",
                                  "    assert_eq!(parse_human_readable_size(\"2k\").unwrap(), 2 << 10);\n"
                                  "    assert_eq!(parse_human_readable_size(\"3m\").unwrap(), 3 << 20);\n"
                                  "    assert_eq!(parse_human_readable_size(\"4g\").unwrap(), 4 << 30);\n"),
        },
        {
            "id": "c2-size-terabytes",
            "title": "parse_human_readable_size: T suffix",
            "prompt": (
                "In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffix `T` "
                "(terabytes, 2^40 bytes). Example: `\"2T\"` is 2199023255552. An overflow stays an error."
            ),
            "edits": [{"op": "insert_after", "file": RG_HUMAN, "anchor": RG_ARM_ANCHOR,
                       "text": "        \"T\" => value.checked_mul(1 << 40),\n"},
                      # The crate's own unit test used T as the example of an unknown suffix.
                      {"op": "replace", "file": RG_HUMAN,
                       "old": "        assert!(parse_human_readable_size(\"123T\").is_err());\n",
                       "new": "        assert!(parse_human_readable_size(\"123X\").is_err());\n"}],
            "test_crate": "grep-cli",
            "test": _rg_size_test("terabytes",
                                  "    assert_eq!(parse_human_readable_size(\"2T\").unwrap(), 2u64 << 40);\n"
                                  "    assert!(parse_human_readable_size(\"99999999T\").is_err());\n"),
        },
        {
            "id": "c3-size-bytes",
            "title": "parse_human_readable_size: B suffix",
            "prompt": (
                "In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffix `B` for bytes. "
                "Example: `\"100B\"` is 100, the same as `\"100\"`."
            ),
            "edits": [{"op": "insert_after", "file": RG_HUMAN, "anchor": RG_ARM_ANCHOR,
                       "text": "        \"B\" => Some(value),\n"}],
            "test_crate": "grep-cli",
            "test": _rg_size_test("bytes_suffix",
                                  "    assert_eq!(parse_human_readable_size(\"100B\").unwrap(), 100);\n"),
        },
        {
            "id": "c4-size-space",
            "title": "parse_human_readable_size: space before the suffix",
            "prompt": (
                "In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept spaces between the number "
                "and the suffix. Example: `\"2 M\"` is 2097152, the same as `\"2M\"`."
            ),
            "edits": [{"op": "replace", "file": RG_HUMAN,
                       "old": "    let suffix = &size[digits_end..];\n",
                       "new": "    let suffix = size[digits_end..].trim_start();\n"}],
            "test_crate": "grep-cli",
            "test": _rg_size_test("space_before_suffix",
                                  "    assert_eq!(parse_human_readable_size(\"2 M\").unwrap(), 2 << 20);\n"
                                  "    assert_eq!(parse_human_readable_size(\"7  K\").unwrap(), 7 << 10);\n"),
        },
        {
            "id": "c5-size-binary",
            "title": "parse_human_readable_size: KiB, MiB and GiB",
            "prompt": (
                "In `crates/cli/src/human.rs`, make `parse_human_readable_size` accept the suffixes `KiB`, `MiB` "
                "and `GiB`, with the same meaning as `K`, `M` and `G`. Example: `\"2KiB\"` is 2048."
            ),
            "edits": [{"op": "insert_after", "file": RG_HUMAN, "anchor": RG_ARM_ANCHOR,
                       "text": "        \"KiB\" => value.checked_mul(1 << 10),\n        \"MiB\" => value.checked_mul(1 << 20),\n        \"GiB\" => value.checked_mul(1 << 30),\n"}],
            "test_crate": "grep-cli",
            "test": _rg_size_test("binary_suffixes",
                                  "    assert_eq!(parse_human_readable_size(\"2KiB\").unwrap(), 2 << 10);\n"
                                  "    assert_eq!(parse_human_readable_size(\"1MiB\").unwrap(), 1 << 20);\n"
                                  "    assert_eq!(parse_human_readable_size(\"1GiB\").unwrap(), 1 << 30);\n"),
        },
    ],
    "disjoint": [
        {
            "id": "d1-type-roc",
            "title": "ignore: file type roc",
            "prompt": (
                "In `crates/ignore/src/default_types.rs`, add a default file type `roc` for files that end in "
                "`.roc` (the Roc language). Keep the list in alphabetical order."
            ),
            "edits": [{"op": "insert_before", "file": "crates/ignore/src/default_types.rs",
                       "anchor": "    (&[\"rocq\"], &[\"*.v\"]),\n",
                       "text": "    (&[\"roc\"], &[\"*.roc\"]),\n"}],
            "test_crate": "ignore",
            "test": '''use ignore::types::TypesBuilder;

#[test]
fn roc_file_type() {
    let mut builder = TypesBuilder::new();
    builder.add_defaults();
    builder.select("roc");
    let types = builder.build().unwrap();
    assert!(types.matched("main.roc", false).is_whitelist());
    assert!(!types.matched("main.rs", false).is_whitelist());
}
''',
        },
        {
            "id": "d2-globset-first-match",
            "title": "globset: GlobSet::first_match",
            "prompt": (
                "In `crates/globset/src/lib.rs`, add a method `GlobSet::first_match(path) -> Option<usize>` that "
                "returns the sequence number of the first glob (in the order they were added) that matches the "
                "path, or `None` when no glob matches."
            ),
            "edits": [{"op": "insert_before", "file": "crates/globset/src/lib.rs",
                       "anchor": "    /// Returns the sequence number of every glob pattern that matches the\n    /// given path.\n    pub fn matches<P: AsRef<Path>>(&self, path: P) -> Vec<usize> {\n",
                       "text": (
                           "    /// Returns the sequence number of the first glob pattern (in the order\n"
                           "    /// they were added) that matches the given path.\n"
                           "    pub fn first_match<P: AsRef<Path>>(&self, path: P) -> Option<usize> {\n"
                           "        self.matches(path).into_iter().min()\n"
                           "    }\n\n")}],
            "test_crate": "globset",
            "test": '''use globset::{Glob, GlobSetBuilder};

#[test]
fn first_match() {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new("*.rs").unwrap());
    builder.add(Glob::new("src/**").unwrap());
    let set = builder.build().unwrap();
    assert_eq!(set.first_match("src/lib.rs"), Some(0));
    assert_eq!(set.first_match("src/a.txt"), Some(1));
    assert_eq!(set.first_match("x.txt"), None);
}
''',
        },
        {
            "id": "d3-stats-without-match",
            "title": "printer: Stats::searches_without_match",
            "prompt": (
                "In `crates/printer/src/stats.rs`, add a method `Stats::searches_without_match() -> u64` that "
                "returns the number of searches that found no match (all searches minus the searches with a match)."
            ),
            "edits": [{"op": "insert_after", "file": "crates/printer/src/stats.rs",
                       "anchor": "    pub fn searches_with_match(&self) -> u64 {\n        self.searches_with_match\n    }\n",
                       "text": (
                           "\n    /// Return the total number of searches that found no match.\n"
                           "    pub fn searches_without_match(&self) -> u64 {\n"
                           "        self.searches.saturating_sub(self.searches_with_match)\n"
                           "    }\n")}],
            "test_crate": "grep-printer",
            "test": '''use grep_printer::Stats;

#[test]
fn searches_without_match() {
    let mut stats = Stats::new();
    stats.add_searches(5);
    stats.add_searches_with_match(2);
    assert_eq!(stats.searches_without_match(), 3);
    assert_eq!(Stats::new().searches_without_match(), 0);
}
''',
        },
        {
            "id": "d4-match-overlaps",
            "title": "matcher: Match::overlaps",
            "prompt": (
                "In `crates/matcher/src/lib.rs`, add a method `Match::overlaps(&self, other: &Match) -> bool` that "
                "returns true when the two matches share at least one byte offset. An empty match overlaps "
                "nothing. Example: 0..5 and 4..8 overlap; 0..4 and 4..8 do not."
            ),
            "edits": [{"op": "insert_after", "file": "crates/matcher/src/lib.rs",
                       "anchor": "    /// Returns true if and only if this match is empty.\n    #[inline]\n    pub fn is_empty(&self) -> bool {\n        self.len() == 0\n    }\n",
                       "text": (
                           "\n    /// Returns true if and only if this match and `other` share at least\n"
                           "    /// one byte offset. An empty match overlaps nothing.\n"
                           "    #[inline]\n"
                           "    pub fn overlaps(&self, other: &Match) -> bool {\n"
                           "        !self.is_empty()\n"
                           "            && !other.is_empty()\n"
                           "            && self.start < other.end\n"
                           "            && other.start < self.end\n"
                           "    }\n")}],
            # grep-matcher sets `autotests = false`, so the hidden test lives in a crate that uses it.
            "test_crate": "grep-searcher",
            "test": '''use grep_matcher::Match;

#[test]
fn match_overlaps() {
    assert!(Match::new(0, 5).overlaps(&Match::new(4, 8)));
    assert!(!Match::new(0, 4).overlaps(&Match::new(4, 8)));
    assert!(!Match::zero(2).overlaps(&Match::new(0, 5)));
}
''',
        },
        {
            "id": "d5-glob-has-wildcards",
            "title": "globset: Glob::has_wildcards",
            "prompt": (
                "In `crates/globset/src/glob.rs`, add a method `Glob::has_wildcards() -> bool` that returns true "
                "when the pattern has a wildcard (`*`, `?`, `**`), a character class (`[...]`) or an "
                "alternation (`{a,b}`), and false when it can match only one literal path."
            ),
            "edits": [{"op": "insert_after", "file": "crates/globset/src/glob.rs",
                       "anchor": "    /// Returns the original glob pattern used to build this pattern.\n    pub fn glob(&self) -> &str {\n        &self.glob\n    }\n",
                       "text": (
                           "\n    /// Returns true if and only if this pattern has a wildcard, a character\n"
                           "    /// class or an alternation.\n"
                           "    pub fn has_wildcards(&self) -> bool {\n"
                           "        self.tokens.iter().any(|t| !matches!(t, Token::Literal(_)))\n"
                           "    }\n")}],
            "test_crate": "globset",
            "test": '''use globset::Glob;

#[test]
fn has_wildcards() {
    assert!(Glob::new("src/*.rs").unwrap().has_wildcards());
    assert!(Glob::new("{a,b}").unwrap().has_wildcards());
    assert!(Glob::new("[ab].txt").unwrap().has_wildcards());
    assert!(Glob::new("**/x").unwrap().has_wildcards());
    assert!(!Glob::new("src/main.rs").unwrap().has_wildcards());
}
''',
        },
    ],
}

# ----------------------------------------------------------------------------------- astro

AS_PATH = "packages/internal-helpers/src/path.ts"
AS_EXT_ANCHOR = "export function fileExtension(path: string) {\n"


def _as_test(module, names, body):
    return (
        "import assert from 'node:assert/strict';\n"
        "import { it } from 'node:test';\n"
        f"import {{ {names} }} from '../src/{module}.ts';\n\n" + body
    )


ASTRO = {
    "colliding": [
        {
            "id": "c1-ext-query",
            "title": "fileExtension: ignore the query and the hash",
            "prompt": (
                "In `packages/internal-helpers/src/path.ts`, make `fileExtension` ignore a query string and a "
                "hash. Examples: `fileExtension('/a/b.js?v=1')` is `'.js'`, `fileExtension('/x/y.css#top')` is `'.css'`."
            ),
            "edits": [{"op": "insert_after", "file": AS_PATH, "anchor": AS_EXT_ANCHOR,
                       "text": "\tpath = path.replace(/[?#].*$/, '');\n"}],
            "test": _as_test("path", "fileExtension",
                             "it('ignores the query and the hash', () => {\n"
                             "\tassert.equal(fileExtension('/a/b.js?v=1'), '.js');\n"
                             "\tassert.equal(fileExtension('/x/y.css#top'), '.css');\n"
                             "});\n"),
        },
        {
            "id": "c2-ext-dotfile",
            "title": "fileExtension: a dotfile has no extension",
            "prompt": (
                "In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `''` for a file whose "
                "name starts with a dot and has no other dot. Examples: `fileExtension('.gitignore')` and "
                "`fileExtension('/a/.env')` are `''`; `fileExtension('a/.eslintrc.json')` stays `'.json'`."
            ),
            "edits": [{"op": "insert_after", "file": AS_PATH, "anchor": AS_EXT_ANCHOR,
                       "text": "\tif (/(^|[\\\\/])\\.[^.\\\\/]*$/.test(path)) return '';\n"}],
            "test": _as_test("path", "fileExtension",
                             "it('gives no extension to a dotfile', () => {\n"
                             "\tassert.equal(fileExtension('.gitignore'), '');\n"
                             "\tassert.equal(fileExtension('/a/.env'), '');\n"
                             "\tassert.equal(fileExtension('a/.eslintrc.json'), '.json');\n"
                             "});\n"),
        },
        {
            "id": "c3-ext-dir-dot",
            "title": "fileExtension: a dot in a directory name",
            "prompt": (
                "In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `''` when the last dot "
                "is in a directory name, not in the file name (with `/` or `\\` as the separator). Examples: "
                "`fileExtension('/a.b/c')` is `''`, `fileExtension('C:\\\\a.b\\\\c')` is `''`."
            ),
            "edits": [{"op": "insert_after", "file": AS_PATH, "anchor": AS_EXT_ANCHOR,
                       "text": "\tif (path.lastIndexOf('.') < Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\\\'))) return '';\n"}],
            "test": _as_test("path", "fileExtension",
                             "it('ignores a dot in a directory name', () => {\n"
                             "\tassert.equal(fileExtension('/a.b/c'), '');\n"
                             "\tassert.equal(fileExtension('C:\\\\a.b\\\\c'), '');\n"
                             "\tassert.equal(fileExtension('/a.b/c.md'), '.md');\n"
                             "});\n"),
        },
        {
            "id": "c4-ext-trailing-dot",
            "title": "fileExtension: a trailing dot",
            "prompt": (
                "In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `''` for a path that "
                "ends with a dot. Today `fileExtension('file.')` returns `'.'`."
            ),
            "edits": [{"op": "insert_after", "file": AS_PATH, "anchor": AS_EXT_ANCHOR,
                       "text": "\tif (path.endsWith('.')) return '';\n"}],
            "test": _as_test("path", "fileExtension",
                             "it('gives no extension for a trailing dot', () => {\n"
                             "\tassert.equal(fileExtension('file.'), '');\n"
                             "\tassert.equal(fileExtension('/a/b.'), '');\n"
                             "});\n"),
        },
        {
            "id": "c5-ext-dts",
            "title": "fileExtension: .d.ts",
            "prompt": (
                "In `packages/internal-helpers/src/path.ts`, make `fileExtension` return `'.d.ts'` for a "
                "TypeScript declaration file. Example: `fileExtension('/types/env.d.ts')` is `'.d.ts'`; "
                "`fileExtension('/a/b.ts')` stays `'.ts'`."
            ),
            "edits": [{"op": "insert_after", "file": AS_PATH, "anchor": AS_EXT_ANCHOR,
                       "text": "\tif (/\\.d\\.ts$/.test(path)) return '.d.ts';\n"}],
            "test": _as_test("path", "fileExtension",
                             "it('keeps .d.ts whole', () => {\n"
                             "\tassert.equal(fileExtension('/types/env.d.ts'), '.d.ts');\n"
                             "\tassert.equal(fileExtension('/a/b.ts'), '.ts');\n"
                             "});\n"),
        },
    ],
    "disjoint": [
        {
            "id": "d1-package-name-length",
            "title": "validatePackageName: at most 214 characters",
            "prompt": (
                "In `packages/internal-helpers/src/cli.ts`, make `validatePackageName` return false for a name "
                "longer than 214 characters (the npm limit). A name of 214 characters stays valid."
            ),
            "edits": [{"op": "replace", "file": "packages/internal-helpers/src/cli.ts",
                       "old": "\treturn NPM_PACKAGE_NAME_REGEX.test(packageName);\n",
                       "new": "\treturn packageName.length <= 214 && NPM_PACKAGE_NAME_REGEX.test(packageName);\n"}],
            "test": _as_test("cli", "validatePackageName",
                             "it('refuses names longer than 214 characters', () => {\n"
                             "\tassert.equal(validatePackageName('a'.repeat(215)), false);\n"
                             "\tassert.equal(validatePackageName('a'.repeat(214)), true);\n"
                             "\tassert.equal(validatePackageName('react'), true);\n"
                             "});\n"),
        },
        {
            "id": "d2-forwarded-skip-empty",
            "title": "getFirstForwardedValue: skip empty entries",
            "prompt": (
                "In `packages/internal-helpers/src/request.ts`, make `getFirstForwardedValue` skip empty entries "
                "of the header. Example: `getFirstForwardedValue(' , ,1.2.3.4, 5.6.7.8')` is `'1.2.3.4'`. When every entry "
                "is empty, it still returns `''`."
            ),
            "edits": [{"op": "replace", "file": "packages/internal-helpers/src/request.ts",
                       "old": "\treturn multiValueHeader\n\t\t?.toString()\n\t\t?.split(',')\n\t\t.map((e) => e.trim())?.[0];\n",
                       "new": "\tconst values = multiValueHeader\n\t\t?.toString()\n\t\t?.split(',')\n\t\t.map((e) => e.trim());\n\treturn values?.find((e) => e !== '') ?? values?.[0];\n"}],
            "test": _as_test("request", "getFirstForwardedValue",
                             "it('skips empty entries', () => {\n"
                             "\tassert.equal(getFirstForwardedValue(' , ,1.2.3.4, 5.6.7.8'), '1.2.3.4');\n"
                             "\tassert.equal(getFirstForwardedValue(', 10.0.0.1'), '10.0.0.1');\n"
                             "\tassert.equal(getFirstForwardedValue('9.9.9.9'), '9.9.9.9');\n"
                             "});\n"),
        },
        {
            "id": "d3-protocol-colon",
            "title": "matchProtocol: accept a trailing colon",
            "prompt": (
                "In `packages/internal-helpers/src/remote.ts`, make `matchProtocol` accept a protocol written "
                "with its trailing colon. Example: `matchProtocol(new URL('https://a.com'), 'https:')` is true, "
                "the same as with `'https'`."
            ),
            "edits": [{"op": "replace", "file": "packages/internal-helpers/src/remote.ts",
                       "old": "\treturn !protocol || protocol === url.protocol.slice(0, -1);\n",
                       "new": "\treturn !protocol || protocol.replace(/:$/, '') === url.protocol.slice(0, -1);\n"}],
            "test": _as_test("remote", "matchProtocol",
                             "it('accepts a trailing colon', () => {\n"
                             "\tassert.equal(matchProtocol(new URL('https://a.com'), 'https:'), true);\n"
                             "\tassert.equal(matchProtocol(new URL('https://a.com'), 'https'), true);\n"
                             "\tassert.equal(matchProtocol(new URL('https://a.com'), 'http:'), false);\n"
                             "});\n"),
        },
        {
            "id": "d4-remove-hash",
            "title": "removeQueryString: remove the hash too",
            "prompt": (
                "In `packages/internal-helpers/src/path.ts`, make `removeQueryString` also remove a hash. "
                "Examples: `removeQueryString('/a/b.png?w=10#x')` and `removeQueryString('/a/b.png#x')` are `'/a/b.png'`."
            ),
            "edits": [{"op": "replace", "file": "packages/internal-helpers/src/path.ts",
                       "old": "export function removeQueryString(path: string) {\n\tconst index = path.lastIndexOf('?');\n",
                       "new": "export function removeQueryString(path: string) {\n\tconst hash = path.indexOf('#');\n\tif (hash > 0) path = path.substring(0, hash);\n\tconst index = path.lastIndexOf('?');\n"}],
            "test": _as_test("path", "removeQueryString",
                             "it('removes the query and the hash', () => {\n"
                             "\tassert.equal(removeQueryString('/a/b.png?w=10#x'), '/a/b.png');\n"
                             "\tassert.equal(removeQueryString('/a/b.png#x'), '/a/b.png');\n"
                             "\tassert.equal(removeQueryString('/a?b'), '/a');\n"
                             "});\n"),
        },
        {
            "id": "d5-get-by-path",
            "title": "object: getByPath",
            "prompt": (
                "In `packages/internal-helpers/src/object.ts`, add an exported function `getByPath(obj, path)` "
                "that reads a dot-separated property path such as `'a.b.c'`. It returns `undefined` when a step "
                "is missing or when a step is one of `FORBIDDEN_PATH_KEYS`. Example: "
                "`getByPath({ a: { b: { c: 3 } } }, 'a.b.c')` is 3; `getByPath({}, '__proto__.x')` is `undefined`."
            ),
            "edits": [{"op": "insert_after", "file": "packages/internal-helpers/src/object.ts",
                       "anchor": "export const FORBIDDEN_PATH_KEYS = new Set(['__proto__', 'constructor', 'prototype']);\n",
                       "text": (
                           "\n/**\n"
                           " * Reads a dot-separated property path such as `a.b.c`. Returns `undefined` when a step\n"
                           " * is missing or is one of FORBIDDEN_PATH_KEYS.\n"
                           " */\n"
                           "export function getByPath(obj: unknown, path: string): unknown {\n"
                           "\tlet current: any = obj;\n"
                           "\tfor (const key of path.split('.')) {\n"
                           "\t\tif (FORBIDDEN_PATH_KEYS.has(key) || current == null || !Object.hasOwn(Object(current), key)) {\n"
                           "\t\t\treturn undefined;\n"
                           "\t\t}\n"
                           "\t\tcurrent = current[key];\n"
                           "\t}\n"
                           "\treturn current;\n"
                           "}\n")}],
            "test": _as_test("object", "getByPath",
                             "it('reads a dot path and refuses forbidden keys', () => {\n"
                             "\tassert.equal(getByPath({ a: { b: { c: 3 } } }, 'a.b.c'), 3);\n"
                             "\tassert.equal(getByPath({ a: 1 }, 'a.x'), undefined);\n"
                             "\tassert.equal(getByPath({}, '__proto__.polluted'), undefined);\n"
                             "\tassert.equal(getByPath({ a: { constructor: 1 } }, 'a.constructor'), undefined);\n"
                             "});\n"),
        },
    ],
}

TASKS = {"requests": REQUESTS, "ripgrep": RIPGREP, "astro": ASTRO}
SETS = ("disjoint", "colliding")
