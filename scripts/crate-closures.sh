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
# RAWmakase's own decoders (rawmakase-raw) and color is managed in Rust
# (moxcms): no crate may bring a C RAW library (LibRaw) or Little CMS back,
# whether as a crate or linked by a build script. Export resolves system fonts and
# lists installed-font directories itself, so no GUI crate reaches it;
# only the app uses fastframe-fonts, for its interface.
set -euo pipefail

gui='egui.*|eframe|epaint|winit|fastframe-fonts'
# C RAW decoders and Little CMS, under any binding's name.
c_imaging='lcms2.*|.*lcms.*|libraw.*|.*-libraw.*|rsraw.*'
native="rawmakase-decode|rawmakase-raw|$c_imaging"
# The ONNX Runtime binding opens a library at run time. Only the app composes it: no
# value, file-format, catalog or rendering crate may reach it or the selection models
# built on it, so saved masks render without any inference installed.
inference='rawmakase-inference|rawmakase-selection|ort.*'
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
# The extension protocol, the SDK, the host and cargo-rawmakase build without the
# app, a GPU, a GUI, native imaging libraries or inference: extension programs
# and their authors' tools stay as light as the control clients.
for crate in rawmakase-extension-protocol rawmakase-extension rawmakase-extensions rawmakase-example rawmakase-midi cargo-rawmakase; do
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
# Decoding writes no encoded image either: the JPEG XL encoders, which export
# writes its JPEG XL files with, stay export's.
encoders='zune-jpegxl|jixel'
check rawmakase-decode "rawmakase|rawmakase-catalog|rawmakase-export|cc|nasm.*|cmake|pkg-config|$encoders|$gui|$c_imaging|$inference" "$rav1d_build"
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
check rawmakase-export "rawmakase|$gui|$c_imaging|$inference"
# The library is what an app outside the workspace links: never the app, its
# GUI (egui's parts emath and ecolor named too), its file chooser or the
# extension host, and no C imaging library. Inference comes only with the
# AI Denoise feature, which it doesn't have yet.
check rawmakase-core "rawmakase|rawmakase-extensions|rfd|emath|ecolor|$gui|$c_imaging|$inference"
# Merge to HDR reads RAWs through decoding and writes DNGs: no app, catalog,
# export, window, GUI, C imaging library or inference.
check rawmakase-hdr-merge "rawmakase|rawmakase-catalog|rawmakase-export|rawmakase-denoise|$gui|$c_imaging|$inference"
# Inference itself is a leaf: it runs the ONNX Runtime and AI Denoise's model, and
# knows no workspace crate but the cancellation token (not the selection models or
# the remote worker built on it), and no window, GPU or imaging library.
check rawmakase-inference "rawmakase|rawmakase-model|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-decode|rawmakase-protocol|rawmakase-selection|rawmakase-remote|wgpu.*|naga|$gui|$native"
# AI Denoise's service sits on inference, decoding and the model installer: no
# app, catalog, export, selection model, window, GUI or C imaging library. It
# reaches wgpu only through the renderer, whose decoded-image types it passes on.
check rawmakase-denoise "rawmakase|rawmakase-catalog|rawmakase-export|rawmakase-selection|rawmakase-remote|$gui|$c_imaging"
# The selection models take pixels and return coverage: no workspace crate but
# inference and the cancellation token, and no edit, catalog, renderer, decoder,
# extension host, window, GPU or imaging library.
check rawmakase-selection "rawmakase|rawmakase-model|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-decode|rawmakase-protocol|rawmakase-remote|rawmakase-extension.*|wgpu.*|naga|$gui|$native"
# The remote worker and its client sit on top of the selection models and inference (and the cancellation token) alone: the protocol
# and the worker know no catalog, renderer, GUI or native library, and only the
# AI masks extension composes the client.
check rawmakase-remote "rawmakase|rawmakase-model|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-decode|rawmakase-protocol|wgpu.*|naga|$gui|$native"
# The AI masks extension sits on the selection models, the remote client and
# the installer: no catalog, renderer, decoder, GUI, GPU or native imaging
# library.
check rawmakase-ai-masks "rawmakase|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-interop|rawmakase-decode|wgpu.*|naga|$gui|$native"
# The MCP server is a client of the control protocol alone: no workspace crate
# but the protocol and the control client, and no GPU, GUI, native library or
# inference.
check rawmakase-mcp "rawmakase|rawmakase-cancel|rawmakase-fs|rawmakase-tiff|rawmakase-model|rawmakase-interop|rawmakase-catalog|rawmakase-engine|rawmakase-export|rawmakase-decode|rawmakase-remote|wgpu.*|naga|$gui|$native|$inference"
# The app runs the bundled MCP program (`rawmakase mcp`) and never links it:
# no MCP server or its runtime in the app binary. (tokio still appears in the
# app's tree for wasm targets only, through wasm-bindgen-futures.)
check rawmakase "rawmakase-mcp|rmcp.*"
# Nor does it link the bundled MIDI program, or a MIDI library: MIDI input is
# that program's (extensions/midi), which the app starts beside it.
check rawmakase "rawmakase-midi|midir|coremidi|alsa.*"
# Nor the bundled AI masks program, the selection models it runs or the
# remote worker's client (extensions/ai-masks): the app runs AI Denoise on
# rawmakase-inference alone.
check rawmakase "rawmakase-ai-masks|rawmakase-selection|rawmakase-remote"
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
for manifest in Cargo.toml crates/*/Cargo.toml tools/*/Cargo.toml extensions/*/Cargo.toml; do
    case "$manifest" in crates/rawmakase-decode/*|crates/rawmakase-raw/*) continue ;; esac
    if grep -q '^rawmakase-raw' "$manifest"; then
        echo "$manifest must not depend on rawmakase-raw (only rawmakase-decode's raw adapter uses it)" >&2
        status=1
    fi
done
paths=$(grep -rln 'rawmakase_raw::' --include='*.rs' src crates tools extensions examples tests \
    | grep -v -e '^crates/rawmakase-decode/src/raw/' -e '^crates/rawmakase-raw/' || true)
if [ -n "$paths" ]; then
    echo "rawmakase_raw is used outside rawmakase-decode's raw adapter:" >&2
    echo "$paths" | sed 's/^/    /' >&2
    status=1
fi
[ "$status" -eq 0 ] && echo "The extracted crates depend on no app, GPU, GUI or native imaging crate beyond their own, and nothing links a C RAW library or Little CMS."
exit "$status"
