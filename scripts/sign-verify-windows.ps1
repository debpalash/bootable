<#
.SYNOPSIS
  Assert that every Windows executable and installer carries a valid Authenticode
  signature, including the executables inside the ZIP, MSI, and NSIS installer.
  Run only when the release workflow signed the Windows packages.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$OutputDirectory = 'dist/windows'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$output = Join-Path $root $OutputDirectory
$scratch = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }

$signtool = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" `
    -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $signtool) { Write-Warning 'signtool.exe not found; relying on Get-AuthenticodeSignature only.' }

function Assert-Signed([string]$Path, [string]$Label) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid') {
        throw "$Label is not validly signed: $($signature.Status) $($signature.StatusMessage)"
    }
    if (-not $signature.TimeStamperCertificate) { throw "$Label has no countersigned timestamp." }
    if ($signtool) {
        & $signtool.FullName verify /pa /all $Path | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "signtool verify /pa failed for $Label." }
    }
    Write-Host "OK $Label -> $($signature.SignerCertificate.Subject)"
}

function Assert-ExecutablesSigned([string]$Directory, [string]$Container) {
    foreach ($name in @('bootable.exe', 'bootable-desktop.exe', 'bootable-helper.exe')) {
        $found = Get-ChildItem -LiteralPath $Directory -Recurse -Filter $name -File
        if (-not $found) { throw "$Container is missing $name." }
        foreach ($file in $found) { Assert-Signed $file.FullName "$Container/$name" }
    }
}

foreach ($name in @(
        "bootable-$Version-x86_64.msi",
        "bootable-$Version-x86_64-setup.exe",
        "bootable-desktop-$Version-x86_64.exe",
        "bootable-tui-$Version-x86_64.exe")) {
    Assert-Signed (Join-Path $output $name) $name
}

$zipExtract = Join-Path $scratch 'bootable-sign-verify-zip'
Expand-Archive -LiteralPath (Join-Path $output "bootable-$Version-x86_64-pc-windows-msvc.zip") `
    -DestinationPath $zipExtract -Force
Assert-ExecutablesSigned $zipExtract 'zip'

$msiExtract = Join-Path $scratch 'bootable-sign-verify-msi'
$process = Start-Process -FilePath 'msiexec.exe' -ArgumentList @(
    '/a', "`"$(Join-Path $output "bootable-$Version-x86_64.msi")`"", '/qn', "TARGETDIR=`"$msiExtract`""
) -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "MSI administrative extraction failed: $($process.ExitCode)" }
Assert-ExecutablesSigned $msiExtract 'msi'

$nsisExtract = Join-Path $scratch 'bootable-sign-verify-nsis'
& 7z x "-o$nsisExtract" -y (Join-Path $output "bootable-$Version-x86_64-setup.exe") | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not extract the NSIS installer.' }
Assert-ExecutablesSigned $nsisExtract 'setup'

Write-Host 'All Windows executables and installers carry valid, timestamped Authenticode signatures.'
