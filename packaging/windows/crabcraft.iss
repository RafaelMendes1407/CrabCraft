; Windows installer, built by .github/workflows/release.yml. Pass the version with /DVersion=x.y.z.
#ifndef Version
  #error Pass /DVersion=x.y.z
#endif

[Setup]
AppId={{6E1F4C52-9B7A-4D0E-8C13-5A2D7F3B9E61}
AppName=CrabCraft
AppVersion={#Version}
AppPublisher=CrabCraft contributors
AppPublisherURL=https://github.com/RafaelMendes1407/CrabCraft
DefaultDirName={autopf}\CrabCraft
DefaultGroupName=CrabCraft
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE-MIT
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\CrabCraft.exe
OutputDir=..\..
OutputBaseFilename=CrabCraft-{#Version}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\CrabCraft\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\CrabCraft"; Filename: "{app}\CrabCraft.exe"
Name: "{autodesktop}\CrabCraft"; Filename: "{app}\CrabCraft.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\CrabCraft.exe"; Description: "{cm:LaunchProgram,CrabCraft}"; Flags: nowait postinstall skipifsilent
