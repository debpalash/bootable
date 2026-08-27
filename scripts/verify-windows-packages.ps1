[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$OutputDirectory = 'dist/windows'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$output = Join-Path $root $OutputDirectory
$expected = @(
    "bootable-$Version-x86_64.msi",
    "bootable-$Version-x86_64-setup.exe",
    "bootable-desktop-$Version-x86_64.exe",
    "bootable-tui-$Version-x86_64.exe",
    "bootable-$Version-x86_64-pc-windows-msvc.zip"
)

foreach ($name in $expected) {
    $path = Join-Path $output $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing package: $name" }
    $sidecar = "$path.sha256"
    $expectedHash = (Get-Content -LiteralPath $sidecar -Raw).Split(' ')[0]
    $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLower()
    if ($expectedHash -ne $actualHash) { throw "Checksum mismatch: $name" }
}

$reportedVersion = & (Join-Path $output "bootable-tui-$Version-x86_64.exe") --version
if ($LASTEXITCODE -ne 0 -or $reportedVersion.Trim() -ne "bootable $Version") {
    throw "Portable TUI reported an unexpected version: $reportedVersion"
}
& (Join-Path $output "bootable-tui-$Version-x86_64.exe") --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Portable TUI failed its help smoke test.' }

$msiExtract = Join-Path $env:RUNNER_TEMP 'bootable-msi-verify'
$process = Start-Process -FilePath 'msiexec.exe' -ArgumentList @(
    '/a',
    "`"$(Join-Path $output "bootable-$Version-x86_64.msi")`"",
    '/qn',
    "TARGETDIR=`"$msiExtract`""
) -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "MSI administrative extraction failed: $($process.ExitCode)" }
foreach ($name in @('bootable.exe', 'bootable-desktop.exe', 'bootable-helper.exe')) {
    if (-not (Get-ChildItem -LiteralPath $msiExtract -Recurse -Filter $name -File)) {
        throw "MSI is missing $name."
    }
}

$listing = & 7z l (Join-Path $output "bootable-$Version-x86_64-setup.exe")
if ($LASTEXITCODE -ne 0) { throw 'Could not inspect the NSIS installer.' }
foreach ($name in @('bootable.exe', 'bootable-desktop.exe', 'bootable-helper.exe')) {
    if (-not ($listing -match [regex]::Escape($name))) { throw "NSIS installer is missing $name." }
}

$zipExtract = Join-Path $env:RUNNER_TEMP 'bootable-zip-verify'
Expand-Archive -LiteralPath (Join-Path $output "bootable-$Version-x86_64-pc-windows-msvc.zip") `
    -DestinationPath $zipExtract -Force
foreach ($name in @('bootable.exe', 'bootable-desktop.exe', 'bootable-helper.exe', 'install.ps1')) {
    if (-not (Test-Path -LiteralPath (Join-Path $zipExtract $name) -PathType Leaf)) {
        throw "ZIP is missing $name."
    }
}
