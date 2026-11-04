#!/usr/bin/env bash
# Checks that the extracted crates keep out what they are extracted to avoid:
# the app crate, the GPU and GUI stacks, and the native imaging libraries. Cargo
# already forbids a cycle back to the app; nothing else stops a leaf crate
# from gaining wgpu, egui or a C RAW library binding as a dependency.
#
#     scripts/crate-closures.sh
#
# The catalog's bundled SQLite (libsqlite3-sys) is intended and allowed. The
# renderer has wgpu for its GPU preview port. RAW files are read by
# RAWmakase's own decoders (rawmakase-raw) and colour is managed in Rust
# (moxcms): no crate may bring a C RAW library (LibRaw) or Little CMS back,
# whether as a crate or linked by a build script. Export reads Inter and
# the installed-font directories from fastframe-fonts, which brings egui (but
# no window or GPU backend) along.
set -euo pipefail

gui='egui.*|eframe|epaint|winit|fastframe-fonts'
# C RAW decoders and Little CMS, under any binding's name.
c_imaging='lcms2.*|.*lcms.*|libraw.*|.*-libraw.*|rsraw.*'
native="rawmakase-decode|rawmakase-raw|$c_imaging"
# The ONNX Runtime binding opens a library at run time. Only the app composes it: no
# value, file-format, catalog or rendering crate may reach it, so saved masks render
# without any inference installed.
inference='rawmakase-inference|ort.*'
leaf="rawmakase|wgpu.*|naga|$gui|$native|$inference"
status=0
check() {
    local crate=$1 forbidden="^($2) "
    # Every platform's dependencies, not only this machine's.
    found=$(cargo tree --locked -p "$crate" --all-features --target all -e normal,build \
        --prefix none --format '{p}' | sort -u | grep -E "$forbidden" || true)
    if [ -n "${3:-}" ]; then
        found=$(echo "$found" | grep -Ev "^($3) " || true)
    fi
    if [ -n "$found" ]; then
        echo "$crate must not depend on:" >&2
        echo "$found" | sed 's/^/    /' >&2
        status=1
    fi
}
for crate in rawmakase-cancel rawmakase-fs rawmakase-tiff rawmakase-model rawmakase-interop rawmakase-catalog rawmakase-protocol; do
    check "$crate" "$leaf"
done
check rawmakase-engine "rawmakase|rawmakase-catalog|rawmakase-export|$gui|$native|$inference"
# The decoding boundary is Rust alone: nothing in it is built from C. rav1d (the
# AV1 decoder for AVIF) lists cc and nasm-rs as build dependencies but uses them
# only for its asm features, which stay off, so they may come in through rav1d
# and nothing else. Cargo merges features across the build, so no crate of the
# workspace may turn asm on. Drop the exception once rav1d makes them optional,
# or the AV1 decoder needs no build script.
rav1d_build='cc|nasm-rs'
check rawmakase-decode "rawmakase|rawmakase-catalog|rawmakase-export|cc|nasm.*|cmake|pkg-config|$gui|$c_imaging|$inference" "$rav1d_build"
decode_tree=$(cargo tree --locked -p rawmakase-decode --all-features --target all -e normal,build \
    --prefix none --format '{p}')
for dep in cc nasm-rs; do
    grep -q "^$dep " <<<"$decode_tree" || continue
    others=$(cargo tree --locked -p rawmakase-decode --all-features --target all -e normal,build \
        -i "$dep" --depth 1 --prefix none --format '{p}' | tail -n +2 | grep -v '^rav1d ' || true)
    if [ -n "$others" ]; then
        echo "rawmakase-decode may reach $dep only through rav1d, not:" >&2
        echo "$others" | sed 's/^/    /' >&2
        status=1
    fi
done
asm=$(cargo tree --locked --workspace --all-features --target all -e features -i rav1d \
    --prefix none | grep '^rav1d feature "asm' | sed 's/ (\*)$//' | sort -u || true)
if [ -n "$asm" ]; then
    echo "rav1d must be built without assembly:" >&2
    echo "$asm" | sed 's/^/    /' >&2
    status=1
fi
check rawmakase-export "rawmakase|eframe|winit|egui-wgpu|egui-winit|egui_extras|$c_imaging|$inference"
# Inference itself is a leaf: it takes pixels and returns coverage, and knows no
# workspace crate but the cancellation token, and no window, GPU or imaging library.
check rawmakase-inference "rawmakase|rawmakase-model|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-decode|rawmakase-protocol|wgpu.*|naga|$gui|$native"
# The remote worker and its client sit on top of inference (and the cancellation token) alone: the protocol
# and the worker know no catalog, renderer, GUI or native library, and only the
# app composes the client.
check rawmakase-remote "rawmakase|rawmakase-model|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-decode|rawmakase-protocol|wgpu.*|naga|$gui|$native"
# RAWmakase's own RAW decoders sit on the TIFF reader and the cancellation token alone: no workspace crate
# above them, no GPU, GUI or inference, and nothing built from C (no build
# script crates).
check rawmakase-raw "rawmakase|rawmakase-model|rawmakase-interop|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-decode|rawmakase-protocol|rawmakase-remote|wgpu.*|naga|cc|pkg-config|$gui|$c_imaging|$inference"
check rawmakase-tiff "rawmakase|rawmakase-model|rawmakase-raw|rawmakase-decode|cc|pkg-config|$gui|$native|$inference"
# No crate of the workspace, the app with every feature and platform included,
# reaches a C RAW library or Little CMS.
found=$(cargo tree --locked --workspace --all-features --target all -e normal,build,dev \
    --prefix none --format '{p}' | sort -u | grep -E "^($c_imaging) " || true)
if [ -n "$found" ]; then
    echo "No workspace crate may depend on a C RAW library or Little CMS:" >&2
    echo "$found" | sed 's/^/    /' >&2
    status=1
fi
# Nor does a build script link one: rawmakase-decode has no build script, and
# no other build script names LibRaw or Little CMS.
if [ -e crates/rawmakase-decode/build.rs ] \
    || grep -Eq '^[[:space:]]*(build|links)[[:space:]]*=|^\[(target\..*\.)?build-dependencies\]' \
        crates/rawmakase-decode/Cargo.toml; then
    echo "rawmakase-decode must not have a build script or build dependencies" >&2
    status=1
fi
linking=$(git ls-files '*build.rs' | xargs grep -Eil 'libraw|lcms|raw_r|rustc-link-lib=(dylib=|static=)?(raw|lcms)' || true)
if [ -n "$linking" ]; then
    echo "Build scripts must not link a C RAW library or Little CMS:" >&2
    echo "$linking" | sed 's/^/    /' >&2
    status=1
fi
# The app and export reach the decoders only through rawmakase-decode's raw
# adapter: no direct dependency and no rawmakase_raw path anywhere else.
for manifest in Cargo.toml crates/*/Cargo.toml tools/*/Cargo.toml; do
    case "$manifest" in crates/rawmakase-decode/*|crates/rawmakase-raw/*) continue ;; esac
    if grep -q '^rawmakase-raw' "$manifest"; then
        echo "$manifest must not depend on rawmakase-raw (only rawmakase-decode's raw adapter uses it)" >&2
        status=1
    fi
done
paths=$(grep -rln 'rawmakase_raw::' --include='*.rs' src crates tools examples tests \
    | grep -v -e '^crates/rawmakase-decode/src/raw/' -e '^crates/rawmakase-raw/' || true)
if [ -n "$paths" ]; then
    echo "rawmakase_raw is used outside rawmakase-decode's raw adapter:" >&2
    echo "$paths" | sed 's/^/    /' >&2
    status=1
fi
[ "$status" -eq 0 ] && echo "The extracted crates depend on no app, GPU, GUI or native imaging crate beyond their own, and nothing links a C RAW library or Little CMS."
exit "$status"
