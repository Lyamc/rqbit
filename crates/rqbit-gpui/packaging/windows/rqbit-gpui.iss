; Inno Setup script for the standalone rqbit GPUI client (per-user install,
; no admin rights). Build on Windows with Inno Setup 6:
;   iscc /DAppVersion=9.0.0 /DSourceExe=..\..\..\..\target\release\rqbit-gpui.exe rqbit-gpui.iss
; The "default app" checkbox is opt-in (unchecked). Uninstall always removes
; the registration.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\..\..\target\release\rqbit-gpui.exe"
#endif

[Setup]
AppId={{6B1E5C1A-7D0B-4E0B-9E33-2B7F3C0D9A51}
AppName=rqbit
AppVersion={#AppVersion}
AppPublisher=rqbit
DefaultDirName={localappdata}\Programs\rqbit
DefaultGroupName=rqbit
PrivilegesRequired=lowest
OutputBaseFilename=rqbit-gpui-{#AppVersion}-setup
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\rqbit-gpui.exe
DisableProgramGroupPage=yes

[Tasks]
Name: "defaultapp"; Description: "Open magnet links and .torrent files with rqbit"; Flags: unchecked
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "rqbit-gpui.exe"; Flags: ignoreversion
Source: "register-handlers.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\rqbit"; Filename: "{app}\rqbit-gpui.exe"
Name: "{userdesktop}\rqbit"; Filename: "{app}\rqbit-gpui.exe"; Tasks: desktopicon

[Run]
; Registers for the current user and opens Settings > Default apps, where
; Windows 10/11 require the user to confirm rqbit.
Filename: "{app}\rqbit-gpui.exe"; Parameters: "--register-handlers"; Tasks: defaultapp; Flags: runhidden waituntilterminated
Filename: "{app}\rqbit-gpui.exe"; Description: "Start rqbit"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\rqbit-gpui.exe"; Parameters: "--unregister-handlers"; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterHandlers"
