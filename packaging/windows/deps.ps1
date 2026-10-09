# Builds LibRaw and Little CMS for the MSVC build with vcpkg, pinned to a
# commit with the same versions packaging/native-deps.sh builds elsewhere
# (lcms2 2.19.1; LibRaw comes from the overlay port in vcpkg-ports, which
# builds the same LibRaw commit as the other platforms). Static libraries against the static C
# runtime the app links (.cargo/config.toml), release builds only; build.rs finds them through vcpkg's
# pkg-config files. Builds natively for this Windows' architecture: x64 or ARM64.
#
#   packaging/windows/deps.ps1 C:\path\to\deps
#
# Sets PKG_CONFIG and PKG_CONFIG_PATH for the build in this process, prints
# them, and appends them to $GITHUB_ENV when run in GitHub Actions. On ARM64 it
# also puts LLVM's clang on PATH (and $GITHUB_PATH): ring assembles with it there. Third-party notices are copied to
# <deps>\notices.
param([Parameter(Mandatory)][string]$Root)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$commit = 'b8b8df2201ad8509b81a830fe0957bcb98e06c27'
if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') {
    # vcpkg has no release-only ARM64 host triplet.
    $triplet = 'arm64-windows-static-release'
    $hostTriplet = 'arm64-windows'
} else {
    $triplet = 'x64-windows-static-release'
    $hostTriplet = 'x64-windows-release'
}
$vcpkg = Join-Path $Root 'vcpkg'

if (-not (Test-Path (Join-Path $vcpkg '.git'))) {
    New-Item -ItemType Directory -Force $vcpkg | Out-Null
    git -C $vcpkg init --quiet
    git -C $vcpkg remote add origin https://github.com/microsoft/vcpkg
}
git -C $vcpkg fetch --quiet --depth 1 origin $commit
git -C $vcpkg -c advice.detachedHead=false checkout --quiet --force FETCH_HEAD
if (-not (Test-Path (Join-Path $vcpkg 'vcpkg.exe'))) {
    & (Join-Path $vcpkg 'bootstrap-vcpkg.bat') -disableMetrics
}

# LibRaw decodes lossy DNGs with libjpeg, as the other platforms' builds do.
& (Join-Path $vcpkg 'vcpkg.exe') install --disable-metrics --host-triplet=$hostTriplet `
    "--overlay-ports=$(Join-Path $PSScriptRoot 'vcpkg-ports')" `
    "libraw[core,dng-lossy]:$triplet" "pkgconf:$hostTriplet"

$installed = Join-Path $vcpkg 'installed'
$pkgconf = Join-Path $installed "$hostTriplet\tools\pkgconf\pkgconf.exe"
$pcPath = Join-Path $installed "$triplet\lib\pkgconfig"
if (-not (Test-Path $pkgconf)) { throw "vcpkg did not install $pkgconf" }
foreach ($pc in 'libraw_r', 'lcms2') {
    if (-not (Test-Path (Join-Path $pcPath "$pc.pc"))) { throw "vcpkg did not install $pc.pc" }
}

$notices = Join-Path $Root 'notices'
New-Item -ItemType Directory -Force $notices | Out-Null
foreach ($port in 'libraw', 'lcms', 'jasper', 'libjpeg-turbo', 'zlib') {
    Copy-Item (Join-Path $installed "$triplet\share\$port\copyright") (Join-Path $notices "$port-LICENSE.txt")
}

$env:PKG_CONFIG = $pkgconf
$env:PKG_CONFIG_PATH = $pcPath
"PKG_CONFIG=$pkgconf"
"PKG_CONFIG_PATH=$pcPath"
if ($env:GITHUB_ENV) {
    "PKG_CONFIG=$pkgconf" | Out-File -Append -Encoding utf8 $env:GITHUB_ENV
    "PKG_CONFIG_PATH=$pcPath" | Out-File -Append -Encoding utf8 $env:GITHUB_ENV
}
if ($triplet -like 'arm64-*' -and -not (Get-Command clang -ErrorAction SilentlyContinue)) {
    $llvm = Join-Path $env:ProgramFiles 'LLVM\bin'
    if (-not (Test-Path (Join-Path $llvm 'clang.exe'))) { throw 'LLVM (clang) is not installed' }
    $env:PATH = "$llvm;$env:PATH"
    if ($env:GITHUB_PATH) { $llvm | Out-File -Append -Encoding utf8 $env:GITHUB_PATH }
}
