#!/usr/bin/env python3
"""Dependency ratchet for the top-level modules of the library crate.

Production code in `src/<module>` may use another top-level module only when
`scripts/deps-allowed.txt` lists that edge. `check` fails on an edge missing
from the list, so no new dependency appears unnoticed, and on a listed edge the
code no longer has, so a removed dependency cannot quietly come back. It also
fails on a cycle, and on a module reaching, through any chain, one that
`scripts/deps-closures.txt` says it never may (the edit's values never reach
the RAW decoders or the renderer). The target layering is in docs/architecture.md.

`check` also reads the workspace's crates from `cargo metadata` (no build, no network)
and fails when a crate depends,
directly or through others, on a workspace crate `scripts/deps-crates.txt` says it
never may (the model never on the renderer, the RAW decoders on no edit).

    python3 scripts/deps.py check    # what CI runs
    python3 scripts/deps.py report   # every edge with its sites, and the cycles
    python3 scripts/deps.py write    # rewrite the allow-list from the code

The scan reads `crate::` paths, `use` trees and `super::` chains that climb to
the crate root. Comments, string literals and test code (`tests.rs` files and
`#[cfg(test)]` items) are ignored. Method calls on values of another module's
types are not paths, so they are not seen.
"""
import re
import sys
import json
import subprocess
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"
ALLOWED = ROOT / "scripts" / "deps-allowed.txt"
CLOSURES = ROOT / "scripts" / "deps-closures.txt"
CRATE_RULES = ROOT / "scripts" / "deps-crates.txt"


def blank(text):
    """Replace every character except newlines with a space, keeping line numbers."""
    return re.sub(r"[^\n]", " ", text)


def strip_comments_and_literals(text):
    """Blank comments, string literals and char literals, so only code is scanned."""
    out = []
    i, n = 0, len(text)
    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
        elif text.startswith("/*", i):
            depth, end = 1, i + 2
            while end < n and depth:
                if text.startswith("/*", end):
                    depth, end = depth + 1, end + 2
                elif text.startswith("*/", end):
                    depth, end = depth - 1, end + 2
                else:
                    end += 1
        elif (raw := re.match(r'b?r(#*)"', text[i : i + 260])) and (
            i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")
        ):
            close = '"' + raw.group(1)
            end = text.find(close, i + raw.end())
            end = n if end < 0 else end + len(close)
        elif text[i] == '"':
            end = i + 1
            while end < n and text[end] != '"':
                end += 2 if text[end] == "\\" else 1
            end += 1
        elif char := re.match(r"'(\\.[^']*|[^\\'])'", text[i : i + 12]):
            end = i + char.end()
        else:
            out.append(text[i])
            i += 1
            continue
        out.append(blank(text[i:end]))
        i = end
    return "".join(out)


def after_closing_brace(code, open_brace):
    """Position just after the brace that closes the one at `open_brace`."""
    depth = 0
    for k in range(open_brace, len(code)):
        depth += {"{": 1, "}": -1}.get(code[k], 0)
        if depth == 0:
            return k + 1
    return len(code)


def attributed_end(code, start):
    """End of what an attribute at `start` applies to: an item's `;` or closing
    brace, a field's, variant's or match arm's `,`, or the end of the block
    around it."""
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
        elif code[k] in ";," and depth == 0:
            return k + 1
    return len(code)


TEST_ATTRIBUTE = re.compile(r"#\[cfg\((?:test|all\([^()]*\btest\b[^()]*\))\)\]")


def test_spans(code):
    """Spans of `#[cfg(test)]` items: inline modules, functions, imports, impls."""
    return [(m.start(), attributed_end(code, m.end())) for m in TEST_ATTRIBUTE.finditer(code)]


def test_module_files():
    """Files declared as `#[cfg(test)] mod name;`, and directories under them."""
    paths = set()
    for path in SRC.rglob("*.rs"):
        code = strip_comments_and_literals(path.read_text(encoding="utf-8"))
        parent = path.parent if path.name in ("mod.rs", "lib.rs", "main.rs") else path.with_suffix("")
        declaration = r"(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;"
        for m in re.finditer(TEST_ATTRIBUTE.pattern + r"\s*" + declaration, code):
            name = m.group(1)
            paths.update({parent / f"{name}.rs", parent / name})
    return paths


def is_test_file(path, test_paths):
    if path.name == "tests.rs" or path.name.endswith("_tests.rs"):
        return True
    return any(path == t or t in path.parents for t in test_paths)


def top_level_modules():
    """Modules declared in src/lib.rs, which are the nodes of the graph."""
    code = strip_comments_and_literals((SRC / "lib.rs").read_text(encoding="utf-8"))
    return sorted(re.findall(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", code, re.M))


def crate_modules():
    """Modules src/lib.rs re-exports from the workspace's crates (`pub use
    rawmakase_engine::develop;`). Cargo builds those crates on their own, so they are
    no nodes of the allow-listed graph, but closure rules may still forbid them."""
    code = strip_comments_and_literals((SRC / "lib.rs").read_text(encoding="utf-8"))
    names = []
    for m in re.finditer(r"^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+rawmakase_\w+\s*::\s*(\{[^}]*\}|\w+)\s*;", code, re.M):
        names += re.findall(r"\w+", m.group(1))
    return sorted(set(names) - {"self"})


def inline_module_depth(code):
    """For each position, how many inline `mod name { ... }` blocks enclose it."""
    opens = {m.end() - 1 for m in re.finditer(r"\bmod\s+\w+\s*\{", code)}
    depths, stack, braces = [], [], 0
    for i, ch in enumerate(code):
        if ch == "{":
            braces += 1
            if i in opens:
                stack.append(braces)
        elif ch == "}":
            if stack and stack[-1] == braces:
                stack.pop()
            braces -= 1
        depths.append(len(stack))
    return depths


def group_heads(body):
    """First segment of each top-level entry of a `{...}` use group."""
    heads, depth, current = [], 0, ""
    for ch in body + ",":
        if ch == "," and depth == 0:
            if current.strip():
                heads.append(re.split(r"::|\s", current.strip())[0])
            current = ""
            continue
        depth += {"{": 1, "}": -1}.get(ch, 0)
        current += ch
    return heads


def heads_after(code, match):
    """Module names a path reaches: one name, or every head of a `{...}` group."""
    if match.group("group"):
        open_brace = match.end() - 1
        return group_heads(code[open_brace + 1 : after_closing_brace(code, open_brace) - 1])
    return [match.group("name")]


ROOT_PATH = re.compile(r"(?<![\w$])crate\s*::\s*(?:(?P<group>\{)|(?P<name>\w+))")
SUPER_PATH = re.compile(r"(?<![\w:])(?P<supers>(?:super\s*::\s*)+)(?:(?P<group>\{)|(?P<name>\w+))")


def production_edges(with_crates=False):
    """{(from, to): [site, ...]} for every production use of another module, and
    with `with_crates` of a module re-exported from a workspace crate."""
    modules = set(top_level_modules())
    targets = modules | set(crate_modules()) if with_crates else modules
    test_paths = test_module_files()
    edges = defaultdict(list)
    for path in sorted(SRC.rglob("*.rs")):
        parts = path.relative_to(SRC).with_suffix("").parts
        if parts[-1] == "mod":
            parts = parts[:-1]
        source = parts[0]
        if source not in modules or is_test_file(path, test_paths):
            continue
        code = strip_comments_and_literals(path.read_text(encoding="utf-8"))
        excluded = test_spans(code)
        depths = inline_module_depth(code)
        hits = []
        for m in ROOT_PATH.finditer(code):
            hits += [(m.start(), head) for head in heads_after(code, m)]
        for m in SUPER_PATH.finditer(code):
            if m.group("supers").count("super") == len(parts) + depths[m.start()]:
                hits += [(m.start(), head) for head in heads_after(code, m)]
        for position, target in hits:
            if target == source or target not in targets:
                continue
            if any(start <= position < end for start, end in excluded):
                continue
            line = code.count("\n", 0, position) + 1
            edges[(source, target)].append(f"{path.relative_to(ROOT)}:{line}")
    return edges


def cycles(edges):
    """Strongly connected components with more than one module (Tarjan)."""
    graph = defaultdict(set)
    for source, target in edges:
        graph[source].add(target)
    index, low, stack, on_stack, found = {}, {}, [], set(), []

    def visit(node):
        index[node] = low[node] = len(index)
        stack.append(node)
        on_stack.add(node)
        for next_node in sorted(graph[node]):
            if next_node not in index:
                visit(next_node)
                low[node] = min(low[node], low[next_node])
            elif next_node in on_stack:
                low[node] = min(low[node], index[next_node])
        if low[node] == index[node]:
            component = []
            while not component or component[-1] != node:
                component.append(stack.pop())
                on_stack.discard(component[-1])
            if len(component) > 1:
                found.append(sorted(component))

    for node in sorted(set(graph) | {t for ts in graph.values() for t in ts}):
        if node not in index:
            visit(node)
    return found


def edge_line(edge):
    return f"{edge[0]} -> {edge[1]}"


def read_allowed():
    lines = ALLOWED.read_text(encoding="utf-8").splitlines()
    entries = [line.split("#")[0].strip() for line in lines]
    return {tuple(part.strip() for part in e.split("->")) for e in entries if e}


def reaches(edges, module):
    """Every module `module` uses, directly or through others."""
    graph = defaultdict(set)
    for source, target in edges:
        graph[source].add(target)
    seen, todo = set(), [module]
    while todo:
        for target in graph[todo.pop()]:
            if target not in seen:
                seen.add(target)
                todo.append(target)
    return seen


def read_closures():
    """(module, forbidden modules) from deps-closures.txt: `catalog never raw develop`."""
    return read_rules(CLOSURES)


def path_to(edges, source, target):
    """One chain of modules from `source` to `target`, for the message."""
    graph = defaultdict(set)
    for a, b in edges:
        graph[a].add(b)
    previous, todo = {source: None}, [source]
    while todo:
        node = todo.pop(0)
        for next_node in sorted(graph[node]):
            if next_node not in previous:
                previous[next_node] = node
                todo.append(next_node)
    chain, node = [], target
    while node is not None:
        chain.append(node)
        node = previous[node]
    return " -> ".join(reversed(chain))


def check_closures(edges):
    broken = 0
    known = set(top_level_modules()) | set(crate_modules())
    for module, forbidden in read_closures():
        for name in [module, *forbidden]:
            if name not in known:
                print(f"{CLOSURES.relative_to(ROOT)} names {name}, which is no top-level module")
                broken += 1
        reached = reaches(edges, module)
        for other in forbidden:
            if other in reached:
                print(f"{module} must not reach {other}, but does: {path_to(edges, module, other)}")
                broken += 1
    return broken


def read_rules(path):
    """(name, forbidden names) from a rules file: `catalog never raw develop`."""
    rules = []
    for line in path.read_text(encoding="utf-8").splitlines():
        words = line.split("#")[0].split()
        if words:
            if len(words) < 3 or words[1] != "never":
                sys.exit(f"{path.relative_to(ROOT)}: expected `name never other...`: {line}")
            rules.append((words[0], words[2:]))
    return rules


def workspace_crates():
    """{crate: {workspace crates it depends on}}: normal and build dependencies, on
    every platform. Dev-dependencies are left out: they build only the crate's own
    tests. Cargo resolves membership, inherited and renamed dependencies."""
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"],
        cwd=ROOT, capture_output=True, text=True,
    )
    if out.returncode:
        sys.exit(f"cargo metadata failed:\n{out.stderr}")
    packages = json.loads(out.stdout)["packages"]
    names = {p["name"] for p in packages}
    return {
        p["name"]: {d["name"] for d in p["dependencies"] if d["kind"] != "dev" and d["name"] in names}
        for p in packages
    }


def check_crates():
    """Crate rules from deps-crates.txt against the Cargo.toml files."""
    graph = workspace_crates()
    edges = {(a, b) for a, deps in graph.items() for b in deps}
    broken = 0
    rules = read_rules(CRATE_RULES)
    for crate, forbidden in rules:
        for name in [crate, *forbidden]:
            if name not in graph:
                print(f"{CRATE_RULES.relative_to(ROOT)} names {name}, which is no workspace crate")
                broken += 1
        reached = reaches(edges, crate)
        for other in forbidden:
            if other in reached:
                print(f"{crate} must not depend on {other}, but does: {path_to(edges, crate, other)}")
                broken += 1
    return broken, len(rules)


def check(edges):
    allowed = read_allowed()
    new = sorted(set(edges) - allowed)
    stale = sorted(allowed - set(edges))
    for edge in new:
        print(f"New dependency {edge_line(edge)}, used at:")
        for site in edges[edge]:
            print(f"    {site}")
    for edge in stale:
        print(f"{edge_line(edge)} is no longer used: delete it from {ALLOWED.relative_to(ROOT)}")
    if new:
        print(
            "\nA new edge between top-level modules needs a reason. If it fits the layering in\n"
            "docs/architecture.md, add it to the allow-list in the same pull request."
        )
    found = cycles(edges)
    for component in found:
        print("Cycle between " + ", ".join(component))
    broken = check_closures(production_edges(with_crates=True))
    broken_crates, crate_rules = check_crates()
    broken += broken_crates
    if not new and not stale and not found and not broken:
        rules = len(read_closures())
        print(
            f"{len(edges)} module dependencies, all allowed; no cycles; {rules} closure rules"
            f" and {crate_rules} crate rules hold."
        )
    return 1 if new or stale or found or broken else 0


def report(edges):
    for edge in sorted(edges):
        print(f"{edge_line(edge)} ({len(edges[edge])})")
        for site in edges[edge]:
            print(f"    {site}")
    found = cycles(edges)
    print(f"\n{len(found)} cycle(s)")
    for component in found:
        print("    " + ", ".join(component))
    return 0


def write(edges):
    header = (
        "# Allowed dependencies between top-level modules, checked by scripts/deps.py.\n"
        "# Delete a line when the code stops needing it. A new line needs a reason that\n"
        "# fits the layering in docs/architecture.md.\n"
    )
    ALLOWED.write_text(header + "".join(edge_line(e) + "\n" for e in sorted(edges)), encoding="utf-8")
    print(f"Wrote {len(edges)} edges to {ALLOWED.relative_to(ROOT)}")
    return 0


COMMANDS = {"check": check, "report": report, "write": write}

if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in COMMANDS:
        sys.exit(f"usage: {sys.argv[0]} {{{'|'.join(COMMANDS)}}}")
    sys.exit(COMMANDS[sys.argv[1]](production_edges()))
