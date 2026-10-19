#!/usr/bin/env python3
"""Hidden behaviour checks for the ripgrep task set (tasks_rg.py): one check per task, named as the task,
plus one `<pair>_both` check per pair that uses both changes together (60 checks; names in tasks_rg.BOTH).

    python3 probes_rg.py REPO   -> one JSON object {check: true|false|"error: ..."}

Builds rg from REPO with `cargo build --bin rg` (CARGO_TARGET_DIR = $PROBES_TARGET_DIR if set, else
REPO/target); a failed build makes every check "error: build failed". Each check runs the built binary in a
fresh temp dir with small fixture files, a timeout, an empty HOME and no RIPGREP_CONFIG_PATH. Every check
fails on the base commit 3fce3b5. Checks compare behaviour, not exact wording, unless the task fixes the
format. Optional: PROBES_ONLY=name,name runs only those checks (the others are left out of the output).
"""
import bz2, gzip, json, os, re, shutil, subprocess, sys, tempfile

REPO = os.path.abspath(sys.argv[1])
TARGET = os.environ.get("PROBES_TARGET_DIR") or os.path.join(REPO, "target")
RG = os.path.join(TARGET, "debug", "rg")
TIMEOUT = 20
TMP_ROOT = tempfile.mkdtemp(prefix="probes-rg-")
HOME = os.path.join(TMP_ROOT, "home")
os.makedirs(HOME)


def run(args, cwd, stdin=None, env=None):
    e = {k: v for k, v in os.environ.items()
         if not k.startswith(("RIPGREP_", "GIT_", "XDG_")) and k not in ("NO_COLOR", "TERM")}
    e["HOME"] = HOME
    e.update(env or {})
    # stdin is /dev/null unless given: rg would otherwise search an inherited pipe instead of the cwd
    p = subprocess.run([RG, *args], cwd=cwd, input=stdin, stdin=None if stdin is not None else subprocess.DEVNULL,
                       capture_output=True, timeout=TIMEOUT, env=e)
    return p.returncode, p.stdout, p.stderr


def fixture(files):
    """files: {relpath: str | bytes | None (directory)} -> new temp dir."""
    d = tempfile.mkdtemp(dir=TMP_ROOT)
    for rel, content in files.items():
        path = os.path.join(d, rel)
        if content is None:
            os.makedirs(path, exist_ok=True)
            continue
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(content.encode() if isinstance(content, str) else content)
    return d


def script(d, name, body):
    path = os.path.join(d, name)
    with open(path, "w") as f:
        f.write("#!/bin/sh\n" + body)
    os.chmod(path, 0o755)
    return "./" + name


def lines(b):
    return b.decode("utf-8", "replace").splitlines()


def out_lines(args, cwd, **kw):
    code, out, _ = run(args, cwd, **kw)
    return code, lines(out)


def listed(args, cwd):
    _, ls = out_lines(["--files", *args], cwd)
    return set(ls)


# ---------------------------------------------------------------- pair: --sort size / extension
def sort_size():
    d = fixture({"a.txt": "x" * 29 + "\n", "b.txt": "x" * 9 + "\n", "c.txt": "x" * 19 + "\n"})
    return (out_lines(["--files", "--sort", "size"], d)[1] == ["b.txt", "c.txt", "a.txt"]
            and out_lines(["-l", "x", "--sortr", "size"], d)[1] == ["a.txt", "c.txt", "b.txt"])


def sort_extension():
    d = fixture({n: "x\n" for n in ["z.md", "a.rs", "b.txt", "Makefile", "y.rs"]})
    asc = out_lines(["--files", "--sort", "extension"], d)[1]
    desc = out_lines(["-l", "x", "--sortr", "extension"], d)[1]
    return (asc == ["Makefile", "z.md", "a.rs", "y.rs", "b.txt"]
            and len(desc) == 5 and desc[0] == "b.txt" and set(desc[1:3]) == {"a.rs", "y.rs"}
            and desc[3:] == ["z.md", "Makefile"])


def sort_both():
    d = fixture({"a.rs": "x" * 29 + "\n", "b.md": "x" * 9 + "\n", "c.txt": "x" * 19 + "\n"})
    return (out_lines(["--files", "--sort", "size"], d)[1] == ["b.md", "c.txt", "a.rs"]
            and out_lines(["-l", "x", "--sortr", "extension"], d)[1] == ["c.txt", "a.rs", "b.md"]
            and out_lines(["--files", "--sortr", "size"], d)[1] == ["a.rs", "c.txt", "b.md"])


# ---------------------------------------------------------------- pair: --color aliases
def colored(args, d):
    code, out, _ = run(args, d)
    return code == 0 and b"foo" in out and b"\x1b[" in out


def plain(args, d):
    code, out, _ = run(args, d)
    return code == 0 and b"foo" in out and b"\x1b" not in out


def color_always_aliases():
    d = fixture({"a.txt": "foo\n"})
    return colored(["--color", "yes", "foo", "a.txt"], d) and colored(["--color=force", "foo", "a.txt"], d)


def color_never_aliases():
    d = fixture({"a.txt": "foo\n"})
    return plain(["-p", "--color", "no", "foo", "a.txt"], d) and plain(["-p", "--color=none", "foo", "a.txt"], d)


def color_both():
    d = fixture({"a.txt": "foo\n"})
    return (colored(["--color", "no", "--color", "yes", "foo", "a.txt"], d)
            and plain(["--color", "force", "--color", "none", "foo", "a.txt"], d))


# ---------------------------------------------------------------- pair: --colors styles
def styled(styles, d):
    args = ["--color", "always", "--colors", "match:none"]
    for s in styles:
        args += ["--colors", "match:style:" + s]
    code, out, _ = run(args + ["foo", "a.txt"], d)
    return out if code == 0 and b"foo" in out else None


def colors_dimmed():
    d = fixture({"a.txt": "foo\n"})
    on, off = styled(["dimmed"], d), styled(["dimmed", "nodimmed"], d)
    return on is not None and b"\x1b[2m" in on and off is not None and b"\x1b[2m" not in off


def colors_strikethrough():
    d = fixture({"a.txt": "foo\n"})
    on, off = styled(["strikethrough"], d), styled(["strikethrough", "nostrikethrough"], d)
    return on is not None and b"\x1b[9m" in on and off is not None and b"\x1b[9m" not in off


def colors_both():
    d = fixture({"a.txt": "foo\n"})
    both = styled(["dimmed", "strikethrough"], d)
    half = styled(["dimmed", "strikethrough", "nodimmed"], d)
    return (both is not None and b"\x1b[2m" in both and b"\x1b[9m" in both
            and half is not None and b"\x1b[2m" not in half and b"\x1b[9m" in half)


# ---------------------------------------------------------------- pair: size suffixes
def size_dir():
    return fixture({"small.txt": "foo\n" + "a" * 100 + "\n", "big.txt": "foo\n" + "b" * 3000 + "\n"})


def size_files(arg, d):
    code, ls = out_lines(["-l", "--max-filesize", arg, "foo"], d)
    return code, set(ls)


def size_lowercase():
    d = size_dir()
    return (size_files("2k", d) == (0, {"small.txt"}) and size_files("1m", d) == (0, {"small.txt", "big.txt"})
            and size_files("1g", d) == (0, {"small.txt", "big.txt"})
            and run(["--dfa-size-limit", "10m", "foo", "small.txt"], d)[0] == 0)


def size_terabytes():
    d = size_dir()
    return (size_files("1T", d) == (0, {"small.txt", "big.txt"})
            and run(["--max-filesize", "16777216T", "foo"], d)[0] == 2
            and size_files("16777215T", d) == (0, {"small.txt", "big.txt"})
            and run(["--regex-size-limit", "1T", "foo", "small.txt"], d)[0] == 0)


def size_both():
    d = size_dir()
    return (size_files("1t", d) == (0, {"small.txt", "big.txt"})
            and run(["--max-filesize", "16777216t", "foo"], d)[0] == 2
            and size_files("2k", d) == (0, {"small.txt"}))


# ---------------------------------------------------------------- pair: --type-add parsing
def web_dir():
    return fixture({n: "foo\n" for n in ["x.html", "y.css", "z.js", "a.rs", "b.c", "w.htm"]})


def type_add_spaces():
    d = web_dir()
    return (listed(["--type-add", "web : *.html", "-t", "web"], d) == {"x.html"}
            and listed(["--type-add", "mine: include: rust , c", "-t", "mine"], d) == {"a.rs", "b.c"}
            and listed(["--type-add", "web:*.html", "-t", "web"], d) == {"x.html"})


def type_add_glob_list():
    d = web_dir()
    return (listed(["--type-add", "web:*.html,*.css", "-t", "web"], d) == {"x.html", "y.css"}
            and listed(["--type-add", "web:*.{html,js},*.css", "-t", "web"], d) == {"x.html", "z.js", "y.css"}
            and listed(["--type-add", "web:*.{html,htm}", "-t", "web"], d) == {"x.html", "w.htm"})


def type_add_both():
    d = web_dir()
    return (listed(["--type-add", "web: *.html , *.css", "-t", "web"], d) == {"x.html", "y.css"}
            and listed(["--type-add", "mine : include : rust,c", "-t", "mine"], d) == {"a.rs", "b.c"})


# ---------------------------------------------------------------- pair: glob bracket expressions
def glob_posix_class():
    d = fixture({n: "" for n in ["1.log", "a.log", "_.log", "B.log"]})
    return (listed(["-g", "[[:digit:]].log"], d) == {"1.log"}
            and listed(["-g", "[![:digit:]].log"], d) == {"a.log", "_.log", "B.log"}
            and listed(["-g", "[[:upper:]].log"], d) == {"B.log"}
            and listed(["-g", "[[:alpha:]_].log"], d) == {"a.log", "_.log", "B.log"})


def glob_class_escape():
    d = fixture({n: "" for n in ["]x.txt", "-x.txt", "ax.txt", "bx.txt", "cx.txt"]})
    return (listed(["-g", "[\\]]x.txt"], d) == {"]x.txt"}
            and listed(["-g", "[a\\-c]x.txt"], d) == {"ax.txt", "-x.txt", "cx.txt"}
            and listed(["-g", "[a-c]x.txt"], d) == {"ax.txt", "bx.txt", "cx.txt"})


def glob_class_both():
    d = fixture({n: "" for n in ["1x.txt", "]x.txt", "ax.txt"]})
    return listed(["-g", "[[:digit:]\\]]x.txt"], d) == {"1x.txt", "]x.txt"}


# ---------------------------------------------------------------- pair: --stats text
def stats_lines(args, d):
    code, out, _ = run(["--stats", *args], d)
    return [l.strip() for l in lines(out)] if code == 0 else []


def stats_singular():
    d = fixture({"one.txt": "foo bar\n", "two.txt": "foo\nfoo\n"})
    one = stats_lines(["foo", "one.txt"], d)
    two = stats_lines(["foo", "one.txt", "two.txt"], d)
    return (all(l in one for l in ["1 match", "1 matched line", "1 file contained matches", "1 file searched"])
            and all(l in two for l in ["3 matches", "3 matched lines", "2 files contained matches",
                                       "2 files searched"]))


def stats_without_match():
    d = fixture({"m.txt": "foo\n", "n1.txt": "bar\n", "n2.txt": "baz\n"})
    ls = stats_lines(["foo", "m.txt", "n1.txt", "n2.txt"], d)
    idx = [i for i, l in enumerate(ls) if re.fullmatch(r"1 files? contained matches", l)]
    return len(idx) == 1 and idx[0] + 1 < len(ls) and ls[idx[0] + 1] == "2 files contained no matches"


def stats_both():
    d = fixture({"m.txt": "foo\n", "n1.txt": "bar\n"})
    ls = stats_lines(["foo", "m.txt", "n1.txt"], d)
    return ("1 file contained matches" in ls and "1 match" in ls
            and ls[ls.index("1 file contained matches") + 1] == "1 file contained no matches")


# ---------------------------------------------------------------- pair: --type-list filters
def type_names(args):
    d = fixture({})
    code, ls = out_lines(["--type-list", *args], d)
    return code, [l.split(":")[0] for l in ls]


def type_list_select():
    code, names = type_names(["-t", "rust", "-t", "c"])
    full = type_names([])[1]
    return code == 0 and names == ["c", "rust"] and len(full) > 100


def type_list_negate():
    code, names = type_names(["-T", "rust"])
    full = type_names([])[1]
    return code == 0 and "rust" not in names and "c" in names and len(names) == len(full) - 1 > 100


def type_list_both():
    code, names = type_names(["-t", "rust", "-t", "c", "-T", "c"])
    return code == 0 and names == ["rust"]


# ---------------------------------------------------------------- pair: --replace references
def replaced(pattern, repl, text, extra=()):
    d = fixture({})
    code, out, _ = run([*extra, pattern, "-r", repl], d, stdin=text.encode())
    return lines(out) if code == 0 else None


def replace_digit_ref():
    return (replaced("(f)(o)o", "$1x$2y", "foo\n") == ["fxoy"]
            and replaced("(f)(o)o", "${1}x", "foo\n") == ["fx"]
            and replaced("(?P<w>f)oo", "$w!", "foo\n") == ["f!"])


def replace_ampersand():
    return (replaced("fo+", "[$&]", "foo bar\n", ["-o"]) == ["[foo]"]
            and replaced("fo+", "$$&", "foo\n") == ["$&"]
            and replaced("fo+", "<$0>", "foo\n") == ["<foo>"])


def replace_both():
    return replaced("(f)oo", "$1x$&", "foo\n") == ["fxfoo"] and replaced("(f)(o)o", "$&$2z", "foo\n") == ["foooz"]


# ---------------------------------------------------------------- pair: RIPGREP_CONFIG_PATH
def config_dir():
    return fixture({"f.txt": "foo\nfoo\nfoo\n"})


def with_home_files(files):
    for name, content in files.items():
        with open(os.path.join(HOME, name), "w") as f:
            f.write(content)


def config_tilde():
    d = config_dir()
    with_home_files({"rgrc": "--max-count=1\n"})
    code, ls = out_lines(["foo", "f.txt"], d, env={"RIPGREP_CONFIG_PATH": "~/rgrc"})
    return code == 0 and ls == ["foo"]


def config_multiple():
    d = config_dir()
    for name, content in {"a.rc": "--max-count=1\n", "b.rc": "--line-number\n"}.items():
        with open(os.path.join(d, name), "w") as f:
            f.write(content)
    a, b = os.path.join(d, "a.rc"), os.path.join(d, "b.rc")
    r1 = out_lines(["foo", "f.txt"], d, env={"RIPGREP_CONFIG_PATH": a + os.pathsep + b})
    r2 = out_lines(["foo", "f.txt"], d, env={"RIPGREP_CONFIG_PATH": a + os.pathsep + os.pathsep + b})
    return r1 == (0, ["1:foo"]) and r2 == (0, ["1:foo"])


def config_both():
    d = config_dir()
    with_home_files({"c1.rc": "--max-count=2\n", "c2.rc": "--line-number\n"})
    code, ls = out_lines(["foo", "f.txt"], d, env={"RIPGREP_CONFIG_PATH": "~/c1.rc" + os.pathsep + "~/c2.rc"})
    return code == 0 and ls == ["1:foo", "2:foo"]


# ---------------------------------------------------------------- singles: core
def context_num_shorthand():
    d = fixture({"c.txt": "1\n2\nfoo\n4\n5\n6\n7\n8\nfoo\n10\n11\n"})
    ok = True
    for n in ["1", "2", "12"]:
        a, b = run(["-" + n, "foo", "c.txt"], d), run(["-C" + n, "foo", "c.txt"], d)
        ok = ok and a[0] == 0 and a[:2] == b[:2]
    _, null_out, _ = run(["-0", "foo", "c.txt", "c.txt"], d)
    return ok and b"c.txt\x00foo" in null_out


def type_comma_list():
    d = fixture({"a.rs": "foo\n", "b.c": "foo\n", "c.py": "foo\n"})
    return listed(["-t", "rust,c"], d) == {"a.rs", "b.c"} and listed(["-T", "rust,c"], d) == {"c.py"}


def json_msgs(args, d):
    code, out, _ = run(["--json", *args], d)
    msgs = [json.loads(l) for l in lines(out) if l.strip()]
    return code, [(m["type"], m.get("data", {}).get("path", {}).get("text")) for m in msgs]


def json_include_zero():
    d = fixture({"a.txt": "foo\n", "b.txt": "bar\n"})
    code, msgs = json_msgs(["--include-zero", "foo", "a.txt", "b.txt"], d)
    _, plain_msgs = json_msgs(["foo", "a.txt", "b.txt"], d)
    return (code == 0 and ("begin", "b.txt") in msgs and ("end", "b.txt") in msgs and ("match", "a.txt") in msgs
            and ("end", "a.txt") in msgs and not any(p == "b.txt" for _, p in plain_msgs))


def count_field_separator():
    d = fixture({"a.txt": "foo\n", "c.txt": "foo\nfoo foo\n"})
    c = out_lines(["-c", "--field-match-separator", "=", "foo", "a.txt", "c.txt"], d)
    m = out_lines(["--count-matches", "--field-match-separator", "=", "foo", "a.txt", "c.txt"], d)
    plain_c = out_lines(["-c", "foo", "a.txt", "c.txt"], d)
    return (c[0] == 0 and set(c[1]) == {"a.txt=1", "c.txt=2"} and set(m[1]) == {"a.txt=1", "c.txt=3"}
            and set(plain_c[1]) == {"a.txt:1", "c.txt:2"})


def ignore_file_missing_error():
    d = fixture({"a.txt": "foo\n"})
    code, out, err = run(["--ignore-file", "missing.ignore", "foo", "a.txt"], d)
    return code == 2 and b"foo" in out and b"missing.ignore" in err


def engine_pcre_alias():
    d = fixture({"a.txt": "foo\n"})
    a, b = run(["--engine", "pcre", "foo", "a.txt"], d), run(["--engine", "pcre2", "foo", "a.txt"], d)
    return a[0] == b[0] and a[1] == b[1] and b"unrecognized" not in a[2]


def path_double_slash():
    d = fixture({"sub/s.txt": "foo\n"})
    return (out_lines(["--files", "sub//"], d)[1] == ["sub/s.txt"]
            and out_lines(["foo", "sub///"], d)[1] == ["sub/s.txt:foo"]
            and out_lines(["--files", "sub/"], d)[1] == ["sub/s.txt"]
            and out_lines(["--files", "sub"], d)[1] == ["sub/s.txt"])


def stdin_label():
    d = fixture({"a.txt": "foo\n"})
    return (out_lines(["-H", "--label", "in.txt", "foo"], d, stdin=b"foo\n") == (0, ["in.txt:foo"])
            and out_lines(["-H", "--label=in.txt", "foo", "-"], d, stdin=b"foo\n") == (0, ["in.txt:foo"])
            and out_lines(["-H", "--label", "X", "foo", "a.txt"], d) == (0, ["a.txt:foo"]))


def max_columns_preview_default():
    d = fixture({"f.txt": "foo " + "x" * 300 + "\n"})
    code, ls = out_lines(["--max-columns-preview", "foo", "f.txt"], d)
    code10, ls10 = out_lines(["-M", "10", "--max-columns-preview", "foo", "f.txt"], d)
    _, full = out_lines(["foo", "f.txt"], d)
    return (code == 0 and len(ls) == 1 and 150 <= len(ls[0]) < 200 and "[..." in ls[0]
            and code10 == 0 and len(ls10) == 1 and len(ls10[0]) < 60 and "[..." in ls10[0]
            and full == ["foo " + "x" * 300])


def pre_glob_case_insensitive():
    d = fixture({"a.txt": "x\n"})
    pre = script(d, "p.sh", "echo PREPROCESSED\n")
    on = out_lines(["--pre", pre, "--pre-glob", "*.TXT", "--glob-case-insensitive", "PREPROCESSED", "a.txt"], d)
    off = out_lines(["--pre", pre, "--pre-glob", "*.TXT", "PREPROCESSED", "a.txt"], d)
    return on == (0, ["PREPROCESSED"]) and off[0] == 1


# ---------------------------------------------------------------- singles: printer
def omitted_line_singular():
    d = fixture({"f.txt": "foo " + "x" * 50 + "\n" + "foo foo " + "y" * 50 + "\n"})
    code, ls = out_lines(["-M", "20", "-r", "X", "foo", "f.txt"], d)
    _, out, _ = run(["-M", "20", "--color", "always", "foo", "f.txt"], d)
    return (code == 0 and ls == ["[Omitted long line with 1 match]", "[Omitted long line with 2 matches]"]
            and b"[Omitted long line with 1 match]" in out)


def max_columns_chars():
    d = fixture({"f.txt": "ééééé foo\n", "g.txt": "abcdefghij foo\n"})
    return (out_lines(["-M", "10", "foo", "f.txt"], d) == (0, ["ééééé foo"])
            and out_lines(["-M", "8", "foo", "f.txt"], d)[1] == ["[Omitted long matching line]"]
            and out_lines(["-M", "10", "foo", "g.txt"], d)[1] == ["[Omitted long matching line]"])


def binary_message_null():
    d = fixture({"bin.dat": b"x\x00foo\n", "a.txt": "foo\n"})
    _, out0, _ = run(["-0", "foo", "bin.dat", "a.txt"], d)
    _, out, _ = run(["foo", "bin.dat", "a.txt"], d)
    return (b"bin.dat\x00binary file matches" in out0 and b"bin.dat: binary file matches" not in out0
            and b"a.txt\x00foo" in out0 and b"bin.dat: binary file matches" in out)


def hyperlink_zed():
    d = fixture({"a.txt": "foo\n"})
    code, out, _ = run(["-H", "--hyperlink-format", "zed", "--color", "always", "foo", "a.txt"], d)
    return code == 0 and re.search(rb"\x1b\]8;;zed://file/[^\x1b]*a\.txt:1:1", out) is not None


def hyperlink_alias_case():
    d = fixture({"a.txt": "foo\n"})
    code, out, _ = run(["-H", "--hyperlink-format", "VSCode", "--color", "always", "foo", "a.txt"], d)
    code2, out2, _ = run(["-H", "--hyperlink-format", "vscode", "--color", "always", "foo", "a.txt"], d)
    return code == 0 and b"vscode://file/" in out and out == out2


def trim_unicode():
    d = fixture({"f.txt": " 　 foo\n\t  foo2\n"})
    return out_lines(["--trim", "foo", "f.txt"], d) == (0, ["foo", "foo2"])


def bytes_printed(args, d):
    code, out, _ = run(["--stats", *args], d)
    text = out.decode()
    m = re.search(r"^(\d+) bytes? printed$", text, re.M)
    body = text.split("\n\n")[0] + "\n"
    return code == 0 and m is not None and int(m.group(1)) == len(body.encode())


def summary_bytes_printed():
    d = fixture({"a.txt": "foo\n", "c.txt": "foo\nfoo\n", "n.txt": "bar\n"})
    return (bytes_printed(["-l", "foo", "a.txt", "c.txt"], d) and bytes_printed(["-c", "foo", "a.txt", "c.txt"], d)
            and bytes_printed(["--files-without-match", "foo", "a.txt", "n.txt"], d))


# ---------------------------------------------------------------- singles: cli crate
def decompress_svgz():
    d = fixture({"x.svgz": gzip.compress(b"<svg>foo</svg>\n")})
    return out_lines(["-z", "foo", "x.svgz"], d) == (0, ["<svg>foo</svg>"])


def decompress_uppercase():
    d = fixture({"up.GZ": gzip.compress(b"foo upper\n"), "y.Bz2": bz2.compress(b"foo bz\n")})
    return (out_lines(["-z", "foo", "up.GZ"], d) == (0, ["foo upper"])
            and out_lines(["-z", "foo", "y.Bz2"], d) == (0, ["foo bz"]))


def pre_exit_status():
    d = fixture({"a.txt": "foo\n"})
    p = script(d, "p.sh", "exit 3\n")
    q = script(d, "q.sh", "echo boom >&2\nexit 4\n")
    c1, _, e1 = run(["--pre", p, "foo", "a.txt"], d)
    c2, _, e2 = run(["--pre", q, "foo", "a.txt"], d)
    pat = r"(exit|status|code)\D{0,20}\b%d\b"
    return (c1 == 2 and re.search(pat % 3, e1.decode(), re.I) is not None
            and c2 == 2 and b"boom" in e2 and re.search(pat % 4, e2.decode(), re.I) is not None)


def pattern_file_bom():
    d = fixture({"pats": b"\xef\xbb\xbffoo\nbar\n", "a.txt": "foo\nbaz\n"})
    return out_lines(["-f", "pats", "a.txt"], d) == (0, ["foo"])


def unescape_esc():
    d = fixture({"a.txt": "foo\nbar\n"})
    _, o1, _ = run(["-n", "--field-match-separator", "\\e", "foo", "a.txt"], d)
    _, o2, _ = run(["-n", "-A1", "--field-context-separator", "\\e", "foo", "a.txt"], d)
    _, o3, _ = run(["--field-match-separator", "\\t", "-n", "foo", "a.txt"], d)
    return o1 == b"1\x1bfoo\n" and b"2\x1bbar\n" in o2 and o3 == b"1\tfoo\n"


# ---------------------------------------------------------------- singles: searcher, regex, globset
def encoding_utf16_alias():
    d = fixture({"le.txt": "foo\n".encode("utf-16-le"), "be.txt": "foo\n".encode("utf-16-be")})
    return (out_lines(["-E", "utf16le", "foo", "le.txt"], d) == (0, ["foo"])
            and out_lines(["-E", "utf16be", "foo", "be.txt"], d) == (0, ["foo"])
            and out_lines(["-E", "utf16", "foo", "le.txt"], d) == (0, ["foo"]))


def smart_case_per_pattern():
    d = fixture({"f.txt": "foo\nFoo\nBAR\nbar\nfOO\n"})
    multi = out_lines(["-S", "-e", "Foo", "-e", "bar", "f.txt"], d)
    return (multi[0] == 0 and set(multi[1]) == {"Foo", "BAR", "bar"}
            and set(out_lines(["-S", "Foo", "f.txt"], d)[1]) == {"Foo"}
            and set(out_lines(["-S", "foo", "f.txt"], d)[1]) == {"foo", "Foo", "fOO"})


def glob_unicode_any():
    d = fixture({"éa.txt": "", "ba.txt": "", "ca.md": ""})
    return (listed(["-g", "?a.txt"], d) == {"éa.txt", "ba.txt"} and listed(["-g", "[é]a.txt"], d) == {"éa.txt"}
            and listed(["-g", "[!b]a.txt"], d) == {"éa.txt"})


# ---------------------------------------------------------------- singles: ignore crate
def type_odin():
    d = fixture({"m.odin": "", "m.go": ""})
    _, tl = out_lines(["--type-list"], d)
    return "odin: *.odin" in tl and listed(["-t", "odin"], d) == {"m.odin"}


def gitignore_escaped_space():
    d = fixture({"foo ": "", "foo": "", "bar": "", ".ignore": "foo\\  \n"})
    return listed([], d) == {"foo", "bar"}


def override_dot_slash():
    d = fixture({"sub/s.txt": "", "t.txt": "", "sub/u.md": ""})
    return (listed(["-g", "./sub/*.txt"], d) == {"sub/s.txt"} and listed(["-g", "!./sub/*"], d) == {"t.txt"}
            and listed(["-g", "sub/*.txt"], d) == {"sub/s.txt"})


def max_filesize_explicit():
    d = fixture({"big.txt": "foo\n" + "b" * 3000 + "\n", "small.txt": "foo\n"})
    r1 = out_lines(["--max-filesize", "1K", "-l", "foo", "big.txt", "small.txt"], d)
    r2 = out_lines(["-j1", "--max-filesize", "1K", "-l", "foo", "big.txt", "small.txt"], d)
    r3 = run(["--max-filesize", "1K", "foo", "big.txt"], d)
    return r1 == (0, ["small.txt"]) and r2 == (0, ["small.txt"]) and r3[0] == 1 and r3[1] == b""


def sapling_repo_gitignore():
    d = fixture({".sl": None, ".gitignore": "ign.txt\n", "ign.txt": "", "keep.txt": ""})
    return listed([], d) == {"keep.txt"} and listed(["--no-ignore-vcs"], d) == {"ign.txt", "keep.txt"}


CHECKS = {f.__name__: f for f in [
    sort_size, sort_extension, sort_both,
    color_always_aliases, color_never_aliases, color_both,
    colors_dimmed, colors_strikethrough, colors_both,
    size_lowercase, size_terabytes, size_both,
    type_add_spaces, type_add_glob_list, type_add_both,
    glob_posix_class, glob_class_escape, glob_class_both,
    stats_singular, stats_without_match, stats_both,
    type_list_select, type_list_negate, type_list_both,
    replace_digit_ref, replace_ampersand, replace_both,
    config_tilde, config_multiple, config_both,
    context_num_shorthand, type_comma_list, json_include_zero, count_field_separator, ignore_file_missing_error,
    engine_pcre_alias, path_double_slash, stdin_label, max_columns_preview_default, pre_glob_case_insensitive,
    omitted_line_singular, max_columns_chars, binary_message_null, hyperlink_zed, hyperlink_alias_case,
    trim_unicode, summary_bytes_printed,
    decompress_svgz, decompress_uppercase, pre_exit_status, pattern_file_bom, unescape_esc,
    encoding_utf16_alias, smart_case_per_pattern, glob_unicode_any,
    type_odin, gitignore_escaped_space, override_dot_slash, max_filesize_explicit, sapling_repo_gitignore,
]}
assert len(CHECKS) == 60


def main():
    only = [n for n in os.environ.get("PROBES_ONLY", "").split(",") if n]
    names = [n for n in CHECKS if not only or n in only]
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET)
    try:
        b = subprocess.run(["cargo", "build", "--bin", "rg"], cwd=REPO, env=env, capture_output=True, timeout=1800)
        built = b.returncode == 0 and os.path.exists(RG)
    except Exception:  # noqa: BLE001 - a missing cargo or a timeout is a failed build
        built = False
    out = {}
    for name in names:
        if not built:
            out[name] = "error: build failed"
            continue
        try:
            out[name] = bool(CHECKS[name]())
        except Exception as e:  # a crash or a timeout counts as a failed check
            out[name] = f"error: {type(e).__name__}: {e}"[:200]
    shutil.rmtree(TMP_ROOT, ignore_errors=True)
    print(json.dumps(out))


if __name__ == "__main__":
    main()
