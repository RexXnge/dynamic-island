; Inno Setup script for Dynamic Island (https://jrsoftware.org/isinfo.php).
; Build the app first (`cargo build --release` in the repo root), then:
;   ISCC.exe installer\setup.iss      -> dist\DynamicIsland-Setup.exe
; Installs per user (no admin) to %LOCALAPPDATA%\Programs\Dynamic Island.

#define AppName "Dynamic Island"
#define AppVersion "1.2.0"
#define AppExe "dynamic-island.exe"

[Setup]
AppId={{6E1B8C52-3F0A-4B7D-9C2E-8A41D5F7B930}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=RexXnge
AppPublisherURL=https://github.com/RexXnge/dynamic-island
AppSupportURL=https://github.com/RexXnge/dynamic-island/issues
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=DynamicIsland-Setup
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile=..\assets\icon.ico
UninstallDisplayName={#AppName}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "Start with Windows"; GroupDescription: "Options:"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Same value the app's own "Start with Windows" switch writes, so the two stay in sync.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "DynamicIsland"; ValueData: """{app}\{#AppExe}"""; Tasks: startup

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall

[Code]
// The island has no window to close politely, so stop it before files are replaced or removed.
procedure StopIsland();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopIsland();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  StopIsland();
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  // The app can switch startup on by itself, so always clear it, chosen at install or not.
  // Settings in %APPDATA%\DynamicIsland are kept for a reinstall.
  if CurUninstallStep = usUninstall then
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'DynamicIsland');
end;
