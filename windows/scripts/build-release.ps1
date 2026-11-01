param(
    [Parameter(Mandatory = $true)] [string]$Version,
    [Parameter(Mandatory = $true)] [string]$EditorZip,
    [Parameter(Mandatory = $true)] [string]$ExtensionVsix,
    [Parameter(Mandatory = $true)] [string]$HostPublish,
    [Parameter(Mandatory = $true)] [string]$LauncherPublish,
    [Parameter(Mandatory = $true)] [string]$Engine,
    [Parameter(Mandatory = $true)] [string]$Brand,
    [Parameter(Mandatory = $true)] [string]$Out,
    [string]$Rcedit,
    [string]$Iscc
)
# Builds Russet's Windows release: VSCodium's pinned editor branded as Russet with Russet's
# extension inside, the self-contained launcher and host, and the backend payload the setup
# page installs into WSL (the Linux engine and the backend scripts). With -Iscc it also
# compiles the installer (installer\russet.iss) and writes its SHA-256 beside it.
#
#   $Engine   a folder with the Linux engine (`russet`) and `russet-adi-host`
#   $Brand    russet-integration\docs\design\brand (make_icons.py --raster output)
$ErrorActionPreference = 'Stop'
$EditorVersion = '1.143.17232'
$EditorSha256 = 'a1090868af8ac0ac04c22d79fdd02246fa115a78daf38a741d8db06df1753c1d'
$here = Split-Path -Parent $PSCommandPath
$windows = Split-Path -Parent $here
# Absolute paths from here on: Inno Setup reads relative ones against the .iss file's folder.
$EditorZip, $ExtensionVsix, $HostPublish, $LauncherPublish, $Engine, $Brand = @(
    $EditorZip, $ExtensionVsix, $HostPublish, $LauncherPublish, $Engine, $Brand) | ForEach-Object { (Resolve-Path -LiteralPath $_).Path }
if ($Rcedit) { $Rcedit = (Resolve-Path -LiteralPath $Rcedit).Path }
$Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
$stage = Join-Path $Out 'stage'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage | Out-Null

# The editor: VSCodium, checked against its pinned release.
if ((Get-FileHash -LiteralPath $EditorZip -Algorithm SHA256).Hash.ToLowerInvariant() -ne $EditorSha256) {
    throw "The editor archive is not VSCodium $EditorVersion (SHA-256 mismatch)."
}
$editor = Join-Path $stage 'editor'
Expand-Archive -LiteralPath $EditorZip -DestinationPath $editor
$productPath = Join-Path $editor 'resources\app\product.json'
$product = Get-Content -LiteralPath $productPath -Raw | ConvertFrom-Json -AsHashtable
$names = @{
    nameShort = 'Russet'; nameLong = 'Russet'; applicationName = 'russet'; dataFolderName = '.russet'
    win32DirName = 'Russet'; win32NameVersion = 'Russet'; win32RegValueName = 'Russet'
    win32AppUserModelId = 'Russet.Russet'; win32MutexName = 'russet'; win32ShellNameShort = 'Russet'
    urlProtocol = 'russet'; serverApplicationName = 'russet-server'; tunnelApplicationName = 'russet-tunnel'
    reportIssueUrl = 'https://github.com/evilcomputer12/russet/issues/new'
}
foreach ($key in $names.Keys) { $product[$key] = $names[$key] }
# Russet updates itself from its own releases, never from VSCodium's feed.
foreach ($key in 'updateUrl', 'downloadUrl') { $product.Remove($key) }
$product | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $productPath -Encoding utf8NoBOM
Rename-Item -LiteralPath (Join-Path $editor 'VSCodium.exe') -NewName 'Russet.exe'
$manifest = Join-Path $editor 'VSCodium.VisualElementsManifest.xml'
if (Test-Path -LiteralPath $manifest) { Rename-Item -LiteralPath $manifest -NewName 'Russet.VisualElementsManifest.xml' }

# Russet's extension, built into the editor like its own extensions.
$unpacked = Join-Path $Out 'vsix'
if (Test-Path -LiteralPath $unpacked) { Remove-Item -LiteralPath $unpacked -Recurse -Force }
Expand-Archive -LiteralPath $ExtensionVsix -DestinationPath $unpacked
Move-Item -LiteralPath (Join-Path $unpacked 'extension') -Destination (Join-Path $editor 'resources\app\extensions\russet')
Remove-Item -LiteralPath $unpacked -Recurse -Force
& (Join-Path $here 'brand-editor.ps1') -Editor $editor -Brand $Brand -Rcedit $Rcedit

# The Windows side: the launcher at the root, the supervisor in host\.
Copy-Item -LiteralPath $HostPublish -Destination (Join-Path $stage 'host') -Recurse
Get-ChildItem -LiteralPath $LauncherPublish -File | Where-Object { $_.Name -like 'Russet.Launcher*' -and $_.Extension -ne '.pdb' } |
    Copy-Item -Destination $stage

# The backend payload: setup installs it under /opt/russet in the Russet-Dev distribution.
$bin = Join-Path $stage 'backend\bin'
$lib = Join-Path $stage 'backend\lib'
New-Item -ItemType Directory -Path $bin, (Join-Path $lib 'patches') | Out-Null
Copy-Item -LiteralPath (Join-Path $Engine 'russet') -Destination (Join-Path $bin 'russet-dev')
Copy-Item -LiteralPath (Join-Path $Engine 'russet-adi-host') -Destination $bin
foreach ($script in 'vphone_component.sh', 'vphone_debug.py', 'xtool_component.sh', 'socket_relay.py', 'device_mux.py') {
    Copy-Item -LiteralPath (Join-Path $windows "backend\$script") -Destination $lib
}
Copy-Item -Path (Join-Path $windows 'backend\patches\*.patch') -Destination (Join-Path $lib 'patches')
@{ type = 'release'; version = $Version; editor = $EditorVersion; backend = 'Russet-Dev' } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'installation.json') -Encoding utf8NoBOM
Write-Output "Staged Russet $Version in $stage"

if ($Iscc) {
    & $Iscc "/DVersion=$Version" "/DStage=$stage" "/DOutput=$Out" "/DIcon=$(Join-Path $Brand 'russet.ico')" (Join-Path $windows 'installer\russet.iss')
    if ($LASTEXITCODE -ne 0) { throw 'The installer did not compile.' }
    $setup = Join-Path $Out "RussetSetup-$Version.exe"
    $hash = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$setup.sha256" -Value "$hash  RussetSetup-$Version.exe" -Encoding ascii -NoNewline
    Write-Output "Built $setup ($hash)"
}
