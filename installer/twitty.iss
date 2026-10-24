; Inno Setup script for the Twitty installer (see docs/development.md).
;
;   iscc /DAppVersion=0.1.0 installer\twitty.iss
;
; Expects a release build in target\release\twitty.exe and writes
; target\installer\twitty-setup-<version>-x64.exe.

#ifndef AppVersion
  #error Pass the version: iscc /DAppVersion=x.y.z installer\twitty.iss
#endif

#define AppName "Twitty"
#define AppExe "twitty.exe"
; SourcePath is this script's folder, with a trailing backslash.
#define Root SourcePath + ".."

[Setup]
; Identifies the app for upgrades and uninstall: never change it.
AppId={{36953B39-0EF5-45C3-B6F3-915272916BC8}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Twitty
AppPublisherURL=https://github.com/Astalfr/twittyv5
VersionInfoVersion={#AppVersion}
; Per-user install by default: no admin rights needed, and everything Twitty
; keeps (settings, decks, session) is per user anyway.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SetupIconFile={#Root}\crates\twitty\resources\twitty.ico
UninstallDisplayIcon={app}\{#AppExe}
OutputDir={#Root}\target\installer
OutputBaseFilename=twitty-setup-{#AppVersion}-x64
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Upgrading while Twitty runs closes it first (Restart Manager).
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; The exe embeds every asset (themes, fonts, icons, sound).
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

; Uninstall leaves the user's data (%APPDATA%\Twitty, logs, the session in
; Credential Manager) in place, so a reinstall keeps decks and sign-in.
