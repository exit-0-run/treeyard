"""Task set for the live coding-agent demo on ripgrep (Rust), base commit 3fce3b5 (ignore-0.4.33, 2026-08-04).

TASKS = [(name, title, description), ...], 50 entries, each observable through the `rg` command line:
- 10 pairs (20 tasks): two independent changes to the SAME flag or function, so the two agents' edits
  collide (--sort values, --color values, --colors styles, size suffixes, --type-add parsing, glob
  bracket expressions, --stats text, --type-list filtering, --replace references, RIPGREP_CONFIG_PATH);
- 30 single tasks, each in its own place (core flags/parser/arguments, printer, searcher, globset,
  ignore, cli, regex).
Only one task (stdin_label) adds a new flag, so the flag list in crates/core/flags/defs.rs is edited once.
Hidden checks: probes_rg.py (one per task plus `<pair>_both` per pair, see BOTH). Reference:
rg_ref.patch (all 50 tasks applied together).
"""

PAIRS = [
    ("sort_size", "sort_extension"),
    ("color_always_aliases", "color_never_aliases"),
    ("colors_dimmed", "colors_strikethrough"),
    ("size_lowercase", "size_terabytes"),
    ("type_add_spaces", "type_add_glob_list"),
    ("glob_posix_class", "glob_class_escape"),
    ("stats_singular", "stats_without_match"),
    ("type_list_select", "type_list_negate"),
    ("replace_digit_ref", "replace_ampersand"),
    ("config_tilde", "config_multiple"),
]

# name of the hidden check that uses both changes of a pair together
BOTH = {
    ("sort_size", "sort_extension"): "sort_both",
    ("color_always_aliases", "color_never_aliases"): "color_both",
    ("colors_dimmed", "colors_strikethrough"): "colors_both",
    ("size_lowercase", "size_terabytes"): "size_both",
    ("type_add_spaces", "type_add_glob_list"): "type_add_both",
    ("glob_posix_class", "glob_class_escape"): "glob_class_both",
    ("stats_singular", "stats_without_match"): "stats_both",
    ("type_list_select", "type_list_negate"): "type_list_both",
    ("replace_digit_ref", "replace_ampersand"): "replace_both",
    ("config_tilde", "config_multiple"): "config_both",
}

TASKS = [
    # ------------------------------------------------------------------ pairs (same flag / function)
    ("sort_size", "--sort/--sortr: add the value size",
     "Add a new value `size` to --sort and --sortr: sort the searched or listed files by their size in "
     "bytes, smallest first with --sort size and largest first with --sortr size (files of equal size may "
     "come in any order). Example: in a directory holding a.txt (30 bytes), b.txt (10 bytes) and c.txt "
     "(20 bytes), `rg --files --sort size` prints b.txt, c.txt, a.txt (one per line) and "
     "`rg -l x --sortr size` prints a.txt, c.txt, b.txt. The other --sort values keep working as today."),
    ("sort_extension", "--sort/--sortr: add the value extension",
     "Add a new value `extension` to --sort and --sortr: sort the searched or listed files by file "
     "extension (the text after the last '.' of the file name; a file name without a dot has an empty "
     "extension and sorts first), and by path among files with the same extension. --sortr extension "
     "gives the exact reverse order. Example: with the files z.md, a.rs, b.txt, Makefile and y.rs, "
     "`rg --files --sort extension` prints Makefile, z.md, a.rs, y.rs, b.txt and "
     "`rg --files --sortr extension` prints b.txt, y.rs, a.rs, z.md, Makefile. The other --sort values keep "
     "working as today."),

    ("color_always_aliases", "--color: accept yes and force",
     "Make --color accept `yes` and `force` as synonyms of `always`, as GNU grep does. "
     "`rg --color yes foo a.txt` and `rg --color force foo a.txt` print colored output (ANSI escapes, for "
     "example \\x1b[1m\\x1b[31mfoo\\x1b[0m) even when stdout is not a terminal, exactly like "
     "`--color always`. Today both values are rejected with an error."),
    ("color_never_aliases", "--color: accept no and none",
     "Make --color accept `no` and `none` as synonyms of `never`, as GNU grep does. "
     "`rg -p --color no foo a.txt` (and the same with `none`) prints the matches without any ANSI escape "
     "codes, exactly like `rg -p --color never foo a.txt`. Today both values are rejected with an error."),

    ("colors_dimmed", "--colors: add the style dimmed",
     "Let --colors accept the style attributes `dimmed` and `nodimmed` (like bold/nobold): "
     "`rg --color always --colors 'match:none' --colors 'match:style:dimmed' foo a.txt` prints the match "
     "wrapped in the faint/dim SGR code, i.e. the output contains \\x1b[2m before foo. A later "
     "`match:style:nodimmed` turns it off again. Today the style is rejected as an unrecognized style "
     "attribute."),
    ("colors_strikethrough", "--colors: add the style strikethrough",
     "Let --colors accept the style attributes `strikethrough` and `nostrikethrough` (like bold/nobold): "
     "`rg --color always --colors 'match:none' --colors 'match:style:strikethrough' foo a.txt` prints the "
     "match with the crossed-out SGR code, i.e. the output contains \\x1b[9m before foo. A later "
     "`match:style:nostrikethrough` turns it off again. Today the style is rejected as an unrecognized "
     "style attribute."),

    ("size_lowercase", "size flags: accept lowercase suffixes",
     "The size arguments of --max-filesize, --dfa-size-limit and --regex-size-limit accept the suffixes "
     "K, M and G only in upper case. Accept k, m and g too, with the same meaning (powers of 1024): "
     "`rg -l --max-filesize 2k foo` lists a 100-byte matching file but skips a 3000-byte one, and "
     "`--max-filesize 1m` keeps both. Today a lowercase suffix is an error."),
    ("size_terabytes", "size flags: add the T suffix",
     "Add the suffix T (tebibytes, 2^40 bytes) to the size arguments of --max-filesize, --dfa-size-limit "
     "and --regex-size-limit, next to K, M and G. `rg -l --max-filesize 1T foo` searches every file; a value "
     "too large for 64 bits is an error as for the other suffixes: `--max-filesize 16777216T` fails with a "
     "'size too big' error (exit code 2) while `--max-filesize 16777215T` works."),

    ("type_add_spaces", "--type-add: tolerate spaces",
     "Make --type-add ignore whitespace around the parts of a definition: around the type name, around the "
     "`include` keyword and around each glob or included type name. `rg --files --type-add 'web : *.html' "
     "-t web` lists x.html exactly like `--type-add 'web:*.html'`, and `--type-add 'mine: include: rust , c' "
     "-t mine` selects the same files as `--type-add 'mine:include:rust,c'`. Today these definitions fail "
     "with an 'invalid definition' error or define a type named 'web '."),
    ("type_add_glob_list", "--type-add: several globs in one definition",
     "Let a plain --type-add definition list several globs separated by commas: "
     "`rg --files --type-add 'web:*.html,*.css' -t web` lists x.html and y.css (but not z.js). A comma inside "
     "braces still belongs to the glob, so `--type-add 'web:*.{html,js},*.css'` selects x.html, z.js and "
     "y.css. Today the whole text after the colon is taken as one glob."),

    ("glob_posix_class", "globs: POSIX character classes",
     "Support POSIX character classes inside bracket expressions of globs (-g/--glob, --iglob, ignore files, "
     "--type-add), with their ASCII meaning: [:alpha:], [:digit:], [:alnum:], [:upper:], [:lower:], "
     "[:space:], [:xdigit:], [:punct:]. They can be mixed with other members and negated. With the files "
     "1.log, a.log, _.log and B.log: `rg --files -g '[[:digit:]].log'` prints 1.log, "
     "`-g '[![:digit:]].log'` prints the other three, `-g '[[:upper:]].log'` prints B.log and "
     "`-g '[[:alpha:]_].log'` prints a.log, _.log and B.log. Today '[:' is read as two literal class members, "
     "so these globs select the wrong files."),
    ("glob_class_escape", "globs: backslash escapes inside bracket expressions",
     "When backslash escapes are enabled for a glob (they are for -g/--glob on Unix), a backslash inside a "
     "bracket expression must escape the next character, as it does outside brackets and as git does. With "
     "the files ]x.txt, -x.txt, ax.txt, bx.txt and cx.txt: `rg --files -g '[\\]]x.txt'` prints only ]x.txt, "
     "and `rg --files -g '[a\\-c]x.txt'` prints ax.txt, -x.txt and cx.txt (a, a literal '-', c; not the "
     "range a-c). Today the backslash is taken as a literal class member."),

    ("stats_singular", "--stats: singular nouns for a count of 1",
     "The --stats summary prints '1 files contained matches', '1 matches' and so on. Use the singular noun "
     "when a count is exactly 1: '1 match', '1 matched line', '1 file contained matches', "
     "'1 file searched', '1 byte printed', '1 byte searched'. Other counts keep the plural ('2 matches', "
     "'0 files contained matches'). Example: `rg --stats foo one.txt` with one.txt holding the single line "
     "'foo bar' prints the lines '1 match', '1 matched line', '1 file contained matches' and "
     "'1 file searched'. The JSON summary is unchanged."),
    ("stats_without_match", "--stats: report files without matches",
     "Add a line to the plain-text --stats summary, directly after the 'N files contained matches' line: "
     "'M files contained no matches', where M is the number of files searched minus the number of files "
     "with a match. Example: `rg --stats foo m.txt n1.txt n2.txt`, where only m.txt contains foo, prints "
     "'1 files contained matches' followed by the new line '2 files contained no matches'. The JSON summary "
     "is unchanged."),

    ("type_list_select", "--type-list: honour -t",
     "When --type-list is combined with -t/--type, print only the selected types (in the usual sorted "
     "order) instead of every type: `rg --type-list -t rust -t c` prints exactly two lines, 'c: ...' and "
     "'rust: *.rs'. Without -t, --type-list prints every type as today."),
    ("type_list_negate", "--type-list: honour -T",
     "When --type-list is combined with -T/--type-not, leave the negated types out of the list: "
     "`rg --type-list -T rust` prints every type except rust (one line fewer than plain --type-list, no "
     "line starting with 'rust:'). Without -T, --type-list prints every type as today."),

    ("replace_digit_ref", "--replace: $1x means group 1 followed by x",
     "In a --replace template, `$` followed by digits and then letters, like `$1x`, is read today as a "
     "reference to a group named '1x', which never exists, so it is replaced with nothing. Read the "
     "longest run of digits as a group number instead and keep the following letters as literal text: "
     "`echo foo | rg '(f)(o)o' -r '$1x$2y'` prints fxoy. Names that start with a letter (`$name`, "
     "`${name}`) and the braced form `${1}x` work as before."),
    ("replace_ampersand", "--replace: $& is the whole match",
     "Support `$&` in a --replace template as a synonym of `$0` (the whole match), as in Perl and "
     "JavaScript: `echo 'foo bar' | rg -o 'fo+' -r '[$&]'` prints [foo]. Today `$&` is printed literally. "
     "`$$` still stands for a literal dollar, so `-r '$$&'` prints $&."),

    ("config_tilde", "RIPGREP_CONFIG_PATH: expand ~",
     "If RIPGREP_CONFIG_PATH starts with `~/`, replace the `~` with the user's home directory (HOME on "
     "Unix) before reading the file: with HOME=/h and /h/rgrc holding the line --max-count=1, "
     "`RIPGREP_CONFIG_PATH='~/rgrc' rg foo f.txt` prints only the first matching line of f.txt. Today rg "
     "reports that it failed to read '~/rgrc' and ignores the config."),
    ("config_multiple", "RIPGREP_CONFIG_PATH: several files",
     "Let RIPGREP_CONFIG_PATH name several config files separated by the platform's path-list separator "
     "(':' on Unix, ';' on Windows). Read them in the given order and use their arguments one after the "
     "other; skip empty entries. Example: with a.rc holding --max-count=1 and b.rc holding --line-number, "
     "`RIPGREP_CONFIG_PATH=/d/a.rc:/d/b.rc rg foo f.txt` prints '1:foo' for a file whose lines are all "
     "foo. Today the whole value is taken as one file name and reading it fails."),

    # ------------------------------------------------------------------ singles: core (flags, arguments)
    ("context_num_shorthand", "accept -NUM as a context shorthand",
     "Like GNU grep, accept a dash followed by digits as a shorthand for --context: `rg -2 foo f.txt` "
     "behaves exactly like `rg -C2 foo f.txt`, and `rg -12 foo f.txt` like `rg -C12 foo f.txt`. Today rg "
     "fails with 'unrecognized flag -2'. A digit that is already a flag keeps its meaning: -0 is still "
     "--null."),
    ("type_comma_list", "-t/-T: accept a comma-separated list",
     "Let -t/--type and -T/--type-not take several type names separated by commas: `rg --files -t rust,c` "
     "lists the same files as `rg --files -t rust -t c` (a.rs and b.c, not c.py), and "
     "`rg --files -T rust,c` lists only c.py. Today rg fails with 'unrecognized file type: rust,c'."),
    ("json_include_zero", "--json: honour --include-zero",
     "With --json --include-zero, emit a begin and an end message also for every searched file that has "
     "no match: `rg --json --include-zero foo a.txt b.txt`, where only a.txt contains foo, prints begin, "
     "match and end messages for a.txt and a begin and an end message for b.txt. Without --include-zero "
     "the output is unchanged (no messages for b.txt)."),
    ("count_field_separator", "-c: honour --field-match-separator",
     "-c/--count and --count-matches always print 'path:count'. Use the --field-match-separator value "
     "between the path and the count, as the normal match output does: "
     "`rg -c --field-match-separator '=' foo a.txt c.txt` prints a.txt=1 and c.txt=2. The default "
     "separator stays ':'."),
    ("ignore_file_missing_error", "--ignore-file: a missing file is an error",
     "When a file given with --ignore-file cannot be read, rg prints the error but still exits with status "
     "0 if something matched. Keep printing the error and keep searching, but make rg exit with status 2: "
     "`rg --ignore-file missing.ignore foo a.txt` prints a.txt's matching lines, reports missing.ignore on "
     "stderr and exits 2."),
    ("engine_pcre_alias", "--engine: accept pcre",
     "Accept `pcre` as an alias of `pcre2` for --engine: `rg --engine pcre foo a.txt` behaves exactly "
     "like `rg --engine pcre2 foo a.txt` (same output and exit code; in a build without PCRE2 both report "
     "that PCRE2 is not available). Today `pcre` is rejected as an unrecognized regex engine."),
    ("path_double_slash", "paths with repeated trailing slashes",
     "A directory argument that ends with several slashes leaks them into the printed paths: "
     "`rg --files sub//` prints sub//s.txt and `rg foo sub///` prints sub///s.txt:foo. Print the paths "
     "with a single separator instead: sub/s.txt and sub/s.txt:foo. A single trailing slash (sub/) and no "
     "slash keep working as today."),
    ("stdin_label", "add --label for stdin",
     "Add a flag --label=LABEL (as in GNU grep): the name shown for standard input instead of <stdin>. "
     "`echo foo | rg -H --label in.txt foo` prints in.txt:foo, and so does "
     "`echo foo | rg -H --label in.txt foo -`. Files named on the command line are printed with their own "
     "names as before."),
    ("max_columns_preview_default", "--max-columns-preview without -M",
     "--max-columns-preview has no effect unless -M/--max-columns is also given. When it is given without "
     "a column limit, use a limit of 150 columns: for a matching line of 300 characters, "
     "`rg --max-columns-preview foo f.txt` prints the first 150 characters followed by the usual preview "
     "marker (' [... N more matches]' or ' [... omitted end of long line]'). An explicit -M wins "
     "(`-M 10 --max-columns-preview` previews 10 columns), and without --max-columns-preview nothing "
     "changes."),
    ("pre_glob_case_insensitive", "--glob-case-insensitive: also for --pre-glob",
     "--glob-case-insensitive makes -g/--glob globs case insensitive, but --pre-glob globs stay case "
     "sensitive. Apply it to --pre-glob too: `rg --pre ./p.sh --pre-glob '*.TXT' --glob-case-insensitive "
     "PREPROCESSED a.txt` runs the preprocessor on a.txt (so a preprocessor that prints PREPROCESSED makes "
     "this line match). Without --glob-case-insensitive, --pre-glob stays case sensitive."),

    # ------------------------------------------------------------------ singles: printer
    ("omitted_line_singular", "-M: '1 matches' in the omitted-line message",
     "With -M/--max-columns and a replacement or colors, a too-long line with one match is replaced by "
     "'[Omitted long line with 1 matches]'. Use the singular for one match: `rg -M 20 -r X foo f.txt` "
     "prints '[Omitted long line with 1 match]' for a long line with one foo and still "
     "'[Omitted long line with 2 matches]' for a long line with two (the same with --color always)."),
    ("max_columns_chars", "-M: count characters, not bytes",
     "-M/--max-columns compares the line length in bytes, so a line of 5 'é' and ' foo' (9 characters, 14 "
     "bytes) is omitted by `rg -M 10 foo f.txt`. For valid UTF-8, count characters instead: "
     "`rg -M 10 foo f.txt` prints the line 'ééééé foo', while `rg -M 8 foo f.txt` still prints "
     "'[Omitted long matching line]'. ASCII lines behave as today."),
    ("binary_message_null", "-0: the binary-file message ignores --null",
     "With -0/--null, paths are followed by a NUL byte, except in the binary-file message: "
     "`rg -0 foo bin.dat a.txt` prints 'bin.dat: binary file matches (found \"\\0\" byte around offset 1)'. "
     "With --null, end the path with NUL there too: 'bin.dat\\0binary file matches (...)' (same for the "
     "'WARNING: stopped searching binary file' message). Without --null nothing changes."),
    ("hyperlink_zed", "--hyperlink-format: add the zed alias",
     "Add a hyperlink format alias `zed` for the Zed editor that expands to "
     "`zed://file{path}:{line}:{column}`: `rg -H --hyperlink-format zed --color always foo a.txt` emits "
     "hyperlinks like zed://file/abs/dir/a.txt:1:1. List it with the other aliases where they are listed "
     "(for example in the error message for an invalid format)."),
    ("hyperlink_alias_case", "--hyperlink-format: aliases are case-insensitive",
     "Look up --hyperlink-format aliases case-insensitively: `rg -H --hyperlink-format VSCode --color "
     "always foo a.txt` behaves like `--hyperlink-format vscode` (emits vscode://file/... links). Today a "
     "differently cased alias is rejected because it has no {path} variable."),
    ("trim_unicode", "--trim: also strip Unicode whitespace",
     "--trim strips only ASCII whitespace from the start of printed lines. Strip leading Unicode "
     "whitespace too, such as U+00A0 NO-BREAK SPACE and U+3000 IDEOGRAPHIC SPACE: for a line made of "
     "U+00A0, U+3000, a space and 'foo', `rg --trim foo f.txt` prints 'foo'. Lines with ASCII indentation "
     "are trimmed as today."),
    ("summary_bytes_printed", "--stats: bytes printed is 0 with -c and -l",
     "With -c/--count, --count-matches, -l or --files-without-match, the --stats summary always says "
     "'0 bytes printed'. Count the bytes those modes actually print: `rg -l --stats foo a.txt c.txt` "
     "prints the two paths (12 bytes with their newlines) and then '12 bytes printed'; "
     "`rg -c --stats foo a.txt c.txt` reports the size of its 'a.txt:1' and 'c.txt:2' lines (16 bytes)."),

    # ------------------------------------------------------------------ singles: cli crate
    ("decompress_svgz", "-z: search .svgz files",
     "Make -z/--search-zip decompress `*.svgz` files (gzip-compressed SVG) with gzip: for x.svgz holding a "
     "gzip-compressed '<svg>foo</svg>', `rg -z foo x.svgz` prints <svg>foo</svg>. Today the compressed bytes "
     "are searched as they are."),
    ("decompress_uppercase", "-z: match compressed extensions case-insensitively",
     "-z/--search-zip recognises compressed files only by lower-case extensions. Match the extensions "
     "case-insensitively: `rg -z foo up.GZ` (a gzip file) and `rg -z foo y.Bz2` (a bzip2 file) search the "
     "decompressed text."),
    ("pre_exit_status", "--pre: report the exit status of a failed preprocessor",
     "When the --pre command exits with a non-zero status, the error only shows its stderr, or "
     "'<stderr is empty>'. Include the exit status in the message: for a preprocessor script that runs "
     "`exit 3`, `rg --pre ./p.sh foo a.txt` reports an error that mentions the status 3 (for example "
     "'... exited with status 3' or 'exit status: 3'); when the script also writes to stderr, both its "
     "stderr text and the status appear. rg still exits with status 2."),
    ("pattern_file_bom", "-f: ignore a UTF-8 BOM in the pattern file",
     "A pattern file that starts with a UTF-8 byte order mark (as saved by some Windows editors) makes the "
     "first pattern include the BOM, so it never matches. Drop a UTF-8 BOM at the very start of the file "
     "given to -f/--file: with pats holding BOM + 'foo' and 'bar' lines, `rg -f pats a.txt` prints a.txt's "
     "foo lines."),
    ("unescape_esc", "separators: support the \\e escape",
     "--context-separator, --field-context-separator and --field-match-separator understand the escapes "
     "\\t, \\n, \\r and \\xHH. Also accept \\e for the ESC byte (0x1B), so separators can carry terminal "
     "colors: `rg -n --field-match-separator '\\e' foo a.txt` prints 1, the byte 0x1B, then 'foo'. Today "
     "\\e is printed as the two characters backslash and e."),

    # ------------------------------------------------------------------ singles: searcher, regex, globset
    ("encoding_utf16_alias", "-E: accept utf16, utf16le and utf16be",
     "-E/--encoding accepts utf8 but not utf16, utf16le or utf16be. Accept these dash-less spellings as "
     "aliases of utf-16, utf-16le and utf-16be: `rg -E utf16le foo f.txt` finds foo in a UTF-16LE file "
     "without a BOM (as `-E utf-16le` does), and `-E utf16be` does the same for a UTF-16BE file. Today they "
     "are an 'unknown encoding' error."),
    ("smart_case_per_pattern", "-S: decide smart case per pattern",
     "With -S/--smart-case and several patterns (-e or -f), one upper-case letter in any pattern makes all "
     "of them case sensitive. Decide it for each pattern on its own: in a file with the lines foo, Foo, "
     "BAR, bar, `rg -S -e Foo -e bar` prints Foo, BAR and bar (Foo is matched case-sensitively, bar "
     "case-insensitively). A single pattern behaves as today."),
    ("glob_unicode_any", "globs: ? and [...] match one character, not one byte",
     "In globs (-g/--glob, --iglob, ignore files), `?` and bracket expressions match one byte, so they fail "
     "on non-ASCII names. Make them match one whole UTF-8 character: with the files éa.txt and ba.txt, "
     "`rg --files -g '?a.txt'` lists both, `rg --files -g '[é]a.txt'` lists éa.txt, and "
     "`rg --files -g '[!b]a.txt'` lists éa.txt."),

    # ------------------------------------------------------------------ singles: ignore crate
    ("type_odin", "add the file type odin",
     "Add a default file type `odin` for the Odin language, with the glob *.odin: `rg --type-list` "
     "includes the line 'odin: *.odin' and `rg --files -t odin` lists m.odin but not m.go."),
    ("gitignore_escaped_space", "ignore files: escaped space before trailing spaces",
     "In .gitignore/.ignore files, trailing spaces are dropped unless escaped with a backslash. A line "
     "with an escaped space followed by more unescaped spaces, like 'foo\\  ' (backslash, space, space), "
     "must keep the escaped space, as git does: it ignores the file named 'foo ' (with a trailing space) "
     "but not 'foo'. Today the line is trimmed to 'foo\\', which is an invalid glob, so nothing is "
     "ignored."),
    ("override_dot_slash", "-g: accept a leading ./",
     "A -g/--glob (or --iglob) glob that starts with ./ never matches, because the paths it is matched "
     "against have no ./ prefix. Treat a leading ./ as the current directory: `rg --files -g './sub/*.txt'` "
     "lists sub/s.txt like `-g 'sub/*.txt'`, and `rg --files -g '!./sub/*'` lists everything except the "
     "files in sub."),
    ("max_filesize_explicit", "--max-filesize: apply to explicit files",
     "--max-filesize skips large files found while walking directories, but not files named on the "
     "command line. Apply it to those too: `rg --max-filesize 1K -l foo big.txt small.txt` (big.txt is "
     "3 KiB) lists only small.txt, and `rg --max-filesize 1K foo big.txt` prints nothing and exits 1."),
    ("sapling_repo_gitignore", "treat Sapling checkouts like git repositories",
     ".gitignore files apply only inside a repository (a directory with .git or .jj) unless "
     "--no-require-git is given. Also treat a directory containing a .sl directory (a Sapling checkout) as "
     "a repository root: in a directory with .sl/, a .gitignore saying 'ign.txt', and the files ign.txt and "
     "keep.txt, `rg --files` lists only keep.txt."),
]

assert len(TASKS) == 50 and len({t[0] for t in TASKS}) == 50
assert all(a in {t[0] for t in TASKS} and b in {t[0] for t in TASKS} for a, b in PAIRS)
assert set(BOTH) == set(PAIRS) and len(set(BOTH.values())) == len(PAIRS)
