"""The `long` development set: 9 tasks for 9 Claude Code agents on networkx (dev/README.md, "Long set").

A set for long agent work: a repository whose suite takes minutes, so an agent that tests waits, and tasks of
300 or more changed source lines each that need reading of several modules. All nine work on the text formats
of `networkx/readwrite`: the edge list, adjacency list and multiline adjacency list (`edgelist.py`,
`adjlist.py`, `multiline_adjlist.py`), and Pajek, LEDA, GML, graph6, sparse6, GraphML and GEXF where a task
widens to them.

- Features l1 to l6: quoted names (l1), a graph header line with the type and name (l2), errors with line
  numbers, also of Pajek and LEDA (l3), sorted output and a float format, also of Pajek and GML (l4), byte
  order marks and line ends, also of Pajek and LEDA (l5), compression by content in ten formats (l6).
- Refactors l7 to l9 move code that those features change into a new module with a class, and keep the public
  functions: l7 the parse functions into `LineParser` (l1, l2, l3, l5 change those bodies), l8 the generate
  functions and Pajek's into `LineWriter` (l1, l2, l4), l9 the opening, decoding and encoding of the read and
  write functions into `TextFile` (l2, l3, l5, l6).

The refactors come last in the reference order. The steps of l7 and l8 move the bodies word for word (local
names route the features' lines through the new hooks), so TreeYard's `undoes` check stays quiet on them; l9
puts five copies of the features' decoding and encoding lines into one class, so it removes them by design and
its oracle lands with `undo` (`oracle_undo`). The data is in `long/`: `prompts/<id>.md`,
`patches/<k>-<id>.alone.patch` (the task on the pin), `<k>-<id>.step.patch` (the task on tasks 1..k-1, where
it differs), `without-<k>.patch` (trees of all tasks but k), `tests/`, `integration/`. How they were made:
`long/README.md`.
"""

from pathlib import Path

HERE = Path(__file__).resolve().parent / "long"

SUFFIX = "\n\nKeep the existing behaviour otherwise. Finish the work completely.\n"


def ops_from_patch(text):
    """The edit ops of a `git diff` text: `replace` (hunk context and old lines -> context and new lines)
    for a changed file, `create` for a new one."""
    ops, path, new_file, old, new = [], None, False, None, None

    def flush():
        if old is None:
            return
        if new_file:
            ops.append({"op": "create", "file": path, "text": "".join(new)})
        else:
            ops.append({"op": "replace", "file": path, "old": "".join(old), "new": "".join(new)})

    for line in text.splitlines(True):
        if line.startswith("diff --git "):
            flush()
            old = new = None
            path, new_file = line.split(" b/", 1)[1].strip(), False
        elif line.startswith("new file mode"):
            new_file = True
        elif line.startswith(("--- ", "+++ ", "index ", "similarity", "rename ")) and old is None:
            continue
        elif line.startswith("@@"):
            if not new_file:
                flush()
                old, new = [], []
            elif old is None:
                old, new = [], []
        elif old is not None and line.startswith("\\"):
            continue
        elif old is not None:
            tag, body = line[:1], line[1:]
            if tag in (" ", "-"):
                old.append(body)
            if tag in (" ", "+"):
                new.append(body)
    flush()
    return ops


def patch(k, tid, kind):
    f = HERE / "patches" / f"{k}-{tid}.{kind}.patch"
    return f.read_text() if f.is_file() else None


def prompt_file(path):
    """(title, prompt) of `prompts/<id>.md`: a `# title` line, a blank line, the prompt."""
    title, _, text = path.read_text().partition("\n\n")
    return title.removeprefix("# "), text.rstrip("\n")


PROMPTS = {p.stem: prompt_file(p) for p in sorted((HERE / "prompts").glob("*.md"), key=lambda p: int(p.stem[1:].split("-")[0]))}

# l9 removes copies of other tasks' lines by design: its oracle overrides TreeYard's hold (arm BN).
ORACLE_UNDO = {"l9-text-file": "This refactor moves the decoding and encoding code of the five line formats into "
                                "TextFile: one copy replaces five."}

IDS = list(PROMPTS)


def task(k, tid):
    title, prompt = PROMPTS[tid]
    t = {"id": tid, "title": title, "prompt": prompt,
         "edits": ops_from_patch(patch(k, tid, "alone")),
         "test": (HERE / "tests" / f"{tid.replace('-', '_')}.py").read_text()}
    if tid in ORACLE_UNDO:
        t["oracle_undo"] = ORACLE_UNDO[tid]
    step = patch(k, tid, "step")
    if step is not None:
        t["combined_edits"] = ops_from_patch(step)
    return t


NETWORKX_LONG = [task(k, tid) for k, tid in enumerate(IDS, 1)]

# Integration tests (1-based task numbers): a feature seen through the new class of a refactor, or two features
# across one interface. Each rests only on behaviour that the prompts of both tasks state.
INTEGRATION = [
    {"id": "i1-parser-quoted-names", "pair": [7, 1], "why": "refactor: l1's quoted names must be read by LineParser"},
    {"id": "i2-parser-graph-header", "pair": [7, 2], "why": "refactor: l2's header must be read by LineParser"},
    {"id": "i3-parser-line-errors", "pair": [7, 3], "why": "refactor: LineParser's methods raise l3's LineParseError"},
    {"id": "i4-parser-line-ends", "pair": [7, 5], "why": "refactor: LineParser.split drops l5's line ends"},
    {"id": "i5-writer-quoted-names", "pair": [8, 1], "why": "refactor: LineWriter.name quotes l1's names"},
    {"id": "i6-writer-output-format", "pair": [8, 4], "why": "refactor: l4's sort and float_format are LineWriter options"},
    {"id": "i7-writer-graph-header", "pair": [8, 2], "why": "refactor: l2's header and comments are LineWriter options"},
    {"id": "i8-file-graph-header", "pair": [9, 2], "why": "refactor: l2's header line goes through TextFile.write_lines"},
    {"id": "i9-file-bytes", "pair": [9, 5], "why": "refactor: l5's byte order marks and line ends are TextFile's"},
    {"id": "i10-file-line-errors", "pair": [9, 3], "why": "refactor: TextFile.read names the file of l3's errors"},
    {"id": "i11-file-compression", "pair": [9, 6], "why": "refactor: l6's compression is TextFile's"},
    {"id": "i12-header-file-bytes", "pair": [2, 5], "why": "interface: the header after l5's byte order mark, ended by its newline"},
]
for it in INTEGRATION:
    it["test"] = (HERE / "integration" / f"{it['id'].replace('-', '_')}.py").read_text()

# Set-level settings (tasks/networkx/long/set.json): 9 agents, all Claude Code. Limits for long work: the
# suite takes minutes and agents run it several times, so an agent may need an hour or more of wall time. The
# cost cap per agent is the bound that matters: 9 x 1.5 USD = 13.5 USD at most per run, so one solo pass and one
# 9-agent run stay under 30 USD even if every agent hits it (hard-set solo agents cost 0.10 to 0.32 USD each).
SETTINGS = {
    "agents": 9,
    "mix": ["claude"] * 9,
    "limits": {"max_turns": 600, "max_tokens": 40_000_000, "timeout_s": 7200, "claude_budget_usd": 1.5},
    "sequential": True,
}

# Trees of all tasks but k, written by hand (t2.replay_without gives broken trees: the refactor steps are written
# on the features' code).
WITHOUT = {"networkx": {"long": {int(f.name.split("-")[1].split(".")[0]): ops_from_patch(f.read_text())
                                 for f in sorted((HERE / "patches").glob("without-*.patch"))}}}

TASKS = {"networkx": {"long": NETWORKX_LONG}}
INTEGRATIONS = {"networkx": {"long": INTEGRATION}}
SET_SETTINGS = {"long": SETTINGS}
SETS = ("long",)
