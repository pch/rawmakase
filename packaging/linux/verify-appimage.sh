#!/usr/bin/env bash
# Check an AppImage native-packages built, on its own architecture: its name,
# layout, licences and update information, and with --run, that it starts
# and reports VERSION.
set -euo pipefail
usage='usage: verify-appimage.sh APPIMAGE VERSION [--run]'
image=$(readlink -f -- "${1:?$usage}")
version="${2:?$usage}"
run="${3:-}"
# The documented download names; never rename them.
name=$(basename -- "$image")
[[ "$name" =~ ^rawmakase-${version//./\\.}-(x86_64|aarch64)\.AppImage$ ]] || { echo "Unexpected AppImage name: $name" >&2; exit 1; }
arch=${BASH_REMATCH[1]}
chmod +x "$image"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$work" && "$image" --appimage-extract >/dev/null)
root="$work/squashfs-root"
# AppRun runs the desktop entry's Exec target, the launcher DEB and RPM install.
test "$(readlink "$root/AppRun")" = usr/bin/rawmakase
for file in rawmakase.desktop rawmakase.svg usr/bin/rawmakase usr/lib/rawmakase/rawmakase \
  usr/share/applications/rawmakase.desktop usr/share/licenses/rawmakase/LICENSE \
  usr/share/licenses/rawmakase/AppImage-runtime-LICENSE; do
  test -f "$root/$file" || { echo "AppImage lacks $file" >&2; exit 1; }
done
test "$(sha256sum < "$root/rawmakase.desktop")" = "$(sha256sum < "$root/usr/share/applications/rawmakase.desktop")"
test -x "$root/usr/lib/rawmakase/rawmakase"
# The bundled extensions' programs, from the first release that ships them.
programs=()
for ships in rawmakase-mcp:0.4.13 rawmakase-midi:0.4.13 rawmakase-ai-masks:0.4.13; do
  program=${ships%%:*} since=${ships#*:}
  if [ "$(printf '%s\n' "$since" "$version" | sort -V | head -n1)" = "$since" ]; then
    test -x "$root/usr/lib/rawmakase/$program" || { echo "AppImage lacks an executable $program" >&2; exit 1; }
    programs+=("$root/usr/lib/rawmakase/$program")
  fi
done
# Where AppImageUpdate looks for the next release; native-packages has
# already checked that the .zsync file describes this AppImage.
test "$("$image" --appimage-updateinformation)" = "gh-releases-zsync|pch|rawmakase|latest|rawmakase-*-$arch.AppImage.zsync"
test -s "$image.zsync" || { echo "Missing $name.zsync" >&2; exit 1; }
if [[ "$run" = --run ]]; then
  expected="rawmakase $version"
  # Without FUSE: once extracted by the runtime, and once as extracted here.
  test "$(cd / && APPIMAGE_EXTRACT_AND_RUN=1 "$image" --version)" = "$expected"
  test "$(cd / && "$root/AppRun" --version)" = "$expected"
  # The programs have this architecture and link only the system's
  # libraries; `rawmakase mcp` from the AppImage finds MCP's beside the app,
  # where the app looks for MIDI's and AI masks' too, and completes an MCP
  # initialize.
  if [[ ${#programs[@]} -gt 0 ]]; then
    APPIMAGE_EXTRACT_AND_RUN=1 python3 "$(dirname -- "$0")/../check_programs.py" "$image" \
      "${programs[@]}" --arch "$arch" --version "$version"
  fi
fi
echo "Verified $name"
