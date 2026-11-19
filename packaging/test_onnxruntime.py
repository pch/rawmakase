import contextlib
import hashlib
import io
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import urllib.error
import zipfile

import onnxruntime

RECIPES = [Path(__file__).resolve().parent / "arch" / name / "PKGBUILD"
           for name in ("rawmakase", "rawmakase-git")]

def archive(files):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as tf:
        for name, content in files.items():
            info = tarfile.TarInfo("./" + name)
            info.size = len(content)
            tf.addfile(info, io.BytesIO(content))
    return buffer.getvalue()


def nupkg(files):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as zf:
        for name, content in files.items():
            zf.writestr(name, content)
    return buffer.getvalue()


def nuget_files(library=b"library"):
    member, _ = onnxruntime.NUGET_LIBRARIES[("macos", "arm64")]
    return {member: library, "LICENSE": b"MIT", "ThirdPartyNotices.txt": b"notices",
            "runtimes/osx-x64/native/libonnxruntime.dylib": b"other"}


class FetchTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def fetch(self, data, digest):
        asset = ("macos", "arm64")
        name, _, member, target = onnxruntime.ASSETS[asset]
        prefix = name.rsplit(".", 1)[0]
        assets = {asset: (name, digest, member, target)}
        response = io.BytesIO(data)
        with patch.dict(onnxruntime.ASSETS, assets), patch("onnxruntime.urllib.request.urlopen", return_value=response):
            return onnxruntime.fetch("macos", "arm64", self.root / "out", self.root / "notices"), prefix

    def test_the_library_and_notices_are_taken_from_a_checked_archive(self):
        prefix = "onnxruntime-osx-arm64-" + onnxruntime.VERSION
        files = {f"{prefix}/lib/libonnxruntime.{onnxruntime.VERSION}.dylib": b"library",
                 f"{prefix}/LICENSE": b"MIT", f"{prefix}/ThirdPartyNotices.txt": b"notices",
                 f"{prefix}/include/other.h": b"header"}
        data = archive(files)
        library, _ = self.fetch(data, hashlib.sha256(data).hexdigest())
        self.assertEqual(library.name, "libonnxruntime.dylib")
        self.assertEqual(library.read_bytes(), b"library")
        self.assertEqual((self.root / "notices/onnxruntime-LICENSE").read_bytes(), b"MIT")
        self.assertEqual(sorted(p.name for p in (self.root / "out").iterdir()), ["libonnxruntime.dylib"])

    def test_a_different_archive_is_refused_before_anything_is_written(self):
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            self.fetch(b"not the release", "0" * 64)
        self.assertFalse((self.root / "out").exists())

    def fetch_from_nuget(self, package, package_digest, library_digest):
        """Fetches macOS arm64 while the release archive cannot be downloaded."""
        asset = ("macos", "arm64")
        member, _ = onnxruntime.NUGET_LIBRARIES[asset]
        requested = []

        def urlopen(url, timeout):
            requested.append(url)
            if url == onnxruntime.NUGET:
                return io.BytesIO(package)
            raise urllib.error.HTTPError(url, 404, "Not Found", None, io.BytesIO())

        with patch.object(onnxruntime, "NUGET_SHA256", package_digest), \
                patch.dict(onnxruntime.NUGET_LIBRARIES, {asset: (member, library_digest)}), \
                patch("onnxruntime.urllib.request.urlopen", side_effect=urlopen), \
                contextlib.redirect_stderr(io.StringIO()):
            library = onnxruntime.fetch("macos", "arm64", self.root / "out", self.root / "notices")
        return library, requested

    def test_a_missing_release_falls_back_to_the_same_library_from_nuget(self):
        package = nupkg(nuget_files())
        library, requested = self.fetch_from_nuget(package, hashlib.sha256(package).hexdigest(),
                                                   hashlib.sha256(b"library").hexdigest())
        self.assertEqual(requested[-1], onnxruntime.NUGET)
        self.assertEqual(library.name, "libonnxruntime.dylib")
        self.assertEqual(library.read_bytes(), b"library")
        self.assertEqual((self.root / "notices/onnxruntime-LICENSE").read_bytes(), b"MIT")
        self.assertEqual((self.root / "notices/onnxruntime-ThirdPartyNotices.txt").read_bytes(),
                         b"notices")
        self.assertEqual(sorted(p.name for p in (self.root / "out").iterdir()), ["libonnxruntime.dylib"])

    def test_a_nuget_package_with_another_library_is_refused(self):
        package = nupkg(nuget_files(library=b"tampered"))
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            self.fetch_from_nuget(package, hashlib.sha256(package).hexdigest(),
                                  hashlib.sha256(b"library").hexdigest())
        self.assertFalse((self.root / "out").exists())

    def test_a_different_nuget_package_is_refused(self):
        package = nupkg(nuget_files())
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            self.fetch_from_nuget(package, "0" * 64, hashlib.sha256(b"library").hexdigest())
        self.assertFalse((self.root / "out").exists())

    def test_a_release_that_does_not_match_its_pin_does_not_fall_back(self):
        with patch("onnxruntime.urllib.request.urlopen", return_value=io.BytesIO(b"tampered")) as urlopen:
            with self.assertRaisesRegex(SystemExit, "SHA-256"):
                onnxruntime.fetch("macos", "arm64", self.root / "out", self.root / "notices")
        self.assertEqual(urlopen.call_count, 1)

    def test_a_cached_package_is_used_and_a_bad_download_is_not_cached(self):
        cache = self.root / "cache"
        with patch.dict("os.environ", {"RAWMAKASE_ORT_CACHE": str(cache)}):
            with self.assertRaisesRegex(SystemExit, "SHA-256"):
                self.fetch(b"not the release", "0" * 64)
            self.assertFalse(cache.exists())
            package = nupkg(nuget_files())
            self.fetch_from_nuget(package, hashlib.sha256(package).hexdigest(),
                                  hashlib.sha256(b"library").hexdigest())
            self.assertEqual((cache / onnxruntime.NUGET.rsplit("/", 1)[1]).read_bytes(), package)
            _, requested = self.fetch_from_nuget(package, hashlib.sha256(package).hexdigest(),
                                                 hashlib.sha256(b"library").hexdigest())
        self.assertNotIn(onnxruntime.NUGET, requested)

    def test_every_target_has_a_pinned_library_in_the_nuget_package(self):
        self.assertEqual(onnxruntime.NUGET_LIBRARIES.keys(), onnxruntime.ASSETS.keys())
        self.assertIn(onnxruntime.VERSION, onnxruntime.NUGET)
        self.assertEqual(len(onnxruntime.NUGET_SHA256), 64)
        for (platform, arch), (member, digest) in onnxruntime.NUGET_LIBRARIES.items():
            _, _, _, target = onnxruntime.ASSETS[(platform, arch)]
            self.assertTrue(member.startswith("runtimes/") and member.endswith("/native/" + target), member)
            self.assertEqual(len(digest), 64, member)

    def test_every_supported_target_pins_one_release(self):
        for (platform, arch), (name, digest, member, target) in onnxruntime.ASSETS.items():
            self.assertIn(onnxruntime.VERSION, name)
            self.assertEqual(len(digest), 64, name)
            self.assertTrue(member.startswith("lib/"), name)


@unittest.skipUnless(shutil.which("bash"), "requires bash")
class ArchRecipeTests(unittest.TestCase):
    """makepkg downloads the runtime for the Arch recipes, so they pin it themselves."""

    def recipe(self, recipe, arch, script):
        return subprocess.run(["bash", "-c", f'source "$1"; CARCH={arch}; {script}', "-", recipe],
                              check=True, capture_output=True, text=True).stdout

    def test_arch_recipes_pin_the_same_nuget_package(self):
        package = onnxruntime.NUGET.rsplit("/", 1)[1]
        for recipe in RECIPES:
            for arch in ("x86_64", "aarch64"):
                with self.subTest(recipe=recipe.parent.name, arch=arch):
                    pinned = self.recipe(recipe, arch, f'printf "%s\\n" "${{source_{arch}[@]}}" '
                                                       f'"${{sha256sums_{arch}[@]}}" "${{noextract[@]}}"')
                    self.assertEqual(pinned.split(), [f"{package}::{onnxruntime.NUGET}",
                                                      onnxruntime.NUGET_SHA256, package])

    @unittest.skipUnless(shutil.which("bsdtar"), "requires bsdtar")
    def test_arch_recipes_install_the_runtime_where_the_app_looks(self):
        package = onnxruntime.NUGET.rsplit("/", 1)[1]
        for recipe in RECIPES:
            for arch in ("x86_64", "aarch64"):
                with self.subTest(recipe=recipe.parent.name, arch=arch), \
                        tempfile.TemporaryDirectory() as root:
                    src, pkg = Path(root, "src"), Path(root, "pkg")
                    # makepkg leaves the package unextracted (noextract), beside the
                    # app's checkout, in srcdir.
                    src.mkdir()
                    files = {member: f"library {member}".encode()
                             for member, _ in onnxruntime.NUGET_LIBRARIES.values()}
                    files.update({"LICENSE": b"MIT", "ThirdPartyNotices.txt": b"notices"})
                    (src / package).write_bytes(nupkg(files))
                    (src / "rawmakase").mkdir()
                    (src / "rawmakase-0").mkdir()
                    self.recipe(recipe, arch, f'pkgver=0; srcdir={src}; pkgdir={pkg}; '
                                              'make() { :; }; cd "$srcdir"; package')
                    member, _ = onnxruntime.NUGET_LIBRARIES[("linux", arch)]
                    licenses = pkg / "usr/share/licenses" / recipe.parent.name
                    self.assertEqual((pkg / "usr/lib/rawmakase/libonnxruntime.so").read_bytes(),
                                     f"library {member}".encode())
                    self.assertEqual((licenses / "onnxruntime-LICENSE").read_text(), "MIT")
                    self.assertEqual((licenses / "onnxruntime-ThirdPartyNotices.txt").read_text(), "notices")

if __name__ == "__main__":
    unittest.main()
