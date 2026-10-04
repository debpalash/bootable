<#
.SYNOPSIS
  Stage Windows files for Authenticode signing and apply the signed results.

.DESCRIPTION
  The signing provider (Azure Artifact Signing or SignPath) runs between the two
  actions in the release workflow. It is deliberately outside this script so both
  providers share the same staging, validation, and checksum handling.

  Phase 'binaries' covers the three release executables and runs BEFORE
  package-windows.ps1, so the MSI, NSIS installer, portable EXEs, and ZIP all
  embed signed code. Phase 'installers' covers the MSI and setup EXE produced by
  cargo-packager and runs AFTER package-windows.ps1; it refreshes their .sha256
  sidecars.

  -Action Stage  copies the files to sign into a private directory and records a
                 manifest. Sets the step outputs unsigned_dir and signed_dir.
  -Action Apply  requires every staged file to carry a valid, timestamped
                 Authenticode signature in -SignedDirectory, then copies it back.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Stage', 'Apply')]
    [string]$Action,
    [Parameter(Mandatory = $true)]
    [ValidateSet('binaries', 'installers')]
    [string]$Phase,
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$OutputDirectory = 'dist/windows',
    [string]$SignedDirectory
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$scratch = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$stageRoot = Join-Path $scratch "bootable-sign-$Phase"
$unsignedDir = Join-Path $stageRoot 'unsigned'
$signedDir = Join-Path $stageRoot 'signed'
$manifestPath = Join-Path $stageRoot 'manifest.json'

if ($Phase -eq 'binaries') {
    $sourceDirectory = Join-Path $root "target/$Target/release"
    $names = @('bootable.exe', 'bootable-desktop.exe', 'bootable-helper.exe')
} else {
    $sourceDirectory = Join-Path $root $OutputDirectory
    $names = @("bootable-$Version-x86_64.msi", "bootable-$Version-x86_64-setup.exe")
}

function Set-StepOutput([string]$Name, [string]$Value) {
    if ($env:GITHUB_OUTPUT) { "$Name=$Value" | Add-Content -LiteralPath $env:GITHUB_OUTPUT }
    Write-Host "$Name=$Value"
}

if ($Action -eq 'Stage') {
    if (Test-Path -LiteralPath $stageRoot) { Remove-Item -LiteralPath $stageRoot -Recurse -Force }
    New-Item -ItemType Directory -Path $unsignedDir, $signedDir -Force | Out-Null
    $manifest = foreach ($name in $names) {
        $source = Join-Path $sourceDirectory $name
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Nothing to sign: $source" }
        Copy-Item -LiteralPath $source -Destination (Join-Path $unsignedDir $name)
        [pscustomobject]@{
            name        = $name
            destination = $source
            sha256      = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLower()
        }
    }
    ConvertTo-Json -InputObject @($manifest) | Set-Content -LiteralPath $manifestPath
    Set-StepOutput 'unsigned_dir' $unsignedDir
    Set-StepOutput 'signed_dir' $signedDir
    return
}

# Action = Apply
if (-not $SignedDirectory) { throw '-SignedDirectory is required for -Action Apply.' }
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "No staging manifest for phase '$Phase'; run -Action Stage first."
}
$entries = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
foreach ($entry in $entries) {
    $signed = Join-Path $SignedDirectory $entry.name
    if (-not (Test-Path -LiteralPath $signed -PathType Leaf)) {
        throw "The signing provider did not return $($entry.name)."
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $signed
    if ($signature.Status -ne 'Valid') {
        throw "$($entry.name) is not validly signed: $($signature.Status) $($signature.StatusMessage)"
    }
    if (-not $signature.TimeStamperCertificate) {
        throw "$($entry.name) has no countersigned timestamp, so its signature would expire with the certificate."
    }
    if ((Get-FileHash -LiteralPath $signed -Algorithm SHA256).Hash.ToLower() -eq $entry.sha256) {
        throw "$($entry.name) is byte-identical to the unsigned input."
    }
    Copy-Item -LiteralPath $signed -Destination $entry.destination -Force
    Write-Host "Signed $($entry.name): $($signature.SignerCertificate.Subject)"
    if ($Phase -eq 'installers') {
        $hash = (Get-FileHash -LiteralPath $entry.destination -Algorithm SHA256).Hash.ToLower()
        "$hash  $($entry.name)" | Set-Content -NoNewline -LiteralPath "$($entry.destination).sha256"
    }
}
