; Lensz installer (Inno Setup 6). Build with installer\build.ps1, which stages the files into
; installer\stage first.

#define AppName "Lensz"
#define AppVersion "0.1.2"
#define Stage "stage"
; The face photo used for the look previews; set to 0 before sharing the installer unless you
; have the right to redistribute that photo.
#define IncludePreviewPhoto 0

[Setup]
AppId={{6B0E3B52-2D7C-4E3A-9C1B-7A4F2E8D5C31}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppName}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=output
OutputBaseFilename=LenszSetup-{#AppVersion}
SetupIconFile=lensz.ico
UninstallDisplayIcon={app}\lensz.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; The cameras are registered for the whole PC, which needs admin rights.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 11: the "Lensz" Windows virtual camera needs it.
MinVersion=10.0.22000
; Close a running Lensz (and ask apps holding the camera files) before updating.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "startup"; Description: "Start Lensz in the tray when Windows starts (so the camera is always ready)"; GroupDescription: "Startup:"

[Files]
Source: "{#Stage}\lensz.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\DirectML.dll"; DestDir: "{app}"; Flags: ignoreversion
; Camera DLLs are loaded by other programs (the Windows camera service, OBS…): register them, and
; replace them after a restart if they are in use.
Source: "{#Stage}\lens_vcam.dll"; DestDir: "{app}"; Flags: ignoreversion regserver restartreplace uninsrestartdelete
Source: "{#Stage}\lensz_dshow.dll"; DestDir: "{app}"; Flags: ignoreversion regserver restartreplace uninsrestartdelete
Source: "{#Stage}\models\*"; DestDir: "{app}\models"; Flags: ignoreversion recursesubdirs
#if IncludePreviewPhoto
Source: "{#Stage}\assets\preview_face.png"; DestDir: "{app}\assets"; Flags: ignoreversion skipifsourcedoesntexist
#endif
Source: "THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: VCRedistNeeded

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\lensz.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\lensz.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\lensz.exe"" --tray"; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "Installing the Microsoft Visual C++ runtime…"; Check: VCRedistNeeded; Flags: waituntilterminated
Filename: "{app}\lensz.exe"; Description: "Start Lensz now"; Flags: postinstall nowait skipifsilent runasoriginaluser

[UninstallRun]
; Quit Lensz, then take the "Lensz" Windows camera out of video apps.
Filename: "{sys}\taskkill.exe"; Parameters: "/im lensz.exe /f"; Flags: runhidden; RunOnceId: "QuitLensz"
Filename: "{app}\lensz.exe"; Parameters: "--remove-camera"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveCamera"

[Code]
// The Visual C++ 2015-2022 runtime (x64) is needed by Lensz and its camera DLLs.
function VCRedistNeeded: Boolean;
var
  Installed, Major, Minor: Cardinal;
begin
  Result := True;
  if RegQueryDWordValue(HKLM64, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Installed', Installed) and (Installed = 1) and
     RegQueryDWordValue(HKLM64, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Major', Major) and
     RegQueryDWordValue(HKLM64, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Minor', Minor) then
    Result := (Major < 14) or ((Major = 14) and (Minor < 44));
end;
