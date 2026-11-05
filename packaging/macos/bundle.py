#!/usr/bin/env python3
"""Stage a relocatable app; only the supplied output directory is modified."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import sys
import subprocess
import tempfile


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def dependencies(path):
    return [line.strip().split(" (", 1)[0] for line in run("otool", "-L", str(path)).splitlines()[1:]]


def system(path):
    return path.startswith(("/usr/lib/", "/System/Library/"))


def install_icon_assets(root, resources, minimum_macos, precompiled=None):
    """Install the layered icon, requiring supplied release assets to be usable."""
    if precompiled is not None:
        if not precompiled.is_file() or precompiled.stat().st_size == 0:
            raise SystemExit(f"Missing or empty compiled icon: {precompiled}")
        shutil.copy2(precompiled, resources / "Assets.car")
        return True
    # Local builds can compile with Xcode 26 or retain the flat icon. Release
    # builds supply the catalog compiled on macOS 26, avoiding actool on macOS 15.
    with tempfile.TemporaryDirectory(prefix="rawmakase-icon-") as temporary:
        compiled = Path(temporary)
        result = subprocess.run(["xcrun", "actool", str(root / "packaging/macos/RAWmakase.icon"), "--compile", str(compiled),
                                 "--app-icon", "RAWmakase", "--platform", "macosx",
                                 "--minimum-deployment-target", minimum_macos,
                                 "--output-partial-info-plist", str(compiled / "partial.plist")],
                                capture_output=True)
        catalog = compiled / "Assets.car"
        if result.returncode == 0 and catalog.is_file() and catalog.stat().st_size > 0:
            shutil.copy2(catalog, resources / "Assets.car")
            return True
        if os.environ.get("GITHUB_ACTIONS") == "true":
            raise SystemExit("actool cannot compile packaging/macos/RAWmakase.icon:\n"
                             + result.stderr.decode(errors="replace"))
        print("actool cannot compile packaging/macos/RAWmakase.icon; keeping the flat icon")
        return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("version")
    parser.add_argument("--minimum-macos", default="15.0")
    parser.add_argument("--icon-assets", type=Path,
                        help="Assets.car compiled on macOS 26 for the layered app icon")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    app = args.output.resolve()
    app.mkdir(parents=True, exist_ok=False)
    macos = app / "Contents/MacOS"
    frameworks = app / "Contents/Frameworks"
    resources = app / "Contents/Resources"
    for directory in (macos, frameworks, resources):
        directory.mkdir(parents=True)
    executable = macos / "rawmakase"
    shutil.copy2(args.binary, executable)
    notices = resources / "licenses"
    notices.mkdir()
    shutil.copy2(root / "LICENSE", notices / "RAWmakase.txt")
    shutil.copy2(root / "licenses/Adobe-DNG-SDK.txt", notices)
    shutil.copy2(root / "licenses/Compact-ICC-Profiles-CC0.txt", notices)
    shutil.copy2(root / "licenses/Hack-MIT-BitstreamVera.txt", notices)
    shutil.copy2(root / "licenses/Inter-OFL.txt", notices)
    shutil.copy2(root / "licenses/Lensfun-database-CC-BY-SA-3.0.txt", notices)
    shutil.copy2(root / "licenses/Lucide-ISC.txt", notices)
    shutil.copy2(root / "licenses/NotoEmoji-OFL.txt", notices)
    shutil.copy2(root / "licenses/Ubuntu-UFL.txt", notices)
    shutil.copy2(root / "licenses/emoji-icon-font-MIT.txt", notices)

    # Resolve the original dependency graph before rewriting any load commands.
    copies = {args.binary.resolve(): executable}
    queue = list(copies)
    edges = []
    formulae = set()
    while queue:
        original = queue.pop()
        for dependency in dependencies(original):
            if system(dependency):
                continue
            if not dependency.startswith("/"):
                raise RuntimeError(f"Unresolved dependency {dependency} in {original}")
            source = Path(dependency).resolve(strict=True)
            # A dylib's first LC_ID_DYLIB entry is its own identity.
            if source == original:
                continue
            destination = frameworks / Path(dependency).name
            if source not in copies:
                if destination in copies.values():
                    raise RuntimeError(f"Conflicting library names: {source}")
                copies[source] = destination
                shutil.copy2(source, destination)
                destination.chmod(0o755)
                queue.append(source)
                match = re.search(r"/Cellar/([^/]+)/", str(source))
                if match:
                    formulae.add(match.group(1))
            edges.append((copies[original], dependency, copies[source].name))
    for destination in copies.values():
        # Rewrite first, then replace signatures. Removing Homebrew's signature
        # first can leave a LINKEDIT layout Apple's install_name_tool rejects.
        if destination != executable:
            subprocess.run(["install_name_tool", "-id", "@rpath/" + destination.name, str(destination)], check=True)
    for destination, old, name in edges:
        relative = "@executable_path/../Frameworks/" if destination == executable else "@loader_path/"
        subprocess.run(["install_name_tool", "-change", old, relative + name, str(destination)], check=True)

    # The ONNX Runtime that runs the subject selection model, opened lazily from
    # here; it is signed with the rest, which hardened-runtime library validation
    # requires of anything the app loads.
    sys.path.insert(0, str(root / "packaging"))
    import onnxruntime
    runtime = onnxruntime.fetch("macos", run("lipo", "-archs", str(args.binary)).split()[0],
                                frameworks, notices)
    copies[runtime.resolve()] = runtime
    subprocess.run(["install_name_tool", "-id", "@rpath/" + runtime.name, str(runtime)], check=True)

    # Preserve the exact Homebrew source/version metadata and upstream notices.
    for formula in sorted(formulae):
        info = json.loads(run("brew", "info", "--json=v2", formula))["formulae"][0]
        folder = notices / formula
        folder.mkdir()
        (folder / "homebrew-source.json").write_text(json.dumps(info, indent=2) + "\n")
        prefix = Path(run("brew", "--prefix", formula))
        for item in prefix.iterdir():
            if item.is_file() and item.name.upper().startswith(("LICENSE", "COPYING", "COPYRIGHT", "README")):
                shutil.copy2(item, folder)

    with (root / "packaging/macos/Info.plist").open("rb") as handle:
        metadata = plistlib.load(handle)
    numeric = args.version.split("-", 1)[0]
    metadata.update(CFBundleShortVersionString=numeric, CFBundleVersion=numeric,
                    LSMinimumSystemVersion=args.minimum_macos, CFBundleIconFile="rawmakase.icns")
    with (app / "Contents/Info.plist").open("wb") as handle:
        plistlib.dump(metadata, handle)
    # rsvg-convert is installed only on the build host, never shipped in the app.
    iconset = app.parent / "rawmakase.iconset"
    iconset.mkdir(exist_ok=False)
    try:
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                name = f"icon_{size}x{size}" + ("@2x" if scale == 2 else "") + ".png"
                subprocess.run(["rsvg-convert", "-w", str(size * scale), "-h", str(size * scale),
                                "-o", str(iconset / name), str(root / "packaging/icons/rawmakase.svg")], check=True)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(resources / "rawmakase.icns")], check=True)
    finally:
        shutil.rmtree(iconset)
    # macOS 26 draws the layered icon; older systems use the flat icns above.
    if install_icon_assets(root, resources, args.minimum_macos, args.icon_assets):
        metadata["CFBundleIconName"] = "RAWmakase"
        with (app / "Contents/Info.plist").open("wb") as handle:
            plistlib.dump(metadata, handle)
    # Ad-hoc signatures allow local verification; release signing replaces them.
    # Signing the main executable can validate its enclosing bundle, so sign
    # every nested library first (Intel libraries may arrive unsigned).
    signing_order = [path for path in copies.values() if path != executable]
    signing_order.append(executable)
    for destination in signing_order:
        subprocess.run(["codesign", "--force", "--sign", "-", str(destination)], check=True)
    subprocess.run(["codesign", "--force", "--sign", "-", str(app)], check=True)
    for destination in copies.values():
        for dependency in dependencies(destination):
            if not system(dependency) and not dependency.startswith(("@loader_path/", "@executable_path/", "@rpath/")):
                raise RuntimeError(f"External dependency remains: {dependency}")
    print(app)


if __name__ == "__main__":
    main()
