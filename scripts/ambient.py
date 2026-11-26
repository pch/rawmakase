#!/usr/bin/env python3
"""Ratchet on what the library's crates read from the process around them.

The crates `rawmakase-core` is made of take their configuration as values: a
folder, a font, the ONNX Runtime's path and the GPU's options are given by the
app that builds the context, never looked up. This check searches their
production code for the ways a crate reaches past its caller:

- `env`: `std::env` (variables, the executable's path, the home folder);
- `directories`: the `directories` crate's platform folders;
- `wgpu-env`: wgpu's `from_env` and `with_env`, which read its variables;
- `command`: `Command::new`, which starts a program;
- `folder`: the ambient data, local data and cache folders and the running
  executable's build, called with no argument (`data_dir()`,
  `local_data_dir()`, `asset_dirs()`, `cache_dir()`, `build_identity()`) or
  reached through `rawmakase_protocol::paths`, which reads them from the
  environment;
- `temp`: a temporary file or folder made with no folder given, which lands
  in the system's temporary folder.

It compares what it finds with `scripts/deps-ambient.txt`, one line per file
and kind with a count, and fails on a count the list doesn't have and on one
the code no longer reaches, so the list only shrinks. The library plan
(docs/embedding-plan.md) empties it.

    python3 scripts/ambient.py check    # run by `scripts/deps.py check` too
    python3 scripts/ambient.py report   # every site
    python3 scripts/ambient.py write    # rewrite the list from the code

Only `src/` is read; a crate's `tests/`, `examples/`, `benches/` and build
script are not. Comments, string literals, `tests.rs` files and items compiled
only for tests (`cfg(test)`, the `test-support` feature, and files declared
under either) are left out. Compile-time reads (`env!`, `option_env!`) are not
the process's environment and are not counted.
"""
import re
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from deps import attributed_end, strip_comments_and_literals  # noqa: E402

ITEM = re.compile(
    r"(?:\s*#\[[^\]]*\])*\s*(?:pub(?:\s*\([^)]*\))?\s+)?"
    r"(?:async|const|unsafe|extern|static|fn|struct|enum|union|trait|impl|type|mod|use|macro_rules!)\b"
)


def item_end(code, start):
    """End of what an attribute ending at `start` applies to. An item (a
    function, impl, module, import...) ends at its `;` or at the brace that
    closes its body, whatever commas its signature has (`-> Result<T, E>`);
    anything else (a field, a variant, a match arm, a statement) ends as
    [`attributed_end`] says."""
    if not ITEM.match(code, start):
        return attributed_end(code, start)
    depth = 0
    for k in range(start, len(code)):
        if code[k] in "([{":
            depth += 1
        elif code[k] in ")]}":
            depth -= 1
            if depth < 0:
                return k
            if depth == 0 and code[k] == "}":
                return k + 1
        elif code[k] == ";" and depth == 0:
            return k + 1
    return len(code)

ROOT = Path(__file__).resolve().parent.parent
LIST = ROOT / "scripts" / "deps-ambient.txt"

# The crates the library is made of, and the selection crate beside it, which
# an app depends on as well (docs/embedding-plan.md, "The design").
CRATES = [
    "rawmakase-core",
    "rawmakase-cancel",
    "rawmakase-tiff",
    "rawmakase-fs",
    "rawmakase-model",
    "rawmakase-interop",
    "rawmakase-raw",
    "rawmakase-engine",
    "rawmakase-decode",
    "rawmakase-export",
    "rawmakase-catalog",
    "rawmakase-inference",
    "rawmakase-denoise",
    "rawmakase-hdr-merge",
    "rawmakase-model-install",
    "rawmakase-selection",
]

KINDS = {
    "env": re.compile(r"(?<![\w:])(?:std\s*::\s*)?env\s*::\s*(?:\w+|\{)"),
    "directories": re.compile(r"(?<![\w:])directories\s*::"),
    "wgpu-env": re.compile(r"\b\w*(?:from_env|with_env)\w*\s*\("),
    "command": re.compile(r"(?<![\w])Command\s*::\s*new\b"),
    "folder": re.compile(
        r"(?<![\w.])(?<!fn )(?:data_dir|local_data_dir|xdg_data_home|asset_dirs|cache_dir|build_identity)\s*\(\s*\)"
        r"|(?<![\w:])rawmakase_protocol\s*::\s*paths\b"
    ),
    "temp": re.compile(
        r"(?<![\w])(?:tempfile|tempdir)\s*\("
        r"|(?<![\w])(?:NamedTempFile|TempDir)\s*::\s*(?:new|with_prefix|with_suffix)\s*\("
    ),
}

CFG = re.compile(r"#\[cfg\((.*?)\)\]", re.S)
TEST_ARMS = {"test", 'feature = "test-support"', "feature=\"test-support\""}


def split_arms(text):
    """Top-level comma-separated arms of a `cfg` predicate list."""
    arms, depth, current = [], 0, ""
    for ch in text + ",":
        if ch == "," and depth == 0:
            if current.strip():
                arms.append(" ".join(current.split()))
            current = ""
            continue
        depth += {"(": 1, ")": -1}.get(ch, 0)
        current += ch
    return arms


def test_only(predicate):
    """Whether a `cfg` predicate holds only in a test build: `test`, the
    `test-support` feature, an `all` with one of them, or an `any` of them alone."""
    predicate = " ".join(predicate.split())
    if predicate in TEST_ARMS:
        return True
    for combinator, need in (("all(", any), ("any(", all)):
        if predicate.startswith(combinator) and predicate.endswith(")"):
            return need(test_only(arm) for arm in split_arms(predicate[4:-1]))
    return False


_stripped = {}


def code_of(path):
    """A file's code as `blank_strings_keep_features` leaves it, read once."""
    if path not in _stripped:
        _stripped[path] = blank_strings_keep_features(path.read_text(encoding="utf-8"))
    return _stripped[path]


def blank_strings_keep_features(text):
    """Comments and literals blanked, except a `cfg` attribute's text, which
    names the feature and so must survive."""
    attributes = {m.start(): m.group(0) for m in re.finditer(r"#\[cfg\([^\]]*\)\]", text)}
    code = list(strip_comments_and_literals(text))
    for start, attribute in attributes.items():
        if "".join(code[start : start + 6]) == "#[cfg(":
            code[start : start + len(attribute)] = attribute
    return "".join(code)


def test_only_spans(code):
    return [
        (m.start(), item_end(code, m.end())) for m in CFG.finditer(code) if test_only(m.group(1))
    ]


def test_files(src):
    """Files declared under a test-only attribute, and the folders below them."""
    paths = set()
    declaration = r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;"
    for path in src.rglob("*.rs"):
        code = code_of(path)
        parent = path.parent if path.name in ("mod.rs", "lib.rs", "main.rs") else path.with_suffix("")
        for m in re.finditer(CFG.pattern + declaration, code):
            if test_only(m.group(1)):
                name = m.group(2)
                paths.update({parent / f"{name}.rs", parent / name})
    return paths


def sites():
    """{(file, kind): [line, ...]} for every production site."""
    found = defaultdict(list)
    for crate in CRATES:
        src = ROOT / "crates" / crate / "src"
        if not src.is_dir():
            sys.exit(f"{Path(__file__).name} names {crate}, which has no {src.relative_to(ROOT)}")
        skipped = test_files(src)
        for path in sorted(src.rglob("*.rs")):
            if path.name == "tests.rs" or path.name.endswith("_tests.rs"):
                continue
            if any(path == t or t in path.parents for t in skipped):
                continue
            code = code_of(path)
            excluded = test_only_spans(code)
            for kind, pattern in KINDS.items():
                for m in pattern.finditer(code):
                    if any(start <= m.start() < end for start, end in excluded):
                        continue
                    line = code.count("\n", 0, m.start()) + 1
                    found[(path.relative_to(ROOT).as_posix(), kind)].append(line)
    return found


def read_list():
    listed = {}
    for line in LIST.read_text(encoding="utf-8").splitlines():
        words = line.split("#")[0].split()
        if words:
            if len(words) != 3 or not words[2].isdigit() or words[1] not in KINDS:
                sys.exit(f"{LIST.relative_to(ROOT)}: expected `path kind count`: {line}")
            listed[(words[0], words[1])] = int(words[2])
    return listed


def check(found=None):
    found = sites() if found is None else found
    listed = read_list()
    broken = 0
    for key in sorted(set(found) | set(listed)):
        have, allowed = len(found.get(key, [])), listed.get(key, 0)
        path, kind = key
        if have > allowed:
            lines = ", ".join(str(n) for n in found[key])
            print(f"{path}: {have} `{kind}` site(s) where {LIST.relative_to(ROOT)} allows {allowed} (lines {lines})")
            broken += 1
        elif have < allowed:
            print(f"{path}: {kind} is down to {have} from {allowed}: lower it in {LIST.relative_to(ROOT)}")
            broken += 1
    if broken:
        print(
            "\nThe library's crates take their configuration as values (docs/embedding-plan.md).\n"
            "Pass the folder, font, path or option in from the caller instead of reading it here."
        )
    else:
        total = sum(len(v) for v in found.values())
        print(f"{total} environment reads, programs and loose temporary files, as listed.")
    return 1 if broken else 0


def report(found=None):
    found = sites() if found is None else found
    for (path, kind), lines in sorted(found.items()):
        print(f"{path} {kind}: lines {', '.join(str(n) for n in lines)}")
    return 0


def write(found=None):
    found = sites() if found is None else found
    header = (
        "# Environment reads, programs started and temporary files made with no folder\n"
        "# given, in the production code of the library's crates: `path kind count`.\n"
        "# Checked by scripts/ambient.py (and scripts/deps.py check). It only shrinks:\n"
        "# docs/embedding-plan.md empties it. Never raise a count to make a change pass.\n"
    )
    lines = [f"{path} {kind} {len(n)}\n" for (path, kind), n in sorted(found.items())]
    LIST.write_text(header + "".join(lines), encoding="utf-8")
    print(f"Wrote {len(lines)} lines to {LIST.relative_to(ROOT)}")
    return 0


COMMANDS = {"check": check, "report": report, "write": write}

if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in COMMANDS:
        sys.exit(f"usage: {sys.argv[0]} {{{'|'.join(COMMANDS)}}}")
    sys.exit(COMMANDS[sys.argv[1]]())
