; Build with Inno Setup 6.4 or newer. All payload paths are provided by
; scripts/installer.ps1; never point SourceDir at a user's data directory.
#if Ver < 0x06040000
  #error Inno Setup 6.4 or newer is required
#endif
#ifndef AppVersion
  #error AppVersion must be supplied
#endif
#ifndef SourceDir
  #error SourceDir must be supplied
#endif
#ifndef OutputDir
  #error OutputDir must be supplied
#endif
#ifndef BootstrapperFile
  #error BootstrapperFile must be supplied
#endif

#define ProductName "LeigodGuard"
#define ProductExe "leigod-guard.exe"
#define ProjectURL "https://github.com/CMMUU/leigod-guard"
#define InstallerDir AddBackslash(SourcePath)
#ifndef SourceRoot
  ; Fixture installers compile this same .iss against a temporary payload; the
  ; branding asset always comes from the checkout, not the payload directory.
  #define SourceRoot InstallerDir + ".."
#endif
#define AppIconFile AddBackslash(SourceRoot) + "assets\app-icon.ico"

[Setup]
; Keep this identifier unchanged so future versions upgrade this installation.
AppId={{6ABF5F53-AFD4-4D24-A930-174CF6B21B7A}
AppName={cm:AppDisplayName}
AppVersion={#AppVersion}
AppVerName={cm:AppDisplayName} {#AppVersion}
AppPublisher=CMMUU
AppPublisherURL={#ProjectURL}
AppSupportURL={#ProjectURL}/issues
AppUpdatesURL={#ProjectURL}/releases/latest
LicenseFile={#SourceDir}\licenses\webview2-runtime\INSTALLER-LICENSE.txt
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#ProductName}
VersionInfoDescription=LeigodGuard Setup
DefaultDirName={localappdata}\Programs\LeigodGuard
DefaultGroupName=LeigodGuard
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0
UsePreviousAppDir=yes
UsePreviousTasks=yes
UninstallDisplayIcon={app}\{#ProductExe}
SetupIconFile={#AppIconFile}
UninstallDisplayName={cm:AppDisplayName}
AppMutex=Local\LeigodGuard
; Never ask Restart Manager to close a process, even during a silent upgrade.
CloseApplications=no
RestartApplications=no
SetupMutex=Local\LeigodGuardSetup
WizardStyle=modern
ShowLanguageDialog=auto
Compression=lzma2
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=leigod-guard-v{#AppVersion}-windows-x64-setup
SetupLogging=yes

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Default.isl,{#InstallerDir}languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
chinesesimplified.AppDisplayName=雷神守护
english.AppDisplayName=LeigodGuard
chinesesimplified.DesktopIcon=创建桌面快捷方式(&D)
english.DesktopIcon=Create a &desktop shortcut
chinesesimplified.AdditionalIcons=快捷方式：
english.AdditionalIcons=Shortcuts:
chinesesimplified.LaunchApp=启动雷神守护
english.LaunchApp=Launch LeigodGuard
chinesesimplified.ProjectWebsite=使用说明与更新
english.ProjectWebsite=Help and updates
chinesesimplified.AppRunning=雷神守护正在运行。请先从系统托盘右键菜单退出雷神守护，然后重新开始安装。
english.AppRunning=LeigodGuard is running. Exit it from its system tray menu, then start installation again.
chinesesimplified.RuntimeTitle=准备所需组件
english.RuntimeTitle=Preparing required components
chinesesimplified.RuntimeDescription=正在为雷神守护准备登录组件。
english.RuntimeDescription=Preparing the sign-in component for LeigodGuard.
chinesesimplified.RuntimeInstalling=正在安装 Microsoft Edge WebView2。请保持网络连接，这可能需要几分钟。
english.RuntimeInstalling=Installing Microsoft Edge WebView2. Stay connected to the internet; this may take a few minutes.
chinesesimplified.RuntimeFailed=Microsoft Edge WebView2 未能安装完成。请检查网络连接后点击“重试”，或点击“取消”返回，稍后重新安装。
english.RuntimeFailed=Microsoft Edge WebView2 could not be installed. Check your internet connection and select Retry, or select Cancel to return and run Setup again later.
chinesesimplified.RuntimeRestart=Microsoft Edge WebView2 需要重启电脑才能完成安装。请重启后再次运行雷神守护安装程序。
english.RuntimeRestart=Microsoft Edge WebView2 needs a computer restart to finish installing. Restart your computer, then run LeigodGuard Setup again.

[Messages]
english.ConfirmUninstall=Remove %1 and its program files? Saved account and preference settings will be kept.
english.UninstalledAll=%1 was successfully removed. Saved account and preference settings remain on this computer.

[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
; Keep the prerequisite first so solid compression can extract it promptly.
Source: "{#BootstrapperFile}"; DestDir: "{tmp}"; DestName: "MicrosoftEdgeWebview2Setup.exe"; Flags: dontcopy
; SourceDir is an isolated payload staging directory. Keep this list explicit;
; personal configuration, logs, credentials, and screenshots are not payload.
Source: "{#SourceDir}\{#ProductExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\config.example.toml"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\docs\PRIVACY.md"; DestDir: "{app}\docs"; Flags: ignoreversion
Source: "{#SourceDir}\docs\RELEASING.md"; DestDir: "{app}\docs"; Flags: ignoreversion
Source: "{#SourceDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#InstallerDir}WEBVIEW2-NOTICE.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{group}\{cm:AppDisplayName}"; Filename: "{app}\{#ProductExe}"; WorkingDir: "{app}"
Name: "{group}\{cm:ProjectWebsite}"; Filename: "{#ProjectURL}#readme"
Name: "{autodesktop}\{cm:AppDisplayName}"; Filename: "{app}\{#ProductExe}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#ProductExe}"; WorkingDir: "{app}"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent

[Code]
const
  WebView2ClientKey = 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  StartupKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  StartupValueName = 'LeigodGuard';

var
  RuntimeProgress: TOutputProgressWizardPage;

function HasRuntimeVersion(RootKey: Integer): Boolean;
var
  VersionText: String;
  VersionNumber: Int64;
begin
  Result := False;
  if RegQueryStringValue(RootKey, WebView2ClientKey, 'pv', VersionText) then
    if StrToVersion(Trim(VersionText), VersionNumber) then
      Result := VersionNumber > 0;
end;

function HasWebView2Runtime: Boolean;
begin
  { Microsoft's documented per-user key and 32-bit machine key, plus both
    explicit registry views to support pre-existing per-machine deployments.
    https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution }
  Result := HasRuntimeVersion(HKCU32) or HasRuntimeVersion(HKLM32);
  if (not Result) and IsWin64 then
    Result := HasRuntimeVersion(HKCU64) or HasRuntimeVersion(HKLM64);
end;

procedure InitializeWizard;
begin
  RuntimeProgress := CreateOutputProgressPage(
    CustomMessage('RuntimeTitle'), CustomMessage('RuntimeDescription'));
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExitCode: Integer;
  Started: Boolean;
  Installed: Boolean;
begin
  Result := '';
  if CheckForMutexes('Local\LeigodGuard') then
  begin
    Result := CustomMessage('AppRunning');
    Exit;
  end;
  while not HasWebView2Runtime do
  begin
    Started := False;
    Installed := False;
    ExitCode := -1;
    RuntimeProgress.SetText(CustomMessage('RuntimeInstalling'), '');
    if not WizardSilent then
      RuntimeProgress.Show;
    try
      try
        ExtractTemporaryFile('MicrosoftEdgeWebview2Setup.exe');
        Started := Exec(ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe'),
          '/silent /install', ExpandConstant('{tmp}'), SW_HIDE,
          ewWaitUntilTerminated, ExitCode);
        { Starting the bootstrapper is not sufficient: await completion and
          independently detect the installed Runtime before copying our app. }
        Installed := Started and (ExitCode = 0) and HasWebView2Runtime;
        Log(Format('WebView2 bootstrapper exit code: %d', [ExitCode]));
      except
        Log('WebView2 prerequisite could not be prepared or started.');
      end;
    finally
      if not WizardSilent then
        RuntimeProgress.Hide;
    end;

    if Installed then
      Break;

    if Started and (ExitCode = 3010) then
    begin
      NeedsRestart := True;
      Result := CustomMessage('RuntimeRestart');
      Exit;
    end;

    Result := CustomMessage('RuntimeFailed');
    if WizardSilent then
      Exit;
    if MsgBox(Result, mbError, MB_RETRYCANCEL) <> IDRETRY then
      Exit;
    Result := '';
  end;
  { The user may have launched the app while the prerequisite was installing. }
  if CheckForMutexes('Local\LeigodGuard') then
    Result := CustomMessage('AppRunning');
end;

function StartupExecutable(const Command: String): String;
var
  Value: String;
  Separator: Integer;
begin
  Result := '';
  Value := Trim(Command);
  if Value = '' then
    Exit;
  if Value[1] = '"' then
  begin
    Delete(Value, 1, 1);
    Separator := Pos('"', Value);
    if Separator > 0 then
      Result := Copy(Value, 1, Separator - 1);
  end
  else
  begin
    Separator := Pos(' ', Value);
    if Separator > 0 then
      Result := Copy(Value, 1, Separator - 1)
    else
      Result := Value;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ExistingCommand: String;
begin
  if CurStep = ssPostInstall then
  begin
    { Preserve an existing opt-in; do not enable startup for new users. This
      also migrates the previous portable executable's startup registration. }
    if RegQueryStringValue(HKCU, StartupKey, StartupValueName, ExistingCommand) then
      if CompareText(ExtractFileName(StartupExecutable(ExistingCommand)), '{#ProductExe}') = 0 then
        if not RegWriteStringValue(HKCU, StartupKey, StartupValueName,
          '"' + ExpandConstant('{app}\{#ProductExe}') + '" --minimized') then
          Log('Existing startup preference could not be updated.');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ExistingCommand: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    { Remove only a startup entry owned by this installation. Never remove
      account settings, shared WebView2, or another installation's entry. }
    if RegQueryStringValue(HKCU, StartupKey, StartupValueName, ExistingCommand) then
      if CompareText(StartupExecutable(ExistingCommand), ExpandConstant('{app}\{#ProductExe}')) = 0 then
        RegDeleteValue(HKCU, StartupKey, StartupValueName);
  end;
end;
