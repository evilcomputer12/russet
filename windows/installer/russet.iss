; Russet's Windows installer (Inno Setup 6): a per-user install, no administrator rights.
; scripts\build-release.ps1 stages the files and passes Version, Stage, Output and Icon.
; An update runs it with /SILENT /CLOSEAPPLICATIONS /UPDATE=1: it closes Russet, replaces the
; editor, host and backend payload, and starts Russet again. Uninstalling leaves Russet's WSL
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
