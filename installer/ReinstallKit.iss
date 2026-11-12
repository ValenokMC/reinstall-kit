; Reinstall Kit installer (Inno Setup 6). Built by: src\build.ps1 -Installer
; Installs for the current user only, so no administrator rights are needed and the settings file
; next to the program stays writable.
#define AppVer "1.0.0"

[Setup]
AppId={{6E0B7C4A-52D9-4E0E-9A54-3D0F1B7A9C21}
AppName=Reinstall Kit
AppVersion={#AppVer}
AppVerName=Reinstall Kit {#AppVer}
AppPublisher=Reinstall Kit contributors
DefaultDirName={localappdata}\Programs\ReinstallKit
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=ReinstallKit-setup
SetupIconFile=..\assets\ReinstallKit.ico
UninstallDisplayIcon={app}\ReinstallKit.exe
UninstallDisplayName=Reinstall Kit
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.TaskSchedule=Update the kit automatically every day (a task in Windows Task Scheduler)
russian.TaskSchedule=Обновлять набор автоматически каждый день (задача в планировщике Windows)
english.TaskDesktop=Create a desktop shortcut
russian.TaskDesktop=Создать ярлык на рабочем столе
english.RunApp=Open Reinstall Kit and set it up
russian.RunApp=Открыть Reinstall Kit и настроить

[Tasks]
Name: "schedule"; Description: "{cm:TaskSchedule}"
Name: "desktopicon"; Description: "{cm:TaskDesktop}"

[Files]
; settings.json, the journal and logs belong to the user and are never part of the installer.
Source: "..\dist\ReinstallKit\*"; DestDir: "{app}"; Excludes: "settings.json,catalog.json,journal.txt,\logs\*"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\Reinstall Kit"; Filename: "{app}\ReinstallKit.exe"
Name: "{autodesktop}\Reinstall Kit"; Filename: "{app}\ReinstallKit.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ReinstallKit.exe"; Parameters: "--schedule"; Flags: runhidden; Tasks: schedule
Filename: "{app}\ReinstallKit.exe"; Description: "{cm:RunApp}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\ReinstallKit.exe"; Parameters: "--unschedule"; Flags: runhidden; RunOnceId: "RemoveSchedule"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\logs"
Type: files; Name: "{app}\journal.txt"
