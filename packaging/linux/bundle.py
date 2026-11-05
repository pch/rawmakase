#!/usr/bin/env python3
"""Stage the portable payload: the executable, its notices and any private
library it needs, without replacing system libraries."""
import argparse
import json
import platform
from pathlib import Path
import re
import shutil
import subprocess
import sys


def run(*args):
    return subprocess.check_output(args, text=True)


# Resolve symlinks first: the AppImage's AppRun, or a link from another bin
# directory, points at this script from outside the payload.
LAUNCHER = """#!/bin/sh
set -eu
self=$(readlink -f -- "$0")
base=$(CDPATH= cd -- "$(dirname -- "$self")/../lib/rawmakase" && pwd)
exec "$base/rawmakase" "$@"
"""


def write_launcher(path):
    """Write usr/bin/rawmakase, which runs the private executable beside its libraries."""
    path.parent.mkdir(parents=True)
    path.write_text(LAUNCHER)
    path.chmod(0o755)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    stage = args.output.resolve()
    stage.mkdir(parents=True, exist_ok=False)
    private = stage / "usr/lib/rawmakase"
    private.mkdir(parents=True)
    executable = private / "rawmakase"
    shutil.copy2(args.binary, executable)
    executable.chmod(0o755)
    licenses = stage / "usr/share/licenses/rawmakase"
    licenses.mkdir(parents=True)
    for source, target in [(root / "LICENSE", licenses / "LICENSE"),
                           (root / "licenses/Adobe-DNG-SDK.txt", licenses / "Adobe-DNG-SDK.txt"),
                           (root / "licenses/Compact-ICC-Profiles-CC0.txt", licenses / "Compact-ICC-Profiles-CC0.txt"),
                           (root / "licenses/Hack-MIT-BitstreamVera.txt", licenses / "Hack-MIT-BitstreamVera.txt"),
                           (root / "licenses/Inter-OFL.txt", licenses / "Inter-OFL.txt"),
                           (root / "licenses/Lensfun-database-CC-BY-SA-3.0.txt", licenses / "Lensfun-database-CC-BY-SA-3.0.txt"),
                           (root / "licenses/Lucide-ISC.txt", licenses / "Lucide-ISC.txt"),
                           (root / "licenses/NotoEmoji-OFL.txt", licenses / "NotoEmoji-OFL.txt"),
                           (root / "licenses/Ubuntu-UFL.txt", licenses / "Ubuntu-UFL.txt"),
                           (root / "licenses/emoji-icon-font-MIT.txt", licenses / "emoji-icon-font-MIT.txt"),
                           (root / "packaging/applications/rawmakase.desktop", stage / "usr/share/applications/rawmakase.desktop"),
                           (root / "packaging/icons/rawmakase.svg", stage / "usr/share/icons/hicolor/scalable/apps/rawmakase.svg")]:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    # These are supplied by the supported distribution, including its GPU stack.
    host = re.compile(r"^(lib(c|m|dl|pthread|rt|resolv|util|gcc_s|stdc\+\+|z|"
                      r"vulkan|xkbcommon|xkbcommon-x11|wayland-client|wayland-cursor|wayland-egl|"
                      r"X11|X11-xcb|xcb|Xcursor|Xi|Xrandr|EGL|GL)\.so[.\d]*|ld-linux.*)$")
    listing = run("ldd", str(args.binary))
    if "not found" in listing:
        raise RuntimeError(listing)
    records = []
    for line in listing.splitlines():
        match = re.match(r"\s*(\S+) => (/\S+)", line)
        if not match:
            continue
        name, source = match.groups()
        if host.fullmatch(name):
            continue
        destination = private / name
        shutil.copy2(source, destination)
        destination.chmod(0o755)
        # Preserve Ubuntu's copyright/source reference for a bundled library.
        # ldd may name /lib paths on merged-/usr systems while dpkg records
        # their canonical /usr/lib paths (or the reverse).
        owner = None
        for candidate in dict.fromkeys([str(Path(source).resolve()), source]):
            query = subprocess.run(["dpkg-query", "-S", candidate], text=True, capture_output=True)
            if query.returncode == 0:
                owner = query.stdout.split(": ", 1)[0].split(":", 1)[0]
                break
        if not owner:
            raise RuntimeError(f"Cannot find distribution notices for {source}")
        copyright_file = Path("/usr/share/doc") / owner / "copyright"
        shutil.copy2(copyright_file, licenses / (owner + "-copyright"))
        records.append({"library": name, "package": run("dpkg-query", "-W", "-f=${Package} ${Version}", owner)})
    (licenses / "ubuntu-libraries.json").write_text(json.dumps(records, indent=2) + "\n")
    # The ONNX Runtime that runs the subject selection model, opened lazily from
    # beside the executable; the app starts without it.
    sys.path.insert(0, str(root / "packaging"))
    import onnxruntime
    onnxruntime.fetch("linux", platform.machine(), private, licenses)
    for path in private.iterdir():
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", str(path)], check=True)
    launcher = stage / "usr/bin/rawmakase"
    write_launcher(launcher)
    # Verify without the build-time library path.
    import os
    env = dict(os.environ)
    env.pop("LD_LIBRARY_PATH", None)
    subprocess.run([str(launcher), "--version"], env=env, check=True)
    print(stage)


if __name__ == "__main__":
    main()
