param(
    [string]$ExtensionVsix,
    [string]$HostPublish,
    [string]$LauncherPublish,
    [string]$Restore,
    # Russet's mark for the editor (russet-integration\docs\design\brand) and Electron's rcedit.
    [string]$Brand,
    [string]$Rcedit,
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'Programs\Russet Development')
)
# Reversibly replaces the bundled extension and supervisor of the existing
# development installation. Backups stay outside resources\app\extensions so the
# editor never sees two copies of russet.russet. Refuses while the app runs.
$ErrorActionPreference = 'Stop'
$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
$manifest = Join-Path $InstallRoot 'installation.json'
if (-not (Test-Path -LiteralPath $manifest)) { throw 'No Russet development installation at that path.' }
$extension = Join-Path $InstallRoot 'editor\resources\app\extensions\russet'
$hostDir = Join-Path $InstallRoot 'host'
$running = Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($InstallRoot, [StringComparison]::OrdinalIgnoreCase) }
if ($running) { throw 'Close Russet Development (editor and supervisor) before updating it.' }

function Install-Bits([string]$extensionSource, [string]$hostSource) {
    Remove-Item -LiteralPath $extension -Recurse -Force
    Copy-Item -LiteralPath $extensionSource -Destination $extension -Recurse
    Remove-Item -LiteralPath $hostDir -Recurse -Force
    Copy-Item -LiteralPath $hostSource -Destination $hostDir -Recurse
}

# The launcher lives at the installation root next to editor\ and host\.
function Install-Launcher([string]$launcherSource) {
    Get-ChildItem -LiteralPath $launcherSource -Filter 'Russet.Launcher.*' -File |
        Copy-Item -Destination $InstallRoot -Force
}

if ($Restore) {
    $backup = [IO.Path]::GetFullPath($Restore)
    foreach ($part in 'extension', 'host', 'installation.json') {
        if (-not (Test-Path -LiteralPath (Join-Path $backup $part))) { throw "Backup is missing $part." }
    }
    Install-Bits (Join-Path $backup 'extension') (Join-Path $backup 'host')
    if (Test-Path -LiteralPath (Join-Path $backup 'launcher')) { Install-Launcher (Join-Path $backup 'launcher') }
    Copy-Item -LiteralPath (Join-Path $backup 'installation.json') -Destination $manifest -Force
    Write-Output "Restored development installation from $backup"
    return
}

if (-not (Test-Path -LiteralPath $ExtensionVsix -PathType Leaf)) { throw 'Build the extension VSIX first.' }
if (-not (Test-Path -LiteralPath (Join-Path $HostPublish 'Russet.Host.exe') -PathType Leaf)) { throw 'Publish Russet.Host first.' }
Add-Type -AssemblyName System.IO.Compression.FileSystem
$backup = Join-Path $env:LOCALAPPDATA ('Russet\Backups\dev-install-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$unpacked = Join-Path $backup 'vsix'
New-Item -ItemType Directory -Path $backup | Out-Null
Copy-Item -LiteralPath $extension -Destination (Join-Path $backup 'extension') -Recurse
Copy-Item -LiteralPath $hostDir -Destination (Join-Path $backup 'host') -Recurse
Copy-Item -LiteralPath $manifest -Destination (Join-Path $backup 'installation.json')
[IO.Compression.ZipFile]::ExtractToDirectory([IO.Path]::GetFullPath($ExtensionVsix), $unpacked)
if (-not (Test-Path -LiteralPath (Join-Path $unpacked 'extension\package.json'))) { throw 'VSIX has no extension payload.' }
Install-Bits (Join-Path $unpacked 'extension') ([IO.Path]::GetFullPath($HostPublish))
Remove-Item -LiteralPath $unpacked -Recurse -Force
if ($LauncherPublish) {
    if (-not (Test-Path -LiteralPath (Join-Path $LauncherPublish 'Russet.Launcher.exe') -PathType Leaf)) { throw 'Publish Russet.Launcher first.' }
    $launcherBackup = Join-Path $backup 'launcher'
    New-Item -ItemType Directory -Path $launcherBackup | Out-Null
    Get-ChildItem -LiteralPath $InstallRoot -Filter 'Russet.Launcher.*' -File | Copy-Item -Destination $launcherBackup
    Install-Launcher ([IO.Path]::GetFullPath($LauncherPublish))
}
if ($Brand) {
    & (Join-Path $PSScriptRoot 'brand-editor.ps1') -Editor (Join-Path $InstallRoot 'editor') -Brand $Brand -Rcedit $Rcedit
}
$record = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
$record.extensionSha256 = (Get-FileHash -LiteralPath $ExtensionVsix -Algorithm SHA256).Hash.ToLowerInvariant()
$record | Add-Member -NotePropertyName hostSha256 -NotePropertyValue (Get-FileHash -LiteralPath (Join-Path $hostDir 'Russet.Host.dll') -Algorithm SHA256).Hash.ToLowerInvariant() -Force
$record | ConvertTo-Json | Set-Content -LiteralPath $manifest -Encoding utf8
Write-Output "Updated development installation; previous bits saved in $backup"
