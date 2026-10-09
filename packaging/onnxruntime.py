#!/usr/bin/env python3
"""Fetch the pinned ONNX Runtime library for one platform, checked against its SHA-256.

The app loads the library lazily, from beside the executable, to run the selection
model (docs/subject-and-background-masks.md); nothing links it. Every platform
ships the same release because 1.23.2 is the last one with an official macOS x86_64
build. Only the library and the notices are taken from the archive.

  packaging/onnxruntime.py macos arm64 OUTPUT_DIR NOTICES_DIR
"""
import argparse
import hashlib
import io
import os
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

VERSION = "1.23.2"
BASE = f"https://github.com/microsoft/onnxruntime/releases/download/v{VERSION}/"
# (platform, architecture) -> archive, SHA-256, library inside it, name it is given.
ASSETS = {
    ("macos", "arm64"): (
        f"onnxruntime-osx-arm64-{VERSION}.tgz",
        "b4d513ab2b26f088c66891dbbc1408166708773d7cc4163de7bdca0e9bbb7856",
        f"lib/libonnxruntime.{VERSION}.dylib", "libonnxruntime.dylib"),
    ("macos", "x86_64"): (
        f"onnxruntime-osx-x86_64-{VERSION}.tgz",
        "d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6",
        f"lib/libonnxruntime.{VERSION}.dylib", "libonnxruntime.dylib"),
    ("linux", "x86_64"): (
        f"onnxruntime-linux-x64-{VERSION}.tgz",
        "1fa4dcaef22f6f7d5cd81b28c2800414350c10116f5fdd46a2160082551c5f9b",
        f"lib/libonnxruntime.so.{VERSION}", "libonnxruntime.so"),
    ("linux", "aarch64"): (
        f"onnxruntime-linux-aarch64-{VERSION}.tgz",
        "7c63c73560ed76b1fac6cff8204ffe34fe180e70d6582b5332ec094810241e5c",
        f"lib/libonnxruntime.so.{VERSION}", "libonnxruntime.so"),
    ("windows", "x86_64"): (
        f"onnxruntime-win-x64-{VERSION}.zip",
        "0b38df9af21834e41e73d602d90db5cb06dbd1ca618948b8f1d66d607ac9f3cd",
        "lib/onnxruntime.dll", "onnxruntime.dll"),
    ("windows", "aarch64"): (
        f"onnxruntime-win-arm64-{VERSION}.zip",
        "1cfe88b6435df3b5fb0e9f6bd7d6f5df1e887b6174de7f6e2a47bab956f3f168",
        "lib/onnxruntime.dll", "onnxruntime.dll"),
}
NOTICES = ("LICENSE", "ThirdPartyNotices.txt")


def fetch(platform, arch, output, notices):
    """Put the library in `output` and ONNX Runtime's notices in `notices`."""
    try:
        archive, digest, member, name = ASSETS[(platform, arch)]
    except KeyError:
        raise SystemExit(f"No ONNX Runtime {VERSION} for {platform} {arch}")
    cache = os.environ.get("RAWMAKASE_ORT_CACHE")
    cached = Path(cache) / archive if cache else None
    if cached and cached.is_file():
        data = cached.read_bytes()
    else:
        with urllib.request.urlopen(BASE + archive, timeout=300) as response:
            data = response.read()
        if cached:
            cached.parent.mkdir(parents=True, exist_ok=True)
            cached.write_bytes(data)
    found = hashlib.sha256(data).hexdigest()
    if found != digest:
        raise SystemExit(f"{archive}: SHA-256 is {found}, expected {digest}")
    prefix = archive.rsplit(".", 1)[0]
    wanted = [f"{prefix}/{member}"] + [f"{prefix}/{notice}" for notice in NOTICES]
    if archive.endswith(".zip"):
        with zipfile.ZipFile(io.BytesIO(data)) as zf:
            files = {name: zf.read(name) for name in wanted}
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tf:
            files = {m.name.removeprefix("./"): tf.extractfile(m).read()
                     for m in tf.getmembers()
                     if m.isfile() and m.name.removeprefix("./") in wanted}
    read = lambda path: files[f"{prefix}/{path}"]
    output.mkdir(parents=True, exist_ok=True)
    notices.mkdir(parents=True, exist_ok=True)
    target = output / name
    target.write_bytes(read(member))
    target.chmod(0o755)
    for notice in NOTICES:
        (notices / f"onnxruntime-{notice}").write_bytes(read(notice))
    return target


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("platform", choices=sorted({p for p, _ in ASSETS}))
    parser.add_argument("arch")
    parser.add_argument("output", type=Path)
    parser.add_argument("notices", type=Path)
    args = parser.parse_args()
    print(fetch(args.platform, args.arch, args.output, args.notices))


if __name__ == "__main__":
    sys.exit(main())
