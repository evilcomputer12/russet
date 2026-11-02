param(
    [string]$Drive,
    [string]$Folder,
    [string]$Distribution = 'Russet-Dev',
    [switch]$Check,
    [string]$WaitFor,
    [string]$Reopen
)
# Moves Russet's WSL disk (the Russet-Dev distribution's ext4.vhdx) to another folder or drive
# with WSL's own `wsl --manage <distribution> --move`. Russet finds the disk through WSL's
# registration, so it works from the new place as before. Only Russet-Dev stops; the rest of WSL
# and its other distributions keep running. "Russet: Move WSL Disk to Another Drive" runs this
# script; it also runs on its own, with Russet closed:
#
#   powershell -ExecutionPolicy Bypass -File move-wsl-disk.ps1 -Drive D
#   powershell -ExecutionPolicy Bypass -File move-wsl-disk.ps1 -Folder E:\WSL
#
# The disk goes to <Folder>\Russet-Dev\distro (-Drive D: D:\Russet\Russet-Dev\distro), and the
# setup page's downloads (Swift's, which a failed attempt keeps) follow it to <Folder>\Russet-Dev.
#
#   -Check     only say whether the move can start (exit 0) or why not (exit 1)
#   -WaitFor   Russet's installation folder: wait for its processes to exit first (Russet runs
#              this after it closes itself)
#   -Reopen    start this program (Russet's launcher) afterwards, whether or not the move worked
$ErrorActionPreference = 'Stop'
$env:WSL_UTF8 = '1'
$wsl = Join-Path $env:SystemRoot 'System32\wsl.exe'

function Done([int]$code, [string]$message) {
    if ($message) { Write-Host $message -ForegroundColor $(if ($code -eq 0) { 'Green' } else { 'Red' }) }
    if ($Reopen -and -not $Check) {
        Start-Process -FilePath $Reopen
        if ($code -ne 0) { Read-Host 'Press Enter to close this window' | Out-Null } else { Start-Sleep -Seconds 3 }
    }
    exit $code
}
function GB([long]$bytes) { '{0:N1} GB' -f ($bytes / 1GB) }

if ($WaitFor) {
    Write-Host 'Waiting for Russet to close...'
    $root = [IO.Path]::GetFullPath($WaitFor).TrimEnd('\') + '\'
    $deadline = (Get-Date).AddMinutes(2)
    while ((Get-Date) -lt $deadline -and (Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) })) {
        Start-Sleep -Milliseconds 500
    }
}

# Where WSL registered the disk.
$entry = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss' -ErrorAction SilentlyContinue |
    ForEach-Object { Get-ItemProperty -LiteralPath $_.PSPath } | Where-Object { $_.DistributionName -eq $Distribution } | Select-Object -First 1
if (-not $entry) { Done 1 "WSL has no distribution named ${Distribution}: Russet's backend is not installed yet." }
$from = ($entry.BasePath -replace '^\\\\\?\\', '').TrimEnd('\')
$name = if ($entry.VhdFileName) { $entry.VhdFileName } else { 'ext4.vhdx' }
$disk = Get-Item -LiteralPath (Join-Path $from $name) -ErrorAction SilentlyContinue
if (-not $disk) { Done 1 "$Distribution's disk is not where WSL registered it ($from)." }

if ($Drive) { $Folder = $Drive.Trim().TrimEnd(':', '\') + ':\Russet' }
if (-not $Folder) {
    Write-Host "$Distribution's disk ($(GB $disk.Length)) is in $from. Drives:"
    [IO.DriveInfo]::GetDrives() | Where-Object { $_.IsReady -and $_.DriveType -eq 'Fixed' } |
        ForEach-Object { Write-Host ('  {0}  {1} free ({2})' -f $_.Name, (GB $_.AvailableFreeSpace), $_.DriveFormat) }
    Done 1 'Name where it goes: -Drive D, or -Folder E:\WSL.'
}
$data = Join-Path ([IO.Path]::GetFullPath($Folder)) $Distribution
$to = Join-Path $data 'distro'
if ($to.TrimEnd('\') -ieq $from) { Done 0 "$Distribution's disk is already in $to." }
$root = [IO.Path]::GetPathRoot($to)
$target = New-Object IO.DriveInfo $root
if (-not $target.IsReady -or $target.DriveType -ne 'Fixed') { Done 1 "$root is not a local fixed drive; WSL keeps its disks on local NTFS drives." }
if (@('NTFS', 'ReFS') -notcontains $target.DriveFormat) { Done 1 "$root is $($target.DriveFormat); WSL keeps its disks on NTFS drives." }
if (Test-Path -LiteralPath (Join-Path $to $name)) { Done 1 "$to already holds a $name; choose another folder." }
# A move within a drive renames the file; across drives it is copied whole first.
$sameDrive = [IO.Path]::GetPathRoot($from) -ieq $root
$need = if ($sameDrive) { 0 } else { $disk.Length + 2GB }
if ($target.AvailableFreeSpace -lt $need) {
    Done 1 "$root has $(GB $target.AvailableFreeSpace) free; $Distribution's disk takes $(GB $disk.Length), and the move needs $(GB $need)."
}
if ($Check) { Done 0 "$Distribution's disk ($(GB $disk.Length)) can move from $from to $to." }

$open = Get-Process -Name 'Russet', 'Russet.Host', 'Russet.Launcher' -ErrorAction SilentlyContinue
if ($open) { Done 1 'Close Russet first: it keeps Russet-Dev running.' }

Write-Host "Stopping $Distribution (the rest of WSL keeps running)..."
& $wsl --terminate $Distribution | Out-Null
# WSL lets go of a stopped distribution's disk about a minute later; the move fails before.
$deadline = (Get-Date).AddMinutes(4)
while ($true) {
    try { [IO.File]::Open($disk.FullName, 'Open', 'ReadWrite', 'None').Dispose(); break }
    catch [IO.IOException] {
        if ((Get-Date) -gt $deadline) { Done 1 "WSL still holds $Distribution's disk; restart Windows, then move it before opening Russet." }
        Write-Progress -Activity "Waiting for WSL to release $Distribution's disk" -Status 'usually about a minute'
        Start-Sleep -Seconds 1
    }
}
Write-Progress -Activity "Waiting for WSL to release $Distribution's disk" -Completed
New-Item -ItemType Directory -Force -Path $to | Out-Null
Write-Host "Moving $Distribution's disk ($(GB $disk.Length)) from $from to $to."
Write-Host 'Leave this window open; across drives this takes a few minutes per 10 GB.'
$started = Get-Date
$move = Start-Process -FilePath $wsl -ArgumentList @('--manage', $Distribution, '--move', "`"$to`"") -NoNewWindow -PassThru
$null = $move.Handle
while (-not $move.WaitForExit(1000)) {
    $elapsed = (Get-Date) - $started
    Write-Progress -Activity "Moving $Distribution's disk ($(GB $disk.Length))" -Status ('{0:hh\:mm\:ss} elapsed' -f $elapsed)
}
Write-Progress -Activity "Moving $Distribution's disk" -Completed
$moved = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss' |
    ForEach-Object { Get-ItemProperty -LiteralPath $_.PSPath } | Where-Object { $_.DistributionName -eq $Distribution } | Select-Object -First 1
if ($move.ExitCode -ne 0 -or ($moved.BasePath -replace '^\\\\\?\\', '').TrimEnd('\') -ine $to.TrimEnd('\')) {
    Done 1 "WSL did not move the disk (exit $($move.ExitCode)); it stays in $from and Russet works as before."
}

# The setup page's downloads Russet kept beside the disk follow it (Russet's own layout:
# <data>\distro holds the disk).
$oldData = Split-Path -Parent $from
if ((Split-Path -Leaf $from) -eq 'distro') {
    Get-ChildItem -LiteralPath $oldData -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like 'swift-*' -or $_.Name -like 'ubuntu-*.wsl*' } |
        Move-Item -Destination $data -ErrorAction SilentlyContinue
    foreach ($empty in $from, $oldData) {
        if ((Test-Path -LiteralPath $empty) -and -not (Get-ChildItem -LiteralPath $empty -Force)) { Remove-Item -LiteralPath $empty }
    }
}
& $wsl --distribution $Distribution --user root --exec true
if ($LASTEXITCODE -ne 0) { Done 1 "The disk moved to $to, but $Distribution did not start (exit $LASTEXITCODE)." }
Done 0 "Moved in $('{0:hh\:mm\:ss}' -f ((Get-Date) - $started)): $Distribution's disk is now in $to, and Russet uses it from there."
