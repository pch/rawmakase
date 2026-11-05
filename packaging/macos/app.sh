#!/bin/sh
# Local app bundle, ad-hoc signed; releases are bundled by packaging/macos/bundle.py.
set -eu
cd "$(dirname "$0")/../.."
# Prefer a native rustup compiler over an older Homebrew Rust or Rosetta toolchain.
rustup_bin="${HOME}/.cargo/bin/rustup"
if [ -x "$rustup_bin" ]; then
    architecture=$(uname -m)
    case "$architecture" in arm64) architecture=aarch64 ;; esac
    toolchain="1.98.1-$architecture-apple-darwin"
    if compiler=$("$rustup_bin" which --toolchain "$toolchain" rustc 2>/dev/null); then
        PATH="$(dirname "$compiler"):$PATH"
        export PATH
    else
        echo "Install the native compiler: $rustup_bin toolchain install $toolchain --profile minimal" >&2
        exit 1
    fi
fi
profile=${1:-release}
case "$profile" in
    release) cargo build --release --locked ;;
    debug) cargo build --locked ;;
    *) echo 'Usage: packaging/macos/app.sh [release|debug]' >&2; exit 2 ;;
esac
bundle="target/$profile/RAWmakase.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources/licenses"
# Replace, never overwrite: macOS kills a signed binary rewritten in place.
rm -f "$bundle/Contents/MacOS/rawmakase"
cp "target/$profile/rawmakase" "$bundle/Contents/MacOS/rawmakase"
cp packaging/macos/Info.plist "$bundle/Contents/Info.plist"
cp LICENSE "$bundle/Contents/Resources/licenses/RAWmakase.txt"
cp licenses/Adobe-DNG-SDK.txt "$bundle/Contents/Resources/licenses/Adobe-DNG-SDK.txt"
cp licenses/Compact-ICC-Profiles-CC0.txt "$bundle/Contents/Resources/licenses/Compact-ICC-Profiles-CC0.txt"
cp licenses/Hack-MIT-BitstreamVera.txt "$bundle/Contents/Resources/licenses/Hack-MIT-BitstreamVera.txt"
cp licenses/Inter-OFL.txt "$bundle/Contents/Resources/licenses/Inter-OFL.txt"
cp licenses/Lensfun-database-CC-BY-SA-3.0.txt "$bundle/Contents/Resources/licenses/Lensfun-database-CC-BY-SA-3.0.txt"
cp licenses/Lucide-ISC.txt "$bundle/Contents/Resources/licenses/Lucide-ISC.txt"
cp licenses/NotoEmoji-OFL.txt "$bundle/Contents/Resources/licenses/NotoEmoji-OFL.txt"
cp licenses/Ubuntu-UFL.txt "$bundle/Contents/Resources/licenses/Ubuntu-UFL.txt"
cp licenses/emoji-icon-font-MIT.txt "$bundle/Contents/Resources/licenses/emoji-icon-font-MIT.txt"
# The app icon, rendered as packaging/macos/bundle.py does.
if command -v rsvg-convert >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
    iconset="target/$profile/rawmakase.iconset"
    rm -rf "$iconset"
    mkdir "$iconset"
    for size in 16 32 128 256 512; do
        rsvg-convert -w "$size" -h "$size" -o "$iconset/icon_${size}x${size}.png" packaging/icons/rawmakase.svg
        rsvg-convert -w $((size * 2)) -h $((size * 2)) -o "$iconset/icon_${size}x${size}@2x.png" packaging/icons/rawmakase.svg
    done
    iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/rawmakase.icns"
    rm -rf "$iconset"
    plist="$bundle/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c 'Set :CFBundleIconFile rawmakase.icns' "$plist" 2>/dev/null ||
        /usr/libexec/PlistBuddy -c 'Add :CFBundleIconFile string rawmakase.icns' "$plist"
else
    echo 'brew install librsvg for the app icon' >&2
fi
# With Xcode 26, the layered Liquid Glass icon for macOS 26.
compiled="target/$profile/rawmakase-icon"
rm -rf "$compiled"
mkdir "$compiled"
if xcrun actool packaging/macos/RAWmakase.icon --compile "$compiled" --app-icon RAWmakase \
    --platform macosx --minimum-deployment-target 11.0 \
    --output-partial-info-plist "$compiled/partial.plist" >/dev/null 2>&1 &&
    [ -f "$compiled/Assets.car" ]; then
    cp "$compiled/Assets.car" "$bundle/Contents/Resources/Assets.car"
    plist="$bundle/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c 'Set :CFBundleIconName RAWmakase' "$plist" 2>/dev/null ||
        /usr/libexec/PlistBuddy -c 'Add :CFBundleIconName string RAWmakase' "$plist"
fi
rm -rf "$compiled"
echo "$bundle"
