param(
    [Parameter(Mandatory = $true)] [string]$Editor,
    [Parameter(Mandatory = $true)] [string]$Brand,
    [string]$Rcedit
)
# Puts Russet's mark on the Code-OSS editor folder ($Editor: Russet.exe and resources\): the window
# and taskbar icon (Russet.exe's own icon, with Electron's rcedit when -Rcedit is given), the Start
# menu tiles and the empty editor's watermark. $Brand is russet-integration\docs\design\brand, which
# make_icons.py --raster fills. Re-runnable: it only replaces the upstream files.
$ErrorActionPreference = 'Stop'
$Editor = [IO.Path]::GetFullPath($Editor)
$Brand = [IO.Path]::GetFullPath($Brand)
$app = Join-Path $Editor 'resources\app'
$win32 = Join-Path $app 'resources\win32'
$media = Join-Path $app 'out\media'
foreach ($dir in $win32, $media) { if (-not (Test-Path -LiteralPath $dir)) { throw "No editor at $Editor ($dir is missing)." } }
$icon = Join-Path $Brand 'russet.ico'
Copy-Item -LiteralPath $icon -Destination (Join-Path $win32 'code.ico') -Force
foreach ($tile in 'code_150x150.png', 'code_70x70.png') {
    Copy-Item -LiteralPath (Join-Path $Brand "editor\$tile") -Destination (Join-Path $win32 $tile) -Force
}
foreach ($theme in 'dark', 'light', 'hcDark', 'hcLight') {
    Copy-Item -LiteralPath (Join-Path $Brand "editor\letterpress-$theme.svg") -Destination (Join-Path $media "letterpress-$theme.svg") -Force
}
# The app icon in the window's own title bar.
Copy-Item -LiteralPath (Join-Path $Brand 'russet-icon-light.svg') -Destination (Join-Path $media 'code-icon.svg') -Force
if ($Rcedit) {
    $exe = Get-ChildItem -LiteralPath $Editor -Filter '*.exe' -File | Where-Object { $_.Name -notmatch '^unins' } | Select-Object -First 1
    if (-not $exe) { throw "No editor executable in $Editor." }
    & $Rcedit $exe.FullName --set-icon $icon
    if ($LASTEXITCODE -ne 0) { throw "rcedit could not set $($exe.Name)'s icon." }
}
Write-Output "Branded the editor at $Editor"
