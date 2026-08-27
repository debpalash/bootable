[CmdletBinding()]
param(
    [ValidateSet('Gui', 'Tui', 'All')]
    [string]$Variant = 'Gui',
    [switch]$Elevated
)

$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
$isAdministrator = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdministrator) {
    if ($Elevated) {
        throw 'Administrator authentication was denied or did not produce an elevated process.'
    }
    $arguments = @(
        '-NoLogo',
        '-NoProfile',
        '-ExecutionPolicy', 'Bypass',
        '-File', ('"{0}"' -f $PSCommandPath),
        '-Variant', $Variant,
        '-Elevated'
    )
    $process = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -Verb RunAs -Wait -PassThru
    exit $process.ExitCode
}

$source = Split-Path -Parent $PSCommandPath
$destination = Join-Path $env:ProgramFiles 'Bootable'
$required = @('bootable-helper.exe')
if ($Variant -in @('Gui', 'All')) { $required += 'bootable-desktop.exe' }
if ($Variant -in @('Tui', 'All')) { $required += 'bootable.exe' }
foreach ($name in $required) {
    $path = Join-Path $source $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Release archive is missing $name."
    }
}

Write-Host 'Bootable can erase removable drives after an explicit review and confirmation.'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
foreach ($name in $required) {
    Copy-Item -LiteralPath (Join-Path $source $name) -Destination (Join-Path $destination $name) -Force
}

# Program Files supplies the protected inherited ACL. Reset a replaced helper to that inherited
# ACL before the app will trust it.
$helper = Join-Path $destination 'bootable-helper.exe'
& (Join-Path $env:SystemRoot 'System32\icacls.exe') $helper /reset | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not secure the privileged helper ACL.' }

if ($Variant -in @('Gui', 'All')) {
    $startMenu = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs'
    $shortcutPath = Join-Path $startMenu 'Bootable.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = Join-Path $destination 'bootable-desktop.exe'
    $shortcut.WorkingDirectory = $destination
    $shortcut.Description = 'Create verified boot media from removable drives'
    $shortcut.Save()
}

Write-Host "Installed the $Variant variant in $destination."
Write-Host 'The interface stays unelevated; Windows asks for UAC only when the reviewed write begins.'
