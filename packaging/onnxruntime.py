#!/usr/bin/env python3
"""Fetch the pinned ONNX Runtime library for one platform, checked against its SHA-256.

The app loads the library lazily, from beside the executable, to run the selection
model (docs/subject-and-background-masks.md); nothing links it. Every platform
ships the same release because 1.23.2 is the last one with an official macOS x86_64
build. Only the library and the notices are taken from the archive.

The release archives come from Microsoft's GitHub releases. When those cannot be
downloaded (Microsoft took the repository down in November 2026), the same release
is taken from Microsoft's NuGet package, which holds every platform's library. Its
libraries are byte-for-byte the archives' ones, and each is pinned by its own
SHA-256 as well as the package's. Only a failed download falls back: a file that
does not match its pin is always an error.

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
import urllib.error
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

NUGET = (f"https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/{VERSION}/"
         f"microsoft.ml.onnxruntime.{VERSION}.nupkg")
NUGET_SHA256 = "25fe172f7fcdf34f2b5b02c8b997f6b88abc282956849d4af4335af23d0c4e4a"
# (platform, architecture) -> the library inside the NuGet package, and its SHA-256.
# Except for Windows arm64, whose archive was never downloaded here, each digest is
# that of the library in the archive above.
NUGET_LIBRARIES = {
    ("macos", "arm64"): (
        "runtimes/osx-arm64/native/libonnxruntime.dylib",
        "d306d2bc768540766c7ed8a1e0ff05d2870c77a934ebeee4a7bafa1b732ef299"),
    ("macos", "x86_64"): (
        "runtimes/osx-x64/native/libonnxruntime.dylib",
        "8c9c78de65ea3786f987c0d980e9c1b13a3a5fbc6b3e2965ba05b450e6e4c054"),
    ("linux", "x86_64"): (
        "runtimes/linux-x64/native/libonnxruntime.so",
        "13ab8084954fa4a47c777880180b90810d6020f021441395712b48a75b74c68b"),
    ("linux", "aarch64"): (
        "runtimes/linux-arm64/native/libonnxruntime.so",
        "648ffa64fbe027ae27139109410900cf776a030dec2dbbac51053318cc44c286"),
    ("windows", "x86_64"): (
        "runtimes/win-x64/native/onnxruntime.dll",
        "dec964ab1ee36cc9b0ae247d13b376627992fc57dec0454354017ab8fd84f1ea"),
    ("windows", "aarch64"): (
        "runtimes/win-arm64/native/onnxruntime.dll",
        "99a11bb077f723a81d100fd98099f332e4178bbb7025ac10fd40699a0b82c2f0"),
}


def download(url, name, digest):
    """The bytes at `url`, kept in `RAWMAKASE_ORT_CACHE` as `name` when it is set,
    after checking they hash to `digest`."""
    cache = os.environ.get("RAWMAKASE_ORT_CACHE")
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


def check(name, data, digest):
    found = hashlib.sha256(data).hexdigest()
    if found != digest:
        raise SystemExit(f"{name}: SHA-256 is {found}, expected {digest}")


def from_archive(platform, arch):
    """The library and the notices, by their paths in the release archive."""
    archive, digest, member, _ = ASSETS[(platform, arch)]
    data = download(BASE + archive, archive, digest)
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
    return {path: files[f"{prefix}/{path}"] for path in [member, *NOTICES]}


def from_nuget(platform, arch):
    """The same files as from_archive returns, taken from the NuGet package."""
    _, _, member, _ = ASSETS[(platform, arch)]
    library, digest = NUGET_LIBRARIES[(platform, arch)]
    data = download(NUGET, NUGET.rsplit("/", 1)[1], NUGET_SHA256)
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        files = {member: zf.read(library), **{notice: zf.read(notice) for notice in NOTICES}}
    check(library, files[member], digest)
    return files


def fetch(platform, arch, output, notices):
    """Put the library in `output` and ONNX Runtime's notices in `notices`."""
    try:
        archive, _, member, name = ASSETS[(platform, arch)]
    except KeyError:
        raise SystemExit(f"No ONNX Runtime {VERSION} for {platform} {arch}")
    try:
        files = from_archive(platform, arch)
    except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
        print(f"{archive}: {error}; using the NuGet package", file=sys.stderr)
        files = from_nuget(platform, arch)
    output.mkdir(parents=True, exist_ok=True)
    notices.mkdir(parents=True, exist_ok=True)
    target = output / name
    target.write_bytes(files[member])
    target.chmod(0o755)
    for notice in NOTICES:
        (notices / f"onnxruntime-{notice}").write_bytes(files[notice])
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
