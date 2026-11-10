#!/usr/bin/env bash
# Run inside a clean Ubuntu, Debian or Fedora container, with /packages and
# the packaging folder (as /checks) mounted.
set -euo pipefail
if command -v apt-get >/dev/null; then
  apt-get update -qq
  apt-get install -y --no-install-recommends /packages/*.deb python3
  remove=(apt-get remove -y rawmakase)
else
  dnf install -y --setopt=install_weak_deps=False /packages/*.rpm python3
  remove=(dnf remove -y rawmakase)
fi
rawmakase --version
test -f /usr/share/applications/rawmakase.desktop
test -f /usr/share/icons/hicolor/scalable/apps/rawmakase.svg
test -f /usr/share/licenses/rawmakase/LICENSE
if ldd /usr/lib/rawmakase/rawmakase | grep -q 'not found'; then exit 1; fi
# The runtime the subject selection model needs is opened lazily, so only loading it
# shows it works on a clean system. Pull requests install the published 0.1.8
# package, which predates it; releases from 0.2.2 bundle it.
installed=$(rawmakase --version | awk '{print $2}')
if [ "$(printf '%s\n' 0.2.2 "$installed" | sort -V | head -n1)" = 0.2.2 ]; then
  test -f /usr/lib/rawmakase/libonnxruntime.so
  python3 -c "import ctypes; ctypes.CDLL('/usr/lib/rawmakase/libonnxruntime.so')"
fi
# The bundled extensions' programs, from the first release that ships them:
# beside the app, built for this machine and linking only the system's
# libraries; MCP's found by `rawmakase mcp`, which completes an MCP initialize
# with it, in the folder where the app looks for MIDI's too.
programs=()
for ships in rawmakase-mcp:0.4.13 rawmakase-midi:0.4.13; do
  program=${ships%%:*} since=${ships#*:}
  if [ "$(printf '%s\n' "$since" "$installed" | sort -V | head -n1)" = "$since" ]; then
    programs+=("/usr/lib/rawmakase/$program")
  fi
done
if [ ${#programs[@]} -gt 0 ]; then
  python3 /checks/check_programs.py /usr/bin/rawmakase "${programs[@]}" \
    --arch "$(uname -m)" --version "$installed"
fi
# Winit/wgpu load some libraries at runtime, invisible to ordinary ldd checks.
python3 - <<'PY'
import ctypes
for library in ['libvulkan.so.1', 'libxkbcommon.so.0', 'libxkbcommon-x11.so.0',
                'libwayland-client.so.0', 'libwayland-cursor.so.0', 'libwayland-egl.so.1',
                'libX11.so.6', 'libX11-xcb.so.1', 'libxcb.so.1', 'libXcursor.so.1',
                'libXi.so.6', 'libXrandr.so.2', 'libEGL.so.1', 'libGL.so.1']:
    ctypes.CDLL(library)
PY
mkdir -p /root/.local/share/rawmakase
echo preserve > /root/.local/share/rawmakase/packaging-test
"${remove[@]}"
test "$(cat /root/.local/share/rawmakase/packaging-test)" = preserve
test ! -e /usr/bin/rawmakase
test ! -e /usr/lib/rawmakase/rawmakase
test ! -e /usr/lib/rawmakase/rawmakase-mcp
test ! -e /usr/lib/rawmakase/rawmakase-midi
