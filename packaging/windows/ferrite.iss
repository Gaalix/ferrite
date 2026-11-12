#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif
#ifndef StageDir
  #define StageDir "..\..\dist\win-stage"
#endif

[Setup]
AppId={{8F6E2A19-4C7D-4B9E-A312-9D4C0E7F1B52}
AppName=Ferrite
AppVersion={#MyAppVersion}
AppVerName=Ferrite {#MyAppVersion}
AppPublisher=Gaalix
AppPublisherURL=https://github.com/Gaalix/ferrite
AppSupportURL=https://github.com/Gaalix/ferrite/issues
AppUpdatesURL=https://github.com/Gaalix/ferrite/releases
DefaultDirName={autopf}\Ferrite
DefaultGroupName=Ferrite
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=Ferrite-Setup-{#MyAppVersion}-x64
SetupIconFile=..\..\assets\icons\ferrite.ico
UninstallDisplayIcon={app}\ferrite.exe
UninstallDisplayName=Ferrite
LicenseFile=..\..\LICENSE
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\ferrite.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\libmpv-2.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\assets\icons\ferrite.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Ferrite"; Filename: "{app}\ferrite.exe"; IconFilename: "{app}\ferrite.ico"
Name: "{autodesktop}\Ferrite"; Filename: "{app}\ferrite.exe"; IconFilename: "{app}\ferrite.ico"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\stremio"; ValueType: string; ValueName: ""; ValueData: "URL:Stremio Addon Protocol"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\stremio"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\stremio\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\ferrite.exe"",0"
Root: HKA; Subkey: "Software\Classes\stremio\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ferrite.exe"" ""%1"""

[Run]
Filename: "{app}\ferrite.exe"; Description: "{cm:LaunchProgram,Ferrite}"; Flags: nowait postinstall skipifsilent
