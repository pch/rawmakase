# Stages the Windows payload: the executable, the bundled extensions'
# programs and the licenses. The C runtime is linked in, so no Visual C++
# Redistributable is needed.
#
#   pwsh packaging/windows/stage.ps1 target\release\rawmakase.exe dist\windows -Programs target\release\rawmakase-mcp.exe, target\release\rawmakase-midi.exe
#
# Fails the build if the executable or a program imports any DLL that is not
# part of Windows. Stages natively for this Windows' architecture: x64 or ARM64.
param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$Output,
    # Bundled extensions' programs (rawmakase-mcp.exe, rawmakase-midi.exe), installed beside the
    # executable, where the app looks for them.
    [string[]]$Programs = @()
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$arm64 = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64'

if (Test-Path $Output) { throw "$Output already exists" }
New-Item -ItemType Directory $Output | Out-Null
Copy-Item $Executable (Join-Path $Output 'rawmakase.exe')
foreach ($program in $Programs) { Copy-Item $program $Output }
Copy-Item (Join-Path $root 'LICENSE'), (Join-Path $root 'README.md') $Output
$licenses = New-Item -ItemType Directory (Join-Path $Output 'licenses')
Copy-Item (Join-Path $root 'licenses\*') $licenses

# The ONNX Runtime that runs the subject selection model, opened lazily from beside
# the executable; rawmakase.exe does not import it, so the updater's helper copy
# still starts alone.
python (Join-Path $root 'packaging\onnxruntime.py') windows $(if ($arm64) { 'aarch64' } else { 'x86_64' }) $Output $licenses
if (-not (Test-Path (Join-Path $Output 'onnxruntime.dll'))) { throw 'ONNX Runtime was not staged' }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$component, $bin = if ($arm64) { 'ARM64', 'Hostarm64\arm64' } else { 'x86.x64', 'Hostx64\x64' }
$vs = & $vswhere -latest -products * -requires "Microsoft.VisualStudio.Component.VC.Tools.$component" -property installationPath
if (-not $vs) { throw 'Visual Studio with the C++ tools is not installed' }
$tools = Get-ChildItem (Join-Path $vs 'VC\Tools\MSVC') -Directory | Where-Object Name -Match '^\d+(\.\d+)+$' |
    Sort-Object { [version]$_.Name } | Select-Object -Last 1
$dumpbin = Join-Path $tools.FullName "bin\$bin\dumpbin.exe"

# The updater copies rawmakase.exe alone into a staging folder and runs it as
# its helper, so it must not need any DLL that ships beside it. The programs
# need none either.
$system = Join-Path $env:SystemRoot 'System32'
$binaries = @('rawmakase.exe') + ($Programs | ForEach-Object { Split-Path $_ -Leaf })
foreach ($name in $binaries) {
    $exe = Join-Path $Output $name
    $inside = $false
    foreach ($line in & $dumpbin /nologo /dependents $exe) {
        if ($line -match 'Image has the following( delay load)? dependencies') { $inside = $true; continue }
        if ($line -match '^\s+Summary') { break }
        if ($inside -and $line -match '^\s+(\S+\.dll)\s*$') {
            $dll = $Matches[1].ToLowerInvariant()
            $windows = $dll -like 'api-ms-win-*' -or $dll -like 'ext-ms-*' -or (Test-Path (Join-Path $system $dll))
            # Present on the runner through Visual Studio, but not on a clean Windows.
            $redistributable = $dll -match '^(vcruntime|msvcp|vcomp|concrt|ucrtbased)'
            if (-not $windows -or $redistributable) { throw "$name needs $dll, which is not part of Windows" }
            "$name imports $dll"
        }
    }
}
# The programs have this architecture; `rawmakase mcp` finds the MCP program
# beside the app, where the app looks for MIDI's too, and completes an MCP
# initialize.
if ($Programs.Count -gt 0) {
    $staged = $Programs | ForEach-Object { Join-Path $Output (Split-Path $_ -Leaf) }
    python (Join-Path $root 'packaging\check_programs.py') (Join-Path $Output 'rawmakase.exe') @staged `
        --arch $(if ($arm64) { 'aarch64' } else { 'x86_64' })
    if ($LASTEXITCODE -ne 0) { throw 'A staged program failed its check' }
}
Get-ChildItem $Output -Recurse -File | ForEach-Object { $_.FullName.Substring((Resolve-Path $Output).Path.Length + 1) }
