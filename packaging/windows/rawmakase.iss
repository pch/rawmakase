; RAWmakase's Windows installer, built by packaging/windows/setup.ps1 (through
; native-packages) from the payload packaging/windows/stage.ps1 prepares.
;
; Installs for the current user under %LOCALAPPDATA%\Programs\RAWmakase, so
; it needs no administrator rights. The installer marker tells the app's
; updater that this copy updates by running the next release's setup program
; for its architecture, rawmakase-v<version>-<x86_64|aarch64>-pc-windows-msvc-setup.exe.

#ifndef Version
  #error Version must be defined on the ISCC command line
#endif
#ifndef NumericVersion
  #define NumericVersion Version
#endif
#ifndef Payload
  #error Payload must be defined on the ISCC command line
#endif
#ifndef OutputDir
  #error OutputDir must be defined on the ISCC command line
#endif
#ifndef OutputName
  #error OutputName must be defined on the ISCC command line
#endif
; x64compatible for the x86_64 build, arm64 for the ARM64 build.
#ifndef Architectures
  #define Architectures "x64compatible"
#endif

#define AppName "RAWmakase"
#define AppExeName "rawmakase.exe"

[Setup]
; Never change: this is how Windows tells an update from a new program.
AppId={{5A5F453A-1183-4E22-8B9C-B35362EB39FB}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher=pch
AppPublisherURL=https://github.com/pch/rawmakase
AppSupportURL=https://github.com/pch/rawmakase/issues
AppUpdatesURL=https://github.com/pch/rawmakase/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed={#Architectures}
ArchitecturesInstallIn64BitMode={#Architectures}
MinVersion=10.0
LicenseFile={#Payload}\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
SetupIconFile=rawmakase.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\{#AppExeName}
VersionInfoVersion={#NumericVersion}.0

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#Payload}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "rawmakase-installer.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; AppUserModelID: "io.github.pch.rawmakase"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon; AppUserModelID: "io.github.pch.rawmakase"

[Run]
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
