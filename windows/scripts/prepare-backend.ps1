param([string]$Distribution = 'Russet-Dev')
$ErrorActionPreference = 'Stop'
if ($Distribution -ne 'Russet-Dev') { throw 'This experiment owns only Russet-Dev.' }
$runtimeRoot = Join-Path $env:LOCALAPPDATA 'Russet\Backends\Dev'
$rootfs = Join-Path $runtimeRoot 'ubuntu-26.04-wsl-amd64.wsl'
$expected = '96c7f5fb28a7fe28245331f9bfbe4375f18dd29a4850116ad3c4f60f6700c55c'
$uri = 'https://releases.ubuntu.com/resolute/ubuntu-26.04-wsl-amd64.wsl'
$wsl = Join-Path $env:SystemRoot 'System32\wsl.exe'
$names = (& $wsl --list --quiet) -replace "`0", ''
if ($names.Trim() -contains $Distribution) { throw 'Russet-Dev already exists; refusing to overwrite.' }
if ((Get-PSDrive C).Free -lt 25GB) { throw 'Need 25 GiB free including reserve.' }
New-Item -ItemType Directory -Path $runtimeRoot -Force | Out-Null
if (-not (Test-Path -LiteralPath $rootfs)) {
    $length = [long](Invoke-WebRequest -Method Head -Uri $uri).Headers['Content-Length'][0]
    if ($length -gt 500000000) { throw 'Rootfs exceeds repository download approval boundary.' }
    Invoke-WebRequest -Uri $uri -OutFile "$rootfs.partial"
    if ((Get-FileHash -LiteralPath "$rootfs.partial" -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
        throw 'Rootfs SHA256 mismatch; no import performed.'
    }
    Move-Item -LiteralPath "$rootfs.partial" -Destination $rootfs
}
if ((Get-FileHash -LiteralPath $rootfs -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Rootfs SHA256 mismatch.' }
& $wsl --import $Distribution (Join-Path $runtimeRoot 'distro') $rootfs --version 2
if ($LASTEXITCODE -ne 0) { throw 'WSL import failed.' }
& $wsl -d $Distribution -u root --exec sh -c 'useradd --create-home --shell /bin/bash russet && install -d -m 700 -o russet -g russet /home/russet/.russet-dev'
if ($LASTEXITCODE -ne 0) { throw 'Managed backend user setup failed.' }
& $wsl -d $Distribution -u russet --exec sh -c 'id -u; cat /etc/os-release; command -v python3; df -h /'
