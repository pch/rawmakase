#!/usr/bin/env bash
# Checks that the extracted crates keep out what they are extracted to avoid:
# the app crate, the GPU and GUI stacks, and the native imaging libraries. Cargo
# already forbids a cycle back to the app; nothing else stops a leaf crate
# from gaining wgpu, egui or a LibRaw binding as a dependency.
#
#     scripts/crate-closures.sh
#
# The catalog's bundled SQLite (libsqlite3-sys) is intended and allowed. The
# renderer has wgpu for its GPU preview port. LibRaw and Little CMS are linked
# by rawmakase-native's own build script, not a crate, so the crates above it
# are kept from depending on rawmakase-native instead. Export reads Inter and
# the installed-font directories from fastframe-fonts, which brings egui (but
# no window or GPU backend) along.
set -euo pipefail

gui='egui.*|eframe|epaint|winit|fastframe-fonts'
native='rawmakase-native|rawmakase-raw|lcms2.*|libraw.*'
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
    if [ -n "$found" ]; then
        echo "$crate must not depend on:" >&2
        echo "$found" | sed 's/^/    /' >&2
        status=1
    fi
}
for crate in rawmakase-tiff rawmakase-model rawmakase-interop rawmakase-catalog rawmakase-protocol; do
    check "$crate" "$leaf"
done
check rawmakase-engine "rawmakase|rawmakase-catalog|rawmakase-export|$gui|$native|$inference"
check rawmakase-native "rawmakase|rawmakase-catalog|rawmakase-export|$gui|$inference"
check rawmakase-export "rawmakase|eframe|winit|egui-wgpu|egui-winit|egui_extras|lcms2.*|libraw.*|$inference"
# Inference itself is a leaf: it takes pixels and returns coverage, and knows no
# workspace crate, window, GPU or imaging library.
check rawmakase-inference "rawmakase|rawmakase-model|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-native|rawmakase-protocol|wgpu.*|naga|$gui|$native"
# The remote worker and its client sit on top of inference alone: the protocol
# and the worker know no catalog, renderer, GUI or native library, and only the
# app composes the client.
check rawmakase-remote "rawmakase|rawmakase-model|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-native|rawmakase-protocol|wgpu.*|naga|$gui|$native"
# RAWmakase's own RAW decoders sit on the TIFF reader alone: no workspace crate
# above them, no GPU, GUI or inference, and nothing built from C (no build
# script crates).
check rawmakase-raw "rawmakase|rawmakase-model|rawmakase-interop|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-native|rawmakase-protocol|rawmakase-remote|wgpu.*|naga|cc|pkg-config|$gui|lcms2.*|libraw.*|$inference"
check rawmakase-tiff "rawmakase|rawmakase-model|rawmakase-raw|rawmakase-native|cc|pkg-config|$gui|$native|$inference"
# The app and export reach the decoders only through rawmakase-native's raw
# adapter: no direct dependency and no rawmakase_raw path anywhere else.
for manifest in Cargo.toml crates/*/Cargo.toml tools/*/Cargo.toml; do
    case "$manifest" in crates/rawmakase-native/*|crates/rawmakase-raw/*) continue ;; esac
    if grep -q '^rawmakase-raw' "$manifest"; then
        echo "$manifest must not depend on rawmakase-raw (only rawmakase-native's raw adapter uses it)" >&2
        status=1
    fi
done
paths=$(grep -rln 'rawmakase_raw::' --include='*.rs' src crates tools examples tests \
    | grep -v -e '^crates/rawmakase-native/src/raw/' -e '^crates/rawmakase-raw/' || true)
if [ -n "$paths" ]; then
    echo "rawmakase_raw is used outside rawmakase-native's raw adapter:" >&2
    echo "$paths" | sed 's/^/    /' >&2
    status=1
fi
[ "$status" -eq 0 ] && echo "The extracted crates depend on no app, GPU, GUI or native imaging crate beyond their own."
exit "$status"
