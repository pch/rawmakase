#!/usr/bin/env python3
"""The library's public items, listed and compared, so a change to its surface
is seen in review (docs/embedding-plan.md, "One library, built from the
crates").

    python3 scripts/public-api.py check   # what CI runs
    python3 scripts/public-api.py write   # rewrite the listing from the code

It reads rustdoc's JSON for `rawmakase-core`, which needs a nightly toolchain
(TOOLCHAIN below; `rustup toolchain install` it once), and lists every item an
app can name: the library's own, and what it re-exports from the workspace's
crates, followed into those crates so a re-exported type's fields, methods and
trait implementations are listed under the library's path too. One line per
item, sorted, in `crates/rawmakase-core/public-api.txt`. `check` fails when the
code and the listing differ; a change to the surface rewrites the listing in
the same commit and says so in docs/embedding.md's list of changes.

Blanket implementations (`impl<T> From<T> for T`), auto traits other than
`Send` and `Sync`, and `StructuralPartialEq` are left out: they say nothing an
app relies on. A type is named by the path an app names it by, so a re-exported
item reads as the library's.
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATE = "rawmakase-core"
LISTING = ROOT / "crates" / CRATE / "public-api.txt"
# The nightly whose rustdoc JSON format (version FORMAT) this script reads.
TOOLCHAIN = "nightly-2026-11-02"
FORMAT = 61
KEPT_AUTO = {"Send", "Sync"}
# Marker traits the compiler implements for a derive, which say nothing more.
LEFT_OUT = {"StructuralPartialEq"}


def rustdoc(package):
    """rustdoc's JSON for one workspace package's library."""
    target = ROOT / "target" / "public-api"
    out = subprocess.run(
        ["cargo", f"+{TOOLCHAIN}", "rustdoc", "--locked", "-q", "-p", package, "--lib",
         "--target-dir", str(target), "--", "-Zunstable-options", "--output-format", "json"],
        cwd=ROOT, capture_output=True, text=True,
    )
    if out.returncode:
        sys.exit(f"rustdoc failed for {package} (is {TOOLCHAIN} installed?):\n{out.stderr}")
    doc = json.loads((target / "doc" / f"{package.replace('-', '_')}.json").read_text())
    if doc["format_version"] != FORMAT:
        sys.exit(f"rustdoc JSON format {doc['format_version']}, this script reads {FORMAT}")
    return doc


class Crate:
    """One crate's rustdoc JSON, with a renderer for its types."""

    def __init__(self, doc):
        self.doc = doc
        self.index = doc["index"]
        self.paths = doc["paths"]
        # The library's path of each item it shows, set as the listing walks.
        self.public = {}

    def item(self, id_):
        return self.index.get(str(id_))

    def path_of(self, id_, fallback):
        if str(id_) in self.public:
            return self.public[str(id_)]
        entry = self.paths.get(str(id_))
        return "::".join(entry["path"]) if entry else fallback

    def ty(self, t):
        if t is None:
            return "()"
        (kind, v), = t.items()
        if kind == "resolved_path":
            return self.path_of(v["id"], v["path"]) + self.args(v.get("args"))
        if kind in ("generic", "primitive"):
            return v
        if kind == "borrowed_ref":
            life = f"{v['lifetime']} " if v.get("lifetime") else ""
            return f"&{life}{'mut ' if v['is_mutable'] else ''}{self.ty(v['type'])}"
        if kind == "raw_pointer":
            return f"*{'mut' if v['is_mutable'] else 'const'} {self.ty(v['type'])}"
        if kind == "tuple":
            return "(" + ", ".join(self.ty(x) for x in v) + ("," if len(v) == 1 else "") + ")"
        if kind == "slice":
            return f"[{self.ty(v)}]"
        if kind == "array":
            return f"[{self.ty(v['type'])}; {v['len']}]"
        if kind == "impl_trait":
            return "impl " + " + ".join(self.bound(b) for b in v)
        if kind == "dyn_trait":
            traits = [self.path_of(p["trait"]["id"], p["trait"]["path"]) + self.args(p["trait"].get("args"))
                      for p in v["traits"]]
            return "dyn " + " + ".join(traits + ([v["lifetime"]] if v.get("lifetime") else []))
        if kind == "qualified_path":
            trait = v.get("trait")
            base = f"<{self.ty(v['self_type'])} as {self.path_of(trait['id'], trait['path'])}>" if trait \
                else self.ty(v["self_type"])
            return f"{base}::{v['name']}"
        if kind == "function_pointer":
            sig = v["sig"]
            return "fn(" + ", ".join(self.ty(x) for _, x in sig["inputs"]) + ")" + self.ret(sig["output"])
        return kind

    def args(self, a):
        if not a:
            return ""
        if "angle_bracketed" in a:
            parts = []
            for arg in a["angle_bracketed"]["args"]:
                (k, v), = arg.items()
                parts.append(self.ty(v) if k == "type" else v if k == "lifetime" else
                             v.get("expr", "_") if k == "const" else "_")
            for c in a["angle_bracketed"].get("constraints", []):
                binding = c["binding"]
                rhs = self.ty(binding["equality"]["type"]) if "equality" in binding and "type" in binding["equality"] \
                    else " + ".join(self.bound(b) for b in binding.get("constraint", []))
                parts.append(f"{c['name']} = {rhs}")
            return f"<{', '.join(parts)}>" if parts else ""
        if "parenthesized" in a:
            p = a["parenthesized"]
            return "(" + ", ".join(self.ty(x) for x in p["inputs"]) + ")" + self.ret(p.get("output"))
        return ""

    def ret(self, output):
        return f" -> {self.ty(output)}" if output else ""

    def bound(self, b):
        if "trait_bound" in b:
            t = b["trait_bound"]["trait"]
            modifier = "?" if b["trait_bound"].get("modifier") == "maybe" else ""
            return modifier + self.path_of(t["id"], t["path"]) + self.args(t.get("args"))
        if "outlives" in b:
            return b["outlives"]
        return "_"

    def generics(self, g):
        params = []
        for p in g["params"]:
            kind = p["kind"]
            if "type" in kind:
                if kind["type"].get("is_synthetic"):
                    continue
                bounds = " + ".join(self.bound(b) for b in kind["type"]["bounds"])
                params.append(p["name"] + (f": {bounds}" if bounds else ""))
            elif "lifetime" in kind:
                params.append(p["name"])
            elif "const" in kind:
                params.append(f"const {p['name']}: {self.ty(kind['const']['type'])}")
        return f"<{', '.join(params)}>" if params else ""


class Lister:
    def __init__(self, crates):
        self.crates = crates
        self.lines = set()

    def external(self, crate_id, crate):
        """The workspace crate an external id belongs to, or None for std's and
        third parties' (which are named, not followed)."""
        name = crate.doc["external_crates"][str(crate_id)]["name"]
        if not name.startswith("rawmakase_"):
            return None
        key = name
        if key not in self.crates:
            self.crates[key] = Crate(rustdoc(name.replace("_", "-")))
        return self.crates[key]

    def walk_module(self, crate, item, prefix):
        for child in item["inner"]["module"]["items"]:
            self.walk(crate, crate.item(child), prefix)

    def walk(self, crate, item, prefix):
        if item is None or item["visibility"] != "public":
            return
        (kind, v), = item["inner"].items()
        if kind == "use":
            self.walk_use(crate, v, prefix)
            return
        name = item["name"]
        path = f"{prefix}::{name}"
        crate.public.setdefault(str(item["id"]), path)
        if kind == "module":
            self.lines.add(f"pub mod {path}")
            self.walk_module(crate, item, path)
        elif kind == "struct":
            self.lines.add(f"pub struct {path}{crate.generics(v['generics'])}")
            fields = v["kind"].get("plain", {}).get("fields", []) if isinstance(v["kind"], dict) else []
            tuple_fields = v["kind"].get("tuple", []) if isinstance(v["kind"], dict) else []
            for f in fields:
                field = crate.item(f)
                if field and field["visibility"] == "public":
                    self.lines.add(f"pub {path}::{field['name']}: {crate.ty(field['inner']['struct_field'])}")
            for i, f in enumerate(tuple_fields):
                field = crate.item(f) if f is not None else None
                if field and field["visibility"] == "public":
                    self.lines.add(f"pub {path}::{i}: {crate.ty(field['inner']['struct_field'])}")
            self.impls(crate, v["impls"], path)
        elif kind == "enum":
            attrs = " ".join(a for a in item.get("attrs", []) if "non_exhaustive" in str(a))
            self.lines.add(f"pub enum {path}{crate.generics(v['generics'])}{' #[non_exhaustive]' if attrs else ''}")
            for variant in v["variants"]:
                self.lines.add(f"pub {path}::{crate.item(variant)['name']}")
            self.impls(crate, v["impls"], path)
        elif kind == "function":
            self.lines.add(self.function(crate, item, path))
        elif kind in ("constant", "static"):
            t = v["type"] if kind == "constant" else v["type"]
            self.lines.add(f"pub {'const' if kind == 'constant' else 'static'} {path}: {crate.ty(t)}")
        elif kind == "type_alias":
            self.lines.add(f"pub type {path}{crate.generics(v['generics'])} = {crate.ty(v['type'])}")
        elif kind == "trait":
            self.lines.add(f"pub trait {path}")
        else:
            self.lines.add(f"pub {kind} {path}")

    def walk_use(self, crate, use, prefix):
        target = crate.item(use["id"]) if use.get("id") is not None else None
        if target is not None:
            if use["is_glob"]:
                self.walk_module(crate, target, prefix)
            else:
                self.walk(crate, {**target, "name": use["name"]}, prefix)
            return
        entry = crate.paths.get(str(use["id"])) if use.get("id") is not None else None
        path = f"{prefix}::{use['name']}"
        if entry is None:
            self.lines.add(f"pub use {path} = {use['source']}")
            return
        other = self.external(entry["crate_id"], crate)
        if other is None:
            self.lines.add(f"pub use {path} = {'::'.join(entry['path'])}")
            return
        # The same item in its own crate's documentation, found by its path.
        wanted = entry["path"]
        for id_, e in other.paths.items():
            if e["crate_id"] == 0 and e["path"] == wanted and other.item(id_):
                found = other.item(id_)
                if use["is_glob"]:
                    self.walk_module(other, found, prefix)
                else:
                    self.walk(other, {**found, "name": use["name"]}, prefix)
                return
        self.lines.add(f"pub use {path} = {'::'.join(wanted)}")

    def impls(self, crate, ids, path):
        for id_ in ids:
            imp = crate.item(id_)
            if imp is None:
                continue
            v = imp["inner"]["impl"]
            if v.get("blanket_impl") is not None:
                continue
            trait = v.get("trait")
            if trait is None:
                for m in v["items"]:
                    method = crate.item(m)
                    if method and method["visibility"] == "public" and "function" in method["inner"]:
                        self.lines.add(self.function(crate, method, f"{path}::{method['name']}"))
                    elif method and method["visibility"] == "public" and "assoc_const" in method["inner"]:
                        c = method["inner"]["assoc_const"]
                        self.lines.add(f"pub const {path}::{method['name']}: {crate.ty(c['type'])}")
                continue
            name = trait["path"].split("::")[-1]
            if (v.get("is_synthetic") and name not in KEPT_AUTO) or name in LEFT_OUT:
                continue
            negative = "!" if v.get("is_negative") else ""
            full = crate.path_of(trait["id"], trait["path"]) + crate.args(trait.get("args"))
            self.lines.add(f"impl {negative}{full} for {path}")

    def function(self, crate, item, path):
        f = item["inner"]["function"]
        sig = f["sig"]
        inputs = []
        for name, t in sig["inputs"]:
            if name == "self":
                rendered = crate.ty(t)
                inputs.append({"Self": "self", "&Self": "&self", "&mut Self": "&mut self"}.get(rendered, f"self: {rendered}"))
            else:
                inputs.append(f"{name}: {crate.ty(t)}")
        header = f["header"]
        quals = ("const " if header.get("is_const") else "") + ("unsafe " if header.get("is_unsafe") else "")
        return f"pub {quals}fn {path}{crate.generics(f['generics'])}({', '.join(inputs)}){crate.ret(sig['output'])}"


def listing():
    core = Crate(rustdoc(CRATE))
    root = core.item(core.doc["root"])
    # Twice: the first walk learns each item's public path, which the second
    # names it by wherever it appears.
    crates = {0: core}
    for _ in range(2):
        lister = Lister(crates)
        lister.walk_module(core, root, root["name"])
    return sorted(lister.lines)


def check():
    have = listing()
    want = LISTING.read_text(encoding="utf-8").splitlines() if LISTING.exists() else []
    want = [line for line in want if line and not line.startswith("#")]
    added = sorted(set(have) - set(want))
    removed = sorted(set(want) - set(have))
    for line in added:
        print(f"+ {line}")
    for line in removed:
        print(f"- {line}")
    if added or removed:
        print(
            f"\nThe library's public items differ from {LISTING.relative_to(ROOT)}. If the change is\n"
            "meant, run `python3 scripts/public-api.py write` and note it in docs/embedding.md."
        )
        return 1
    print(f"{len(have)} public items of {CRATE}, as listed.")
    return 0


def write():
    lines = listing()
    header = (
        f"# The public items of {CRATE}, the re-exported ones among them, written by\n"
        "# scripts/public-api.py and compared in CI. A change here is a change to the\n"
        "# library's surface: note it in docs/embedding.md.\n"
    )
    LISTING.write_text(header + "".join(f"{line}\n" for line in lines), encoding="utf-8")
    print(f"Wrote {len(lines)} items to {LISTING.relative_to(ROOT)}")
    return 0


COMMANDS = {"check": check, "write": write}

if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in COMMANDS:
        sys.exit(f"usage: {sys.argv[0]} {{{'|'.join(COMMANDS)}}}")
    sys.exit(COMMANDS[sys.argv[1]]())
