# native-packages' Inno Setup recipe for the windows-amd64 and windows-arm64
# targets, each built natively on its own architecture:
#
#   pwsh packaging/windows/setup.ps1 PAYLOAD PACKAGE VERSION
#
# Compiles packaging/windows/rawmakase.iss over the staged payload into exactly
# PACKAGE. Inno Setup 6.3 or newer.
param(
    [Parameter(Mandatory)][string]$Payload,
    [Parameter(Mandatory)][string]$Package,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
if (Test-Path $Package) { throw "$Package already exists" }
if (-not $Package.EndsWith('.exe')) { throw "$Package is not an .exe" }

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe' }
if (-not (Test-Path $iscc)) { throw 'Inno Setup 6 is not installed (choco install innosetup)' }

$output = [System.IO.Path]::GetFullPath($Package)
$architectures = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x64compatible' }
& $iscc /Q "/DVersion=$Version" "/DNumericVersion=$($Version.Split('-')[0])" "/DArchitectures=$architectures" `
    "/DPayload=$([System.IO.Path]::GetFullPath($Payload))" `
    "/DOutputDir=$([System.IO.Path]::GetDirectoryName($output))" `
    "/DOutputName=$([System.IO.Path]::GetFileNameWithoutExtension($output))" `
    (Join-Path $PSScriptRoot 'rawmakase.iss')
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if (-not (Test-Path $output)) { throw "Inno Setup did not create $output" }
