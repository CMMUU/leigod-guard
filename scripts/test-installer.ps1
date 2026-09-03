#Requires -Version 5.1
<#
.SYNOPSIS
Exercise the real installer on an otherwise unused GitHub-hosted Windows runner.
.DESCRIPTION
Intentionally refuses local or self-hosted execution. Installs into a unique
RUNNER_TEMP directory, uses no real account, never starts the GUI, verifies
shortcuts, upgrade, static CRT linkage, version output and uninstall behavior.
Logs remain under OutputDirectory/installer-test-logs for CI diagnostics.
#>
[CmdletBinding()]
param([string]$OutputDirectory = 'dist')

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
    $env:RUNNER_OS -ne 'Windows' -or -not $env:RUNNER_TEMP) {
    throw 'Installer smoke tests require a fresh GitHub-hosted Windows runner; they must not run on a personal or self-hosted machine.'
}

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
} else {
    $outputRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $OutputDirectory))
}
$testRoot = Join-Path $env:RUNNER_TEMP ('leigod-guard-installer-smoke-' + [guid]::NewGuid().ToString('N'))
$installRoot = Join-Path $testRoot 'installed app'
$logRoot = Join-Path $outputRoot 'installer-test-logs'
$configRoot = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'leigod-guard'
$configPath = Join-Path $configRoot 'config.toml'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{6ABF5F53-AFD4-4D24-A930-174CF6B21B7A}_is1'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$menuRoot = Join-Path ([Environment]::GetFolderPath('Programs')) 'LeigodGuard'
$menuShortcut = Join-Path $menuRoot 'LeigodGuard.lnk'
$desktopShortcut = Join-Path ([Environment]::GetFolderPath('Desktop')) 'LeigodGuard.lnk'
$installedExecutable = Join-Path $installRoot 'leigod-guard.exe'
$uninstaller = Join-Path $installRoot 'unins000.exe'
$ownedStartup = '"' + $installedExecutable + '" --minimized'

function Get-StartupValue {
    return Get-ItemPropertyValue -LiteralPath $runKey -Name 'LeigodGuard' -ErrorAction SilentlyContinue
}

function Assert-NoGuiProcess {
    if (Get-Process -Name 'leigod-guard' -ErrorAction SilentlyContinue) {
        throw 'The silent installer unexpectedly started the application.'
    }
}

function Invoke-SilentInstaller {
    param([string]$Executable, [string[]]$Arguments)
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -PassThru -Wait -WindowStyle Hidden
    if ($process.ExitCode -ne 0) {
        throw "Installer process returned exit code $($process.ExitCode): $Executable. See $logRoot."
    }
    Assert-NoGuiProcess
}

function Assert-InstalledVersion {
    $process = Start-Process -FilePath $installedExecutable -ArgumentList '--version' -PassThru -Wait -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $logRoot 'version.stdout.txt') `
        -RedirectStandardError (Join-Path $logRoot 'version.stderr.txt')
    $reportedVersion = (Get-Content -LiteralPath (Join-Path $logRoot 'version.stdout.txt') -Raw).Trim()
    if ($process.ExitCode -ne 0 -or $reportedVersion -cne "leigod-guard $version") {
        throw "Installed executable version check failed: $reportedVersion (exit $($process.ExitCode))."
    }
    Assert-NoGuiProcess
}

function Assert-StaticRuntime {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) { throw 'vswhere.exe is required for the CRT import check.' }
    $dumpbins = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe')
    if ($LASTEXITCODE -ne 0 -or $dumpbins.Count -eq 0) { throw 'Could not find the MSVC dumpbin.exe tool.' }
    $imports = & $dumpbins[0] /dependents $installedExecutable
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect the installed executable imports.' }
    $imports | Set-Content -LiteralPath (Join-Path $logRoot 'binary-imports.txt') -Encoding UTF8
    if (($imports -join "`n") -match '(?i)\b(?:vcruntime[0-9_]*|msvcp[0-9_]*|concrt[0-9_]*|msvcr1[0-9_]*)\.dll\b') {
        throw 'The distributed executable still requires a separately installed Visual C++ runtime.'
    }
}

# Refuse to modify anything that could belong to an existing installation/user.
Assert-NoGuiProcess
foreach ($existingPath in @($configRoot, $uninstallKey, $menuRoot, $desktopShortcut,
        (Join-Path $env:LOCALAPPDATA 'Programs\LeigodGuard'))) {
    if (Test-Path -LiteralPath $existingPath) { throw "Expected a clean runner, but found $existingPath." }
}
if ($null -ne (Get-StartupValue)) { throw 'Expected a clean runner without a LeigodGuard startup value.' }

Push-Location -LiteralPath $repositoryRoot
try {
    $metadataText = & cargo metadata --locked --offline --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $packages = @($metadata.packages | Where-Object { $_.name -eq 'leigod-guard' })
    if ($packages.Count -ne 1) { throw 'Expected exactly one leigod-guard package.' }
    $version = $packages[0].version
    $setupPath = Join-Path $outputRoot "leigod-guard-v$version-windows-x64-setup.exe"
    if (-not (Test-Path -LiteralPath $setupPath -PathType Leaf)) { throw "Installer missing: $setupPath" }
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($logRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($configRoot) | Out-Null
    [System.IO.File]::WriteAllText($configPath, "# Installer preservation fixture. No account or token.`n",
        [System.Text.UTF8Encoding]::new($false))
    $configHash = (Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash

    $installArguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
        '/LANG=english', '/TASKS=desktopicon', ('/DIR="' + $installRoot + '"'))
    Invoke-SilentInstaller -Executable $setupPath -Arguments ($installArguments + ('/LOG="' + (Join-Path $logRoot 'install.log') + '"'))
    foreach ($requiredFile in @($installedExecutable, $uninstaller, $menuShortcut, $desktopShortcut,
            (Join-Path $installRoot 'README.md'), (Join-Path $installRoot 'LICENSE'),
            (Join-Path $installRoot 'THIRD_PARTY_NOTICES.txt'))) {
        if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) { throw "Missing installed file: $requiredFile" }
    }
    $registration = Get-ItemProperty -LiteralPath $uninstallKey
    if ($registration.DisplayVersion -cne $version -or
        $registration.InstallLocation.TrimEnd('\') -ine $installRoot.TrimEnd('\')) {
        throw 'The uninstall registration has an incorrect version or installation directory.'
    }
    $shell = New-Object -ComObject WScript.Shell
    foreach ($shortcutPath in @($menuShortcut, $desktopShortcut)) {
        if ($shell.CreateShortcut($shortcutPath).TargetPath -ine $installedExecutable) {
            throw "Shortcut points to an unexpected executable: $shortcutPath"
        }
    }
    Assert-InstalledVersion
    Assert-StaticRuntime
    if ($null -ne (Get-StartupValue)) { throw 'A fresh installation unexpectedly enabled startup.' }

    # A running instance must cause silent setup to fail before replacing files.
    # The real program uses the same mutex; no GUI/account is needed for this test.
    $installedHash = (Get-FileHash -LiteralPath $installedExecutable -Algorithm SHA256).Hash
    $installationMutex = [System.Threading.Mutex]::new($true, 'Local\LeigodGuard')
    try {
        $blockedProcess = Start-Process -FilePath $setupPath -ArgumentList ($installArguments +
            ('/LOG="' + (Join-Path $logRoot 'blocked-upgrade.log') + '"')) -PassThru -Wait -WindowStyle Hidden
        if ($blockedProcess.ExitCode -eq 0) { throw 'Setup unexpectedly succeeded while the app mutex was held.' }
        if ((Get-FileHash -LiteralPath $installedExecutable -Algorithm SHA256).Hash -cne $installedHash) {
            throw 'A blocked upgrade replaced the installed executable.'
        }
        Assert-NoGuiProcess
    } finally {
        $installationMutex.ReleaseMutex()
        $installationMutex.Dispose()
    }

    # Migrate an enabled startup preference from an older portable location.
    $oldPortableRoot = Join-Path $testRoot 'old portable'
    [System.IO.Directory]::CreateDirectory($oldPortableRoot) | Out-Null
    $oldPortableExecutable = Join-Path $oldPortableRoot 'leigod-guard.exe'
    Copy-Item -LiteralPath $installedExecutable -Destination $oldPortableExecutable
    $oldPortableStartup = '"' + $oldPortableExecutable + '" --minimized'
    if (-not (Test-Path -LiteralPath $runKey)) { New-Item -Path $runKey | Out-Null }
    New-ItemProperty -LiteralPath $runKey -Name 'LeigodGuard' -Value $oldPortableStartup -PropertyType String -Force | Out-Null
    Invoke-SilentInstaller -Executable $setupPath -Arguments ($installArguments + ('/LOG="' + (Join-Path $logRoot 'upgrade.log') + '"'))
    Assert-InstalledVersion
    if ((Get-StartupValue) -cne $ownedStartup) { throw 'Upgrade did not migrate the enabled startup preference from the portable executable.' }
    if ((Get-FileHash -LiteralPath $oldPortableExecutable -Algorithm SHA256).Hash -cne $installedHash) {
        throw 'Upgrade unexpectedly modified the old portable executable.'
    }
    if ((Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash -cne $configHash) {
        throw 'Installation or upgrade changed the existing user configuration.'
    }

    Invoke-SilentInstaller -Executable $uninstaller -Arguments @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
        ('/LOG="' + (Join-Path $logRoot 'uninstall.log') + '"'))
    foreach ($removedPath in @($installedExecutable, $uninstallKey, $menuShortcut, $desktopShortcut)) {
        if (Test-Path -LiteralPath $removedPath) { throw "Uninstall left an application file or registration behind: $removedPath" }
    }
    if ($null -ne (Get-StartupValue)) { throw 'Uninstall did not remove its own startup entry.' }
    if ((Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash -cne $configHash) {
        throw 'Uninstall changed or removed the preserved user configuration.'
    }

    # Remove only the fixture files this test created. No recursive cleanup of an
    # app-data or installation directory is necessary on this disposable runner.
    Remove-Item -LiteralPath $configPath -Force
    Remove-Item -LiteralPath $configRoot
    Write-Host "Installer smoke test passed for v${version}: install, shortcuts, static CRT, version, mutex protection, upgrade, portable startup migration and uninstall; user configuration preserved."
} finally {
    Pop-Location
}
