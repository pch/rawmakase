#!/usr/bin/env python3
"""Fetch the pinned DirectX Shader Compiler (DXC) DLL for Windows, checked against its SHA-256.

On Windows, RAWmakase loads `dxcompiler.dll` by absolute path from beside the
executable so wgpu never searches `PATH` for another application's copy (#1254);
nothing links it, and builds without the DLL fall back to Windows' built-in FXC.
Only `dxcompiler.dll` (DXC >= 1.8.2502 signs DXIL in-process without `dxil.dll`)
and the release notices are taken from Microsoft's official release archive.

  packaging/dxc.py x86_64 OUTPUT_DIR NOTICES_DIR
"""
import argparse
import hashlib
import io
import os
from pathlib import Path
import sys
import urllib.request
import zipfile

VERSION = "1.9.2609"
ARCHIVE = "dxc_2026_09_29.zip"
URL = f"https://github.com/microsoft/DirectXShaderCompiler/releases/download/v{VERSION}/{ARCHIVE}"
ARCHIVE_SHA256 = "ad31b1fc8443175d204f77a611fdb3ef2ec42759bdc2f1167368de24a4a7e7f1"

# architecture -> (normalized path inside the archive, SHA-256 of dxcompiler.dll).
LIBRARIES = {
    "x86_64": (
        "bin/x64/dxcompiler.dll",
        "0689e2fee0a1c9df8662a59680469f24e2a3ad7ce6bbbcc9284f7a83172bfc59",
    ),
    "aarch64": (
        "bin/arm64/dxcompiler.dll",
        "ec234595c270a908da183eb04f1f5a1a667d3ce81e69f5bc55370503dab48373",
    ),
}
NOTICES = ("LICENSE-LLVM.txt", "LICENCE-MIT.txt", "LICENSE-MS.txt")


def check(name, data, digest):
    found = hashlib.sha256(data).hexdigest()
    if found != digest:
        raise SystemExit(f"{name}: SHA-256 is {found}, expected {digest}")


def download(url, name, digest):
    """The bytes at `url`, kept in `RAWMAKASE_DXC_CACHE` as `name` when it is set,
    after checking they hash to `digest`."""
    cache = os.environ.get("RAWMAKASE_DXC_CACHE")
    cached = Path(cache) / name if cache else None
    if cached and cached.is_file():
        data = cached.read_bytes()
    else:
        with urllib.request.urlopen(url, timeout=300) as response:
            data = response.read()
    check(name, data, digest)
    if cached and not cached.is_file():
        cached.parent.mkdir(parents=True, exist_ok=True)
        cached.write_bytes(data)
    return data


def read_member(zf, entries, path):
    try:
        return zf.read(entries[path])
    except KeyError:
        raise SystemExit(f"{ARCHIVE} does not contain {path}")


def fetch(arch, output, notices):
    """Put `dxcompiler.dll` in `output` and DXC's notices in `notices`."""
    try:
        member, digest = LIBRARIES[arch]
    except KeyError:
        raise SystemExit(f"No DXC {VERSION} for Windows {arch}")
    data = download(URL, ARCHIVE, ARCHIVE_SHA256)
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        # Microsoft's Windows release archives store Windows backslash separators.
        entries = {name.replace("\\", "/"): name for name in zf.namelist()}
        dll = read_member(zf, entries, member)
        notice_bytes = {notice: read_member(zf, entries, notice) for notice in NOTICES}
    check(member, dll, digest)
    output.mkdir(parents=True, exist_ok=True)
    notices.mkdir(parents=True, exist_ok=True)
    target = output / "dxcompiler.dll"
    target.write_bytes(dll)
    target.chmod(0o755)
    for notice, content in notice_bytes.items():
        (notices / f"dxc-{notice}").write_bytes(content)
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("arch", choices=sorted(LIBRARIES))
    parser.add_argument("output", type=Path)
    parser.add_argument("notices", type=Path)
    args = parser.parse_args()
    print(fetch(args.arch, args.output, args.notices))


if __name__ == "__main__":
    sys.exit(main())
