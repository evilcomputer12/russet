; Russet's Windows installer (Inno Setup 6): a per-user install, no administrator rights.
; scripts\build-release.ps1 stages the files and passes Version, Stage, Output and Icon.
; An update runs it with /SILENT /CLOSEAPPLICATIONS /UPDATE=1 while Russet quits itself: it ends what
; outlives the editor (PrepareToInstall), replaces the editor, host and backend payload, and starts
; Russet again. Uninstalling leaves Russet's WSL
; distribution and projects in place.

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId={{6F0C2B9E-8B3B-4D0A-9E7A-1C5D2A7F4B31}
AppName=Russet
AppVersion={#Version}
AppVerName=Russet {#Version}
AppPublisher=Russet contributors
AppPublisherURL=https://github.com/evilcomputer12/russet
AppSupportURL=https://github.com/evilcomputer12/russet/issues
AppUpdatesURL=https://github.com/evilcomputer12/russet/releases
DefaultDirName={localappdata}\Programs\Russet
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
OutputDir={#Output}
OutputBaseFilename=RussetSetup-{#Version}
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\Russet.Launcher.exe
UninstallDisplayName=Russet
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[InstallDelete]
; No stale files across updates: these folders come whole from each release.
Type: filesandordirs; Name: "{app}\editor"
Type: filesandordirs; Name: "{app}\host"
Type: filesandordirs; Name: "{app}\backend"

[Files]
Source: "{#Stage}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{autoprograms}\Russet"; Filename: "{app}\Russet.Launcher.exe"
Name: "{autodesktop}\Russet"; Filename: "{app}\Russet.Launcher.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Russet.Launcher.exe"; Description: "Open Russet"; Flags: nowait postinstall skipifsilent
Filename: "{app}\Russet.Launcher.exe"; Flags: nowait; Check: IsUpdate

[Code]
function IsUpdate: Boolean;
begin
  Result := ExpandConstant('{param:UPDATE|0}') = '1';
end;

// An update from the IDE (which quits itself): once Russet's windows have closed, end the processes of this
// installation that outlive the editor and hold the files being replaced: the Russet IDE MCP server an agent
// keeps running, a socket relay, a setup step. Only those processes: their WSL processes and a VM keep running.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Root: String;
  Code: Integer;
begin
  Result := '';
  if not IsUpdate then Exit;
  Root := ExpandConstant('{app}') + '\';
  StringChangeEx(Root, '''', '''''', True);
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -Command "$r = ''' + Root + '''; ' +
    'function Mine { Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($r, [StringComparison]::OrdinalIgnoreCase) } }; ' +
    '$end = (Get-Date).AddMinutes(10); ' +
    'while ((Mine | Where-Object { $_.MainWindowHandle -ne 0 }) -and (Get-Date) -lt $end) { Start-Sleep 1 }; ' +
    'if (Mine) { Start-Sleep 20; Mine | Where-Object { $_.MainWindowHandle -eq 0 } | Stop-Process -Force }"',
    '', SW_HIDE, ewWaitUntilTerminated, Code);
end;
