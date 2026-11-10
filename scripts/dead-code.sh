#!/usr/bin/env bash
# Lists library items the rawmakase binary never reaches.
#
# The library's public items are exported, so the compiler never calls them
# unused. This builds a scratch copy in which the binary declares the library's
# modules itself; there nothing is exported, and rustc's dead_code lint names
# every item no path from main reaches. Items used only by unit tests,
# integration tests or examples are reported too: decide each one (delete it,
# keep it for the tests, or move the test) rather than deleting the list.
#
#     scripts/dead-code.sh            # report; exits 0 unless the scratch build fails
#
# Run it from the repository root. It builds into target/dead-code.
set -euo pipefail

root=$(pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

rsync -a --exclude target Cargo.toml Cargo.lock build.rs src data assets crates tools extensions "$scratch"/

# The library's module declarations and crate re-exports, private, followed by
# the binary with its `rawmakase::` paths rewritten to `crate::`. Imports of a module itself
# (`develop::{self, ..}`, a bare `raw`) would now clash with the declarations.
python3 - "$scratch/src/main.rs" <<'PY'
import re, sys
library = open("src/lib.rs").read()
visibility = r"(?:pub(?:\(crate\))? )?"
modules = re.findall(rf"^{visibility}mod (\w+);", library, re.M)
declarations = re.findall(rf"^(?:#\[cfg\(.*\)\]\n)?{visibility}mod \w+;", library, re.M)
# `pub use rawmakase_model::{color, model, ..};`: the extracted crates' modules.
reexports = re.findall(rf"^{visibility}use \w+::(?:\{{[^}}]*\}}|\w+);", library, re.M)
for group in reexports:
    modules += re.findall(r"\b(\w+)\b", group.split("::", 1)[1])
# `pub use platform::volume::mounted as volume_mounted;`: an item of the
# library's own under another name, which the binary reaches as `crate::`.
aliases = re.findall(rf"^{visibility}use [\w:]+ as \w+;", library, re.M)
main = open("src/main.rs").read()
inner = re.findall(r"^#!\[.*\]\n", main, re.M)
body = re.sub(r"^#!\[.*\]\n", "", main, flags=re.M)
body = re.sub(r"\brawmakase::", "crate::", body)
def without_modules(group):
    for name in modules:
        group = re.sub(rf"\b{name}::\{{self, ", f"{name}::{{", group)
        group = re.sub(rf"(?<=[{{,\s]){name},\s*", "", group)
        group = re.sub(rf",\s*{name}(?=\s*\}})", "", group)
    return group
body = re.sub(r"use crate::\{.*?\};", lambda m: without_modules(m.group(0)), body, flags=re.S)
private = lambda d: re.sub(r"^pub(?:\(crate\))? ", "", d, flags=re.M)
# Not every re-exported module is used by the binary; that is no dead code.
declared = "\n".join(
    [private(d) for d in declarations]
    + ["#[allow(unused_imports)]\n" + private(r) for r in reexports + aliases]
)
open(sys.argv[1], "w").write("".join(inner) + declared + "\n" + body)
PY
rm "$scratch/src/lib.rs"

# A scratch build that fails says nothing about dead code: report its errors
# and fail, rather than an empty list.
status=0
output=$(CARGO_TARGET_DIR="$root/target/dead-code" cargo check --manifest-path "$scratch/Cargo.toml" \
    --bin rawmakase --message-format short 2>&1) || status=$?
if [ "$status" -ne 0 ]; then
    echo "$output" | grep -E "(^|: )error" | sed -e "s|$scratch/||" | sort -u >&2
    echo "dead-code.sh: the scratch build failed; its report would be meaningless" >&2
    exit "$status"
fi
echo "$output" |
    grep -E "warning: (unused|.* never (used|read|constructed))" |
    sed -e "s|$scratch/||" |
    sort -u || true
