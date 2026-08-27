[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$OutputDirectory = 'dist/windows'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$binaryDirectory = Join-Path $root "target/$Target/release"
$output = Join-Path $root $OutputDirectory
$config = Join-Path $root 'packaging/Packager.toml'

foreach ($name in @('bootable.exe', 'bootable-desktop.exe', 'bootable-helper.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $binaryDirectory $name) -PathType Leaf)) {
        throw "Missing executable: $name"
    }
}
$configuredVersion = Select-String -LiteralPath $config -Pattern '^version = "([^\"]+)"$'
if (-not $configuredVersion -or $configuredVersion.Matches[0].Groups[1].Value -ne $Version) {
    throw "Packager.toml does not declare release version $Version."
}

New-Item -ItemType Directory -Path $output -Force | Out-Null
cargo packager --config $config --formats wix,nsis
if ($LASTEXITCODE -ne 0) { throw 'cargo-packager failed.' }

$msi = Get-ChildItem -LiteralPath $output -Filter '*.msi' | Select-Object -First 1
$installer = Get-ChildItem -LiteralPath $output -Filter '*setup.exe' | Select-Object -First 1
if (-not $msi) { throw 'cargo-packager did not produce an MSI.' }
if (-not $installer) { throw 'cargo-packager did not produce an NSIS installer.' }
Move-Item -LiteralPath $msi.FullName -Destination (Join-Path $output "bootable-$Version-x86_64.msi") -Force
Move-Item -LiteralPath $installer.FullName -Destination (Join-Path $output "bootable-$Version-x86_64-setup.exe") -Force
Copy-Item -LiteralPath (Join-Path $binaryDirectory 'bootable-desktop.exe') `
    -Destination (Join-Path $output "bootable-desktop-$Version-x86_64.exe")
Copy-Item -LiteralPath (Join-Path $binaryDirectory 'bootable.exe') `
    -Destination (Join-Path $output "bootable-tui-$Version-x86_64.exe")

$stage = Join-Path $env:RUNNER_TEMP 'bootable-windows-archive'
New-Item -ItemType Directory -Path $stage -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $binaryDirectory 'bootable.exe') -Destination $stage
Copy-Item -LiteralPath (Join-Path $binaryDirectory 'bootable-desktop.exe') -Destination $stage
Copy-Item -LiteralPath (Join-Path $binaryDirectory 'bootable-helper.exe') -Destination $stage
Copy-Item -LiteralPath (Join-Path $root 'scripts/install.ps1') -Destination $stage
Copy-Item -LiteralPath (Join-Path $root 'assets/bootable-mark.svg') -Destination (Join-Path $stage 'bootable.svg')
Copy-Item -LiteralPath (Join-Path $root 'README.md'),(Join-Path $root 'LICENSE') -Destination $stage
Compress-Archive -Path (Join-Path $stage '*') `
    -DestinationPath (Join-Path $output "bootable-$Version-$Target.zip") -Force

Get-ChildItem -LiteralPath $output -File | Where-Object Extension -ne '.sha256' | ForEach-Object {
    $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower()
    "$hash  $($_.Name)" | Set-Content -NoNewline "$($_.FullName).sha256"
}
