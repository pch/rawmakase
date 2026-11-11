#!/usr/bin/env bash
set -euo pipefail
dmg="${1:?usage: verify.sh FILE.dmg ARCH}"
arch="${2:?expected architecture}"
temporary=$(mktemp -d)
mount="$temporary/mount"
mkdir "$mount"
trap 'hdiutil detach "$mount" >/dev/null 2>&1 || true; rm -rf "$temporary"' EXIT
xcrun stapler validate "$dmg"
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount"
app="$mount/RAWmakase.app"
test -s "$app/Contents/Resources/Assets.car"
test -s "$app/Contents/Resources/rawmakase.icns"
test -s "$app/Contents/Frameworks/libonnxruntime.dylib"
# The selection runtime is the one library the app bundles: RAW decoding and
# colour management are RAWmakase's own Rust code.
for library in "$app/Contents/Frameworks/"*; do
  if [ "$(basename "$library")" != libonnxruntime.dylib ]; then
    echo "Unexpected bundled library: $library" >&2
    exit 1
  fi
done
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconName' "$app/Contents/Info.plist")" = RAWmakase
codesign --verify --strict --deep "$app"
spctl --assess --type execute --verbose=2 "$app"
# The bundled extensions' programs, from the first release that ships them:
# each signed on its own (release signing signs them before the app) and
# checked with the app's executables below.
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
binaries=("$app/Contents/MacOS/rawmakase")
programs=()
for ships in rawmakase-mcp:0.4.13 rawmakase-midi:0.4.13; do
  program="$app/Contents/MacOS/${ships%%:*}" since=${ships#*:}
  if [ "$(printf '%s\n' "$since" "$version" | sort -V | head -n1)" = "$since" ]; then
    test -x "$program"
    codesign --verify --strict "$program"
    binaries+=("$program")
    programs+=("$program")
  fi
done
for binary in "${binaries[@]}" "$app/Contents/Frameworks/"*.dylib; do
  lipo "$binary" -verify_arch "$arch"
  # Only system paths and bundle-relative paths may survive packaging.
  if otool -L "$binary" | tail -n +2 | grep -vE '^[[:space:]]+(/usr/lib/|/System/Library/|@loader_path/|@executable_path/|@rpath/)'; then
    echo "External dependency in $binary" >&2
    exit 1
  fi
done
env -u DYLD_LIBRARY_PATH -u DYLD_FALLBACK_LIBRARY_PATH "$app/Contents/MacOS/rawmakase" --version
# `rawmakase mcp` finds its program beside it, where the app looks for
# MIDI's too, and completes an MCP initialize.
if [ ${#programs[@]} -gt 0 ]; then
  python3 "$(dirname -- "$0")/../check_programs.py" "$app/Contents/MacOS/rawmakase" "${programs[@]}" --arch "$arch" --version "$version"
fi
