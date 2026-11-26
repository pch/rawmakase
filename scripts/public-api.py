#!/usr/bin/env python3
"""The library's public items, listed and compared, so a change to its surface
is seen in review (docs/embedding-plan.md, "One library, built from the
crates").

    python3 scripts/public-api.py check   # what CI runs
    python3 scripts/public-api.py write   # rewrite the listing from the code

It reads rustdoc's JSON for `rawmakase-core` with its default features, which
needs a nightly toolchain (TOOLCHAIN below; `rustup toolchain install` it once),
and lists every item an app can name: the library's own, and what it re-exports
from the workspace's crates, followed into those crates so a re-exported type's
fields, methods and trait implementations are listed under the library's path
too. One line per item, sorted, in `crates/rawmakase-core/public-api.txt`.
`check` fails when the code and the listing differ; a change to the surface
rewrites the listing in the same commit and says so in docs/embedding.md's list
of changes.

Each line carries what a change to would break an app: a function's
qualifiers, generics with their bounds and defaults, `where` clauses,
parameters and result; a type's generics and `#[non_exhaustive]`; each enum
variant's fields; a trait's supertraits and items; each implementation's
generics and the type it is for. A type is named by the path an app names it
by, so a re-exported item reads as the library's wherever it appears.

Blanket implementations (`impl<T> From<T> for T`), auto traits other than
`Send` and `Sync`, and `StructuralPartialEq` are left out: they say nothing an
app relies on. Items hidden from the documentation are left out by rustdoc.
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


def rustdoc(package, workspace=ROOT):
    """rustdoc's JSON for the library of `package` in `workspace`."""
    target = ROOT / "target" / "public-api"
    out = subprocess.run(
        ["cargo", f"+{TOOLCHAIN}", "rustdoc", "--locked", "-q", "-p", package, "--lib",
         "--target-dir", str(target), "--", "-Zunstable-options", "--output-format", "json"],
        cwd=workspace, capture_output=True, text=True,
    )
    if out.returncode:
        sys.exit(f"rustdoc failed for {package} (is {TOOLCHAIN} installed?):\n{out.stderr}")
    doc = json.loads((target / "doc" / f"{package.replace('-', '_')}.json").read_text())
    if doc["format_version"] != FORMAT:
        sys.exit(f"rustdoc JSON format {doc['format_version']}, this script reads {FORMAT}")
    return doc


class Crate:
    """One crate's rustdoc JSON, rendered with the listing's shared names."""

    def __init__(self, doc, public):
        self.doc = doc
        self.index = doc["index"]
        self.paths = doc["paths"]
        self.name = self.index[str(doc["root"])]["name"]
        # The library's path of each item it shows, by (crate, its own path),
        # shared by every crate the listing reads.
        self.public = public

    def item(self, id_):
        return self.index.get(str(id_)) if id_ is not None else None

    def canonical(self, id_):
        """(crate, path) of an item this crate refers to by `id_`."""
        entry = self.paths.get(str(id_))
        if entry is None:
            return None
        crate = self.name if entry["crate_id"] == 0 else \
            self.doc["external_crates"][str(entry["crate_id"])]["name"]
        return crate, tuple(entry["path"])

    def path_of(self, id_, fallback):
        key = self.canonical(id_)
        if key is None:
            return fallback
        return self.public.get(key, "::".join(key[1]))

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
            traits = [self.binder(p.get("generic_params", [])) + self.trait_path(p["trait"]) for p in v["traits"]]
            return "dyn " + " + ".join(traits + ([v["lifetime"]] if v.get("lifetime") else []))
        if kind == "qualified_path":
            trait = v.get("trait")
            base = f"<{self.ty(v['self_type'])} as {self.trait_path(trait)}>" if trait \
                else self.ty(v["self_type"])
            return f"{base}::{v['name']}{self.args(v.get('args'))}"
        if kind == "function_pointer":
            sig = v["sig"]
            inputs = [self.ty(x) for _, x in sig["inputs"]] + (["..."] if sig.get("is_c_variadic") else [])
            return (self.binder(v.get("generic_params", [])) + qualifiers(v.get("header", {}))
                    + "fn(" + ", ".join(inputs) + ")" + self.ret(sig["output"]))
        return kind

    def trait_path(self, t):
        return self.path_of(t["id"], t["path"]) + self.args(t.get("args"))

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
                if "equality" in binding:
                    term = binding["equality"]
                    rhs = self.ty(term["type"]) if "type" in term else term.get("constant", {}).get("expr", "_")
                    parts.append(f"{c['name']} = {rhs}")
                else:
                    parts.append(f"{c['name']}: " + " + ".join(self.bound(b) for b in binding["constraint"]))
            return f"<{', '.join(parts)}>" if parts else ""
        if "parenthesized" in a:
            p = a["parenthesized"]
            return "(" + ", ".join(self.ty(x) for x in p["inputs"]) + ")" + self.ret(p.get("output"))
        return ""

    def ret(self, output):
        return f" -> {self.ty(output)}" if output else ""

    def bound(self, b):
        if "trait_bound" in b:
            t = b["trait_bound"]
            modifier = {"maybe": "?", "maybe_const": "~const "}.get(t.get("modifier"), "")
            lifetimes = self.binder(t.get("generic_params", []))
            return lifetimes + modifier + self.trait_path(t["trait"])
        if "outlives" in b:
            return b["outlives"]
        if "use" in b:
            return "use<" + ", ".join(p.get("lifetime", p.get("param", "_")) if isinstance(p, dict) else p
                                      for p in b["use"]) + ">"
        return "_"

    def binder(self, params):
        names = [p["name"] for p in params]
        return f"for<{', '.join(names)}> " if names else ""

    def generics(self, g):
        params = []
        for p in g["params"]:
            kind = p["kind"]
            if "type" in kind:
                k = kind["type"]
                if k.get("is_synthetic"):
                    continue
                text = p["name"]
                if k["bounds"]:
                    text += ": " + " + ".join(self.bound(b) for b in k["bounds"])
                if k.get("default") is not None:
                    text += f" = {self.ty(k['default'])}"
                params.append(text)
            elif "lifetime" in kind:
                outlives = kind["lifetime"].get("outlives", [])
                params.append(p["name"] + (f": {' + '.join(outlives)}" if outlives else ""))
            elif "const" in kind:
                k = kind["const"]
                text = f"const {p['name']}: {self.ty(k['type'])}"
                if k.get("default") is not None:
                    text += f" = {k['default']}"
                params.append(text)
        return f"<{', '.join(params)}>" if params else ""

    def where(self, g):
        clauses = []
        for w in g.get("where_predicates", []):
            if "bound_predicate" in w:
                b = w["bound_predicate"]
                clauses.append(self.binder(b.get("generic_params", [])) + self.ty(b["type"]) + ": "
                               + " + ".join(self.bound(x) for x in b["bounds"]))
            elif "lifetime_predicate" in w:
                lp = w["lifetime_predicate"]
                clauses.append(f"{lp['lifetime']}: {' + '.join(lp['outlives'])}")
            elif "eq_predicate" in w:
                e = w["eq_predicate"]
                rhs = e["rhs"]
                clauses.append(f"{self.ty(e['lhs'])} = " + (self.ty(rhs["type"]) if "type" in rhs else "_"))
        return f" where {', '.join(clauses)}" if clauses else ""


def qualifiers(header):
    """`const async unsafe extern "C"`, as a function's header has them."""
    abi = header.get("abi", "Rust")
    if isinstance(abi, dict):
        (name, fields), = abi.items()
        abi = name + ("-unwind" if isinstance(fields, dict) and fields.get("unwind") else "")
    return "".join([
        "const " if header.get("is_const") else "",
        "async " if header.get("is_async") else "",
        "unsafe " if header.get("is_unsafe") else "",
        "" if abi == "Rust" else f'extern "{abi}" ',
    ])


def non_exhaustive(item):
    return " #[non_exhaustive]" if any("non_exhaustive" in str(a) for a in item.get("attrs", [])) else ""


class Lister:
    def __init__(self, crates, public, workspace):
        self.crates = crates
        self.public = public
        self.workspace = workspace
        self.lines = set()

    def other(self, crate_name):
        """A workspace crate's documentation, read once; None for std's and
        third parties' crates, whose items are named, not followed."""
        if not crate_name.startswith("rawmakase_"):
            return None
        if crate_name not in self.crates:
            doc = rustdoc(crate_name.replace("_", "-"), self.workspace)
            self.crates[crate_name] = Crate(doc, self.public)
        return self.crates[crate_name]

    def name(self, crate, item, path):
        """Records that `item` of `crate` is named `path` by an app."""
        key = crate.canonical(item["id"])
        if key is not None:
            # An item reachable at two paths is named by the shorter, then the
            # first in order, whichever the walk meets first.
            known = self.public.get(key)
            if known is None or (path.count("::"), path) < (known.count("::"), known):
                self.public[key] = path

    def walk_module(self, crate, item, prefix):
        for child in item["inner"]["module"]["items"]:
            self.walk(crate, crate.item(child), prefix)

    def walk(self, crate, item, prefix, name=None):
        if item is None or item["visibility"] != "public":
            return
        (kind, v), = item["inner"].items()
        if kind == "use":
            self.walk_use(crate, v, prefix)
            return
        path = f"{prefix}::{name or item['name']}"
        self.name(crate, item, path)
        if kind == "module":
            self.lines.add(f"pub mod {path}")
            self.walk_module(crate, item, path)
        elif kind == "struct":
            g = v["generics"]
            self.lines.add(f"pub struct {path}{crate.generics(g)}{crate.where(g)}{non_exhaustive(item)}")
            shape = v["kind"]
            if isinstance(shape, dict) and "plain" in shape:
                self.fields(crate, shape["plain"]["fields"], path)
                if shape["plain"].get("has_stripped_fields"):
                    self.lines.add(f"{path} has private fields")
            elif isinstance(shape, dict) and "tuple" in shape:
                self.lines.add(f"{path} is a tuple struct of {len(shape['tuple'])}")
                for i, f in enumerate(shape["tuple"]):
                    field = crate.item(f)
                    if field and field["visibility"] == "public":
                        self.lines.add(f"pub {path}::{i}: {crate.ty(field['inner']['struct_field'])}")
                    else:
                        self.lines.add(f"{path} has private fields")
            elif shape == "unit":
                self.lines.add(f"{path} is a unit struct")
            self.impls(crate, v["impls"], path)
        elif kind == "union":
            self.lines.add(f"pub union {path}{crate.generics(v['generics'])}")
            self.fields(crate, v["fields"], path)
            self.impls(crate, v["impls"], path)
        elif kind == "enum":
            g = v["generics"]
            self.lines.add(f"pub enum {path}{crate.generics(g)}{crate.where(g)}{non_exhaustive(item)}")
            for variant_id in v["variants"]:
                self.variant(crate, crate.item(variant_id), path)
            self.impls(crate, v["impls"], path)
        elif kind == "function":
            self.lines.add(self.function(crate, item, path))
        elif kind == "constant":
            self.lines.add(f"pub const {path}: {crate.ty(v['type'])}")
        elif kind == "static":
            self.lines.add(f"pub static {'mut ' if v.get('is_mutable') else ''}{path}: {crate.ty(v['type'])}")
        elif kind == "type_alias":
            g = v["generics"]
            self.lines.add(f"pub type {path}{crate.generics(g)}{crate.where(g)} = {crate.ty(v['type'])}")
        elif kind == "trait":
            self.trait(crate, item, v, path)
        else:
            self.lines.add(f"pub {kind} {path}")

    def fields(self, crate, ids, path):
        for f in ids:
            field = crate.item(f)
            if field and field["visibility"] == "public":
                self.lines.add(f"pub {path}::{field['name']}: {crate.ty(field['inner']['struct_field'])}")

    def variant(self, crate, item, path):
        v = item["inner"]["variant"]
        shape = v["kind"]
        line = f"pub {path}::{item['name']}"
        if isinstance(shape, dict) and "tuple" in shape:
            types = [crate.ty(crate.item(f)["inner"]["struct_field"]) if f is not None else "_"
                     for f in shape["tuple"]]
            line += f"({', '.join(types)})"
        elif isinstance(shape, dict) and "struct" in shape:
            fields = [crate.item(f) for f in shape["struct"]["fields"]]
            body = [f"{f['name']}: {crate.ty(f['inner']['struct_field'])}" for f in fields if f]
            if shape["struct"].get("has_stripped_fields"):
                body.append("..")
            line += " { " + ", ".join(body) + " }"
        if v.get("discriminant"):
            line += f" = {v['discriminant'].get('expr', v['discriminant'].get('value'))}"
        self.lines.add(line + non_exhaustive(item))

    def trait(self, crate, item, v, path):
        g = v["generics"]
        supertraits = " + ".join(crate.bound(b) for b in v.get("bounds", []))
        quals = ("unsafe " if v.get("is_unsafe") else "") + ("auto " if v.get("is_auto") else "")
        self.lines.add(f"pub {quals}trait {path}{crate.generics(g)}"
                       f"{': ' + supertraits if supertraits else ''}{crate.where(g)}")
        # Its implementations, wherever the types are from: one for a foreign
        # or re-exported type is listed nowhere else.
        self.impls(crate, v.get("implementations", []), path)
        for member_id in v["items"]:
            member = crate.item(member_id)
            if member is None:
                continue
            (mkind, m), = member["inner"].items()
            mpath = f"{path}::{member['name']}"
            if mkind == "function":
                self.lines.add(self.function(crate, member, mpath, public=False))
            elif mkind == "assoc_type":
                bounds = " + ".join(crate.bound(b) for b in m.get("bounds", []))
                default = f" = {crate.ty(m['type'])}" if m.get("type") else ""
                self.lines.add(f"type {mpath}{crate.generics(m['generics'])}"
                               f"{': ' + bounds if bounds else ''}{default}")
            elif mkind == "assoc_const":
                default = f" = {m['value']}" if m.get("value") else ""
                self.lines.add(f"const {mpath}: {crate.ty(m['type'])}{default}")

    def walk_use(self, crate, use, prefix):
        target = crate.item(use.get("id"))
        if target is not None:
            self.walk_target(crate, target, use, prefix)
            return
        key = crate.canonical(use.get("id")) if use.get("id") is not None else None
        path = f"{prefix}::{use['name']}"
        if key is None:
            self.lines.add(f"pub use {path} = {use['source']}")
            return
        other = self.other(key[0])
        found = None
        if other is not None:
            # The same item in its own crate's documentation, found by its path.
            found = next((other.item(i) for i, e in other.paths.items()
                          if e["crate_id"] == 0 and tuple(e["path"]) == key[1] and other.item(i)), None)
        if found is None:
            self.public.setdefault(key, path)
            self.lines.add(f"pub use {path} = {'::'.join(key[1])}")
            return
        self.walk_target(other, found, use, prefix)

    def walk_target(self, crate, target, use, prefix):
        (kind, _), = target["inner"].items()
        if not use["is_glob"] and kind == "variant":
            # A variant is no item of its own visibility: `pub use Enum::Variant`.
            source = crate.paths.get(str(target["id"]))
            named = "::".join(source["path"]) if source else use["source"]
            self.lines.add(f"pub use {prefix}::{use['name']} = {named}")
        elif not use["is_glob"]:
            self.walk(crate, target, prefix, name=use["name"])
        elif kind == "module":
            self.walk_module(crate, target, prefix)
        elif kind == "enum":
            # `pub use Enum::*`: the variants by their own names.
            for variant_id in target["inner"]["enum"]["variants"]:
                variant = crate.item(variant_id)
                self.lines.add(f"pub use {prefix}::{variant['name']} = "
                               f"{crate.path_of(target['id'], target['name'])}::{variant['name']}")
        else:
            self.lines.add(f"pub use {prefix}::* = {crate.path_of(target['id'], target['name'])}::*")

    def impls(self, crate, ids, path):
        for id_ in ids:
            imp = crate.item(id_)
            if imp is None:
                continue
            v = imp["inner"]["impl"]
            if v.get("blanket_impl") is not None:
                continue
            g = v["generics"]
            header = crate.generics(g)
            target = crate.ty(v["for"])
            trait = v.get("trait")
            if trait is None:
                # An inherent impl's generics and target go on each method, as
                # an app sees them where it calls one.
                plain = target == path
                context = f" [impl{header} {target}{crate.where(g)}]" \
                    if header or g.get("where_predicates") or not plain else ""
                for m in v["items"]:
                    member = crate.item(m)
                    if not member or member["visibility"] != "public":
                        continue
                    if "function" in member["inner"]:
                        self.lines.add(self.function(crate, member, f"{path}::{member['name']}") + context)
                    elif "assoc_const" in member["inner"]:
                        c = member["inner"]["assoc_const"]
                        self.lines.add(f"pub const {path}::{member['name']}: {crate.ty(c['type'])}{context}")
                continue
            name = trait["path"].split("::")[-1]
            if (v.get("is_synthetic") and name not in KEPT_AUTO) or name in LEFT_OUT:
                continue
            negative = "!" if v.get("is_negative") else ""
            unsafe = "unsafe " if v.get("is_unsafe") else ""
            line = f"{unsafe}impl{header} {negative}{crate.trait_path(trait)} for {target}{crate.where(g)}"
            self.lines.add(line)
            # What the implementation chooses for the trait: `type Target`,
            # `type Item`, an associated constant.
            for m in v["items"]:
                member = crate.item(m)
                if member is None:
                    continue
                if "assoc_type" in member["inner"]:
                    t = member["inner"]["assoc_type"].get("type")
                    self.lines.add(f"{line}: type {member['name']} = {crate.ty(t)}")
                elif "assoc_const" in member["inner"]:
                    c = member["inner"]["assoc_const"]
                    self.lines.add(f"{line}: const {member['name']}: {crate.ty(c['type'])}"
                                   + (f" = {c['value']}" if c.get("value") else ""))

    def function(self, crate, item, path, public=True):
        f = item["inner"]["function"]
        sig = f["sig"]
        inputs = []
        for name, t in sig["inputs"]:
            if name == "self":
                rendered = crate.ty(t)
                inputs.append({"Self": "self", "&Self": "&self", "&mut Self": "&mut self"}
                              .get(rendered, f"self: {rendered}"))
            else:
                inputs.append(f"{name}: {crate.ty(t)}")
        if sig.get("is_c_variadic"):
            inputs.append("...")
        quals = qualifiers(f["header"])
        g = f["generics"]
        body = "" if public else ("" if f.get("has_body") is False else " { default }")
        visibility = "pub " if public else ""
        return (f"{visibility}{quals}fn {path}{crate.generics(g)}({', '.join(inputs)})"
                f"{crate.ret(sig['output'])}{crate.where(g)}{body}")


def listing(package=CRATE, workspace=ROOT):
    """The public items of `package`, sorted, as the listing has them."""
    public = {}
    core = Crate(rustdoc(package, workspace), public)
    crates = {core.name: core}
    root = core.item(core.doc["root"])
    # Twice: the first walk learns each item's public path, which the second
    # names it by wherever it appears.
    for _ in range(2):
        lister = Lister(crates, public, workspace)
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
