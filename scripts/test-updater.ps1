#Requires -Version 5.1
<#
.SYNOPSIS
Exercise both update modes through the real --apply-update process on a clean
GitHub-hosted Windows runner. Never runs the real account/monitoring application.
.DESCRIPTION
The next-version executable is a tiny Rust fixture: --version reports its version,
normal startup writes a marker and exits. A separate fixture process exits only
when signaled. Real release helpers therefore perform their production path/hash
checks, parent-process wait, file replacement/Setup upgrade and automatic restart.
All fixture executables/packages remain under RUNNER_TEMP, never release assets.
#>
[CmdletBinding()]
param([string]$OutputDirectory = 'dist')

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
    $env:RUNNER_OS -ne 'Windows' -or -not $env:RUNNER_TEMP) {
    throw 'Updater integration tests require a fresh GitHub-hosted Windows runner. Never run them on a personal or self-hosted machine.'
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$outputRoot = if ([IO.Path]::IsPathRooted($OutputDirectory)) {
    [IO.Path]::GetFullPath($OutputDirectory)
} else { [IO.Path]::GetFullPath((Join-Path $repositoryRoot $OutputDirectory)) }
$testRoot = Join-Path $env:RUNNER_TEMP ('leigod-updater-fixture-' + [guid]::NewGuid().ToString('N'))
$logRoot = Join-Path $outputRoot 'updater-test-logs'
$updatesRoot = Join-Path $env:LOCALAPPDATA 'LeigodGuard\updates'
$configRoot = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'leigod-guard'
$configPath = Join-Path $configRoot 'config.toml'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{6ABF5F53-AFD4-4D24-A930-174CF6B21B7A}_is1'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$menuRoot = Join-Path ([Environment]::GetFolderPath('Programs')) 'LeigodGuard'
$desktopShortcut = Join-Path ([Environment]::GetFolderPath('Desktop')) 'LeigodGuard.lnk'
$utf8 = [Text.UTF8Encoding]::new($false)
$ownedStartup = $null
$createdConfig = $false
$installedRoot = $null

function Assert-Within {
    param([string]$Path, [string]$Root)
    $absolute = [IO.Path]::GetFullPath($Path)
    $prefix = [IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
    if (-not $absolute.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Fixture path is outside its owned directory: $absolute"
    }
    return $absolute
}

function Get-StartupValue {
    if (-not (Test-Path -LiteralPath $runKey)) { return $null }
    $key = Get-Item -LiteralPath $runKey
    try {
        return $key.GetValue('LeigodGuard', $null,
            [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    } finally { $key.Dispose() }
}

function Get-Sha256 {
    param([string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Write-Utf8 {
    param([string]$Path, [string]$Text)
    [IO.File]::WriteAllText($Path, $Text, $utf8)
}

function Invoke-Installer {
    param([string]$Executable, [string[]]$Arguments)
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(180000)) { throw "Fixture installer timed out: $Executable" }
    if ($process.ExitCode -ne 0) { throw "Fixture installer failed with exit code $($process.ExitCode): $Executable" }
}

function Wait-FixtureFile {
    param([string]$Path, [string]$Stage, [int]$TimeoutSeconds = 60)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        $errorFile = Join-Path $Stage 'error.txt'
        if (Test-Path -LiteralPath $errorFile -PathType Leaf) {
            throw ('The real update helper rejected the fixture: ' + (Get-Content -LiteralPath $errorFile -Raw))
        }
        if ([DateTime]::UtcNow -gt $deadline) { throw "Timed out waiting for $Path" }
        Start-Sleep -Milliseconds 100
    }
}

function Test-UpdateMode {
    param([ValidateSet('Portable', 'Installer')][string]$Kind, [string]$Destination, [string]$Artifact)
    $destination = Assert-Within -Path $Destination -Root $testRoot
    $stage = Join-Path $updatesRoot ('update-' + [guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $stage) { throw 'Unexpected fixture staging collision.' }
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    $stage = Assert-Within -Path $stage -Root $updatesRoot
    $modeLogs = Join-Path $logRoot $Kind.ToLowerInvariant()
    [IO.Directory]::CreateDirectory($modeLogs) | Out-Null
    $original = Join-Path $destination 'leigod-guard.exe'
    $originalHash = Get-Sha256 $original
    $artifactPath = Join-Path $stage ([IO.Path]::GetFileName($Artifact))
    Copy-Item -LiteralPath $Artifact -Destination $artifactPath
    $helperPath = Join-Path $stage 'update-helper.exe'
    Copy-Item -LiteralPath $original -Destination $helperPath
    Write-Utf8 -Path (Join-Path $destination 'config.toml') -Text '# harmless per-directory fixture; preserve this'
    Write-Utf8 -Path (Join-Path $destination 'my-notes.txt') -Text 'an unrelated user file'
    $localConfigHash = Get-Sha256 (Join-Path $destination 'config.toml')
    $notesHash = Get-Sha256 (Join-Path $destination 'my-notes.txt')
    $script:ownedStartup = '"' + $original + '" --minimized'
    if (-not (Test-Path -LiteralPath $runKey)) { New-Item -Path $runKey | Out-Null }
    New-ItemProperty -Path $runKey -Name 'LeigodGuard' -Value $script:ownedStartup -PropertyType String -Force | Out-Null

    $exitFile = Join-Path $stage 'parent-exit'
    $parent = Start-Process -FilePath $fixtureExecutable -ArgumentList @('--wait', ('"' + $exitFile + '"')) -PassThru -WindowStyle Hidden
    $helper = $null
    try {
        # .NET's StartTime comes from GetProcessTimes and preserves FILETIME ticks.
        $parentCreated = [uint64]$parent.StartTime.ToUniversalTime().ToFileTimeUtc()
        $plan = [ordered]@{
            schema = 1
            kind = $Kind
            version = $nextVersion
            previous_version = $version
            parent_pid = [uint32]$parent.Id
            parent_created = $parentCreated
            destination = '\\?\' + $destination
            artifact = [IO.Path]::GetFileName($artifactPath)
            artifact_size = [uint64](Get-Item -LiteralPath $artifactPath).Length
            sha256 = Get-Sha256 $artifactPath
            previous_sha256 = $originalHash
        }
        $planPath = Join-Path $stage 'plan.json'
        Write-Utf8 -Path $planPath -Text ($plan | ConvertTo-Json)
        $helper = Start-Process -FilePath $helperPath -ArgumentList @('--apply-update', ('"' + $planPath + '"')) `
            -WorkingDirectory $stage -PassThru -WindowStyle Hidden `
            -RedirectStandardOutput (Join-Path $modeLogs 'helper.stdout.txt') `
            -RedirectStandardError (Join-Path $modeLogs 'helper.stderr.txt')
        Wait-FixtureFile -Path (Join-Path $stage 'ready') -Stage $stage
        if ($parent.HasExited) { throw 'The fixture parent unexpectedly exited before handshake.' }
        if ((Get-Sha256 $original) -ne $originalHash) { throw 'The helper wrote the old executable before the parent exited.' }
        Write-Utf8 -Path (Join-Path $stage 'proceed') -Text "apply`n"
        Start-Sleep -Milliseconds 600
        if ((Get-Sha256 $original) -ne $originalHash) { throw 'The helper did not wait for its parent to exit.' }
        Write-Utf8 -Path $exitFile -Text 'exit normally'
        if (-not $parent.WaitForExit(10000) -or $parent.ExitCode -ne 0) { throw 'The fixture parent failed to exit normally.' }
        Wait-FixtureFile -Path (Join-Path $stage 'complete.txt') -Stage $stage -TimeoutSeconds 180
        Wait-FixtureFile -Path (Join-Path $destination 'restart-marker.txt') -Stage $stage
        if (-not $helper.WaitForExit(10000) -or $helper.ExitCode -ne 0) { throw 'The real helper did not exit successfully.' }
        Start-Sleep -Milliseconds 100
        if ((Get-Content -LiteralPath (Join-Path $destination 'restart-marker.txt') -Raw).Trim() -cne $nextVersion) {
            throw 'The replacement program was not restarted with its expected version.'
        }
        if ((Get-Sha256 $original) -ne (Get-Sha256 $fixtureExecutable)) { throw 'The target executable is not the verified next-version payload.' }
        if ((Get-Sha256 (Join-Path $destination 'config.toml')) -ne $localConfigHash -or
            (Get-Sha256 (Join-Path $destination 'my-notes.txt')) -ne $notesHash -or
            (Get-Sha256 $configPath) -ne $script:configHash) {
            throw 'An update changed a fixture configuration or unrelated user file.'
        }
        if ((Get-StartupValue) -cne $script:ownedStartup) { throw 'An update changed the existing startup preference/path.' }
        if ($Kind -eq 'Portable') {
            $backups = @(Get-ChildItem -LiteralPath $destination -Directory -Filter '.leigod-update-*')
            if ($backups.Count -ne 1 -or
                (Get-Sha256 (Join-Path $backups[0].FullName 'backup\leigod-guard.exe')) -ne $originalHash) {
                throw 'Portable update did not preserve the old executable in its recovery directory.'
            }
            if (Test-Path -LiteralPath $uninstallKey) { throw 'Portable update unexpectedly created an installer registration.' }
        } else {
            $registration = Get-ItemProperty -LiteralPath $uninstallKey
            if ($registration.DisplayVersion -cne $nextVersion -or
                [IO.Path]::GetFullPath($registration.InstallLocation).TrimEnd('\') -ine $destination.TrimEnd('\')) {
                throw 'Installer update did not keep its mode, location, and next-version registration.'
            }
        }
        Write-Utf8 -Path (Join-Path $modeLogs 'result.txt') -Text "$Kind real helper passed: parent identity/wait, package verification, same-mode replacement, preserved data/startup, automatic restart ($version -> $nextVersion)."
        Write-Host "$Kind real update helper integration passed ($version -> $nextVersion)."
    } finally {
        # The parent is our purpose-built fixture and exits in response to this file.
        if (-not $parent.HasExited) {
            Write-Utf8 -Path $exitFile -Text 'exit normally after test'
            $null = $parent.WaitForExit(10000)
        }
        foreach ($name in @('plan.json', 'error.txt', 'setup.log', 'complete.txt', 'recovery-path.txt')) {
            $candidate = Join-Path $stage $name
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                Copy-Item -LiteralPath $candidate -Destination (Join-Path $modeLogs $name)
            }
        }
        # A failed helper may be waiting at its error dialog. It is our uniquely
        # tracked fixture child on an ephemeral runner, never a real user process.
        if ($null -ne $helper -and -not $helper.HasExited) {
            $helper.Kill()
            $null = $helper.WaitForExit(10000)
        }
    }
}

if (Get-Process -Name 'leigod-guard', 'update-helper' -ErrorAction SilentlyContinue) {
    throw 'Expected a runner with no LeigodGuard application or update helper running.'
}
foreach ($existing in @($configRoot, $uninstallKey, $updatesRoot, $menuRoot, $desktopShortcut,
        (Join-Path $env:LOCALAPPDATA 'Programs\LeigodGuard'))) {
    if (Test-Path -LiteralPath $existing) { throw "Expected a clean runner, but found $existing." }
}
if ($null -ne (Get-StartupValue)) { throw 'Expected a clean runner without a LeigodGuard startup preference.' }
$testRoot = Assert-Within -Path $testRoot -Root $env:RUNNER_TEMP
[IO.Directory]::CreateDirectory($testRoot) | Out-Null
[IO.Directory]::CreateDirectory($logRoot) | Out-Null
[IO.Directory]::CreateDirectory($configRoot) | Out-Null
$createdConfig = $true
Write-Utf8 -Path $configPath -Text '# updater integration fixture only; no account, token, or password'
$script:configHash = Get-Sha256 $configPath

Push-Location -LiteralPath $repositoryRoot
try {
    $metadataText = & cargo metadata --locked --offline --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $version = @($metadata.packages | Where-Object { $_.name -eq 'leigod-guard' })[0].version
    if ($version -notmatch '^([0-9]+)\.([0-9]+)\.([0-9]+)$') { throw 'Expected a stable release version.' }
    $nextVersion = "$($Matches[1]).$($Matches[2]).$([int]$Matches[3] + 1)"
    $releaseZip = Join-Path $outputRoot "leigod-guard-v$version-windows-x64.zip"
    $releaseSetup = Join-Path $outputRoot "leigod-guard-v$version-windows-x64-setup.exe"
    foreach ($artifact in @($releaseZip, $releaseSetup)) {
        if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) { throw "Missing real release artifact: $artifact" }
    }
    $payload = Join-Path $testRoot 'next-version payload'
    $seed = Join-Path $testRoot 'original payload'
    Expand-Archive -LiteralPath $releaseZip -DestinationPath $seed
    Expand-Archive -LiteralPath $releaseZip -DestinationPath $payload
    $fixtureSource = Join-Path $testRoot 'fixture.rs'
    $fixtureExecutable = Join-Path $testRoot 'fixture-process.exe'
    $source = @'
use std::{env, fs, thread, time::Duration};
fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("leigod-guard __NEXT_VERSION__");
    } else if args.get(1).map(String::as_str) == Some("--wait") {
        let exit = std::path::Path::new(&args[2]);
        while !exit.is_file() { thread::sleep(Duration::from_millis(50)); }
    } else {
        let exe = env::current_exe().unwrap();
        fs::write(exe.parent().unwrap().join("restart-marker.txt"), "__NEXT_VERSION__").unwrap();
    }
}
'@
    Write-Utf8 -Path $fixtureSource -Text $source.Replace('__NEXT_VERSION__', $nextVersion)
    & rustc --edition 2021 --crate-name leigod_update_fixture --target x86_64-pc-windows-msvc `
        -C target-feature=+crt-static -o $fixtureExecutable $fixtureSource
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile the inert next-version/parent fixture.' }
    Copy-Item -LiteralPath $fixtureExecutable -Destination (Join-Path $payload 'leigod-guard.exe') -Force
    $nextZip = Join-Path $testRoot "leigod-guard-v$nextVersion-windows-x64.zip"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($payload, $nextZip, [IO.Compression.CompressionLevel]::Optimal, $false)
    $portable = Join-Path $testRoot '绿色 便携 app'
    Expand-Archive -LiteralPath $releaseZip -DestinationPath $portable
    Test-UpdateMode -Kind Portable -Destination $portable -Artifact $nextZip
    if ((Get-StartupValue) -ceq $ownedStartup) {
        Remove-ItemProperty -LiteralPath $runKey -Name 'LeigodGuard'
        $ownedStartup = $null
    }

    # Compile the unchanged production installer script against the inert newer
    # executable. Existing runner prerequisites are used; no compiler is installed.
    $compiler = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if (-not $compiler) { throw 'The runner must provide the actual Inno Setup 6 compiler.' }
    $bootstrapper = Join-Path $testRoot 'MicrosoftEdgeWebview2Setup.exe'
    Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrapper -UseBasicParsing
    $signature = Get-AuthenticodeSignature -LiteralPath $bootstrapper
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
        -not $signature.SignerCertificate -or
        $signature.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false) -cne 'Microsoft Corporation') {
        throw 'The fixture Setup prerequisite must have a valid Microsoft Corporation signature.'
    }
    $fixtureBuild = Join-Path $testRoot 'fixture setup only'
    [IO.Directory]::CreateDirectory($fixtureBuild) | Out-Null
    & $compiler /Qp "/DAppVersion=$nextVersion" "/DSourceDir=$payload" "/DOutputDir=$fixtureBuild" `
        "/DBootstrapperFile=$bootstrapper" (Join-Path $repositoryRoot 'installer\leigod-guard.iss') *> (Join-Path $logRoot 'fixture-iscc.log')
    if ($LASTEXITCODE -ne 0) { throw 'Could not build the next-version fixture with the production installer.' }
    $nextSetup = Join-Path $fixtureBuild "leigod-guard-v$nextVersion-windows-x64-setup.exe"
    $installedRoot = Assert-Within -Path (Join-Path $testRoot '安装 app with spaces') -Root $testRoot
    Invoke-Installer -Executable $releaseSetup -Arguments @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
        '/LANG=english', '/TASKS=desktopicon', ('/DIR="' + $installedRoot + '"'), ('/LOG="' + (Join-Path $logRoot 'initial-setup.log') + '"'))
    Test-UpdateMode -Kind Installer -Destination $installedRoot -Artifact $nextSetup
    Invoke-Installer -Executable (Join-Path $installedRoot 'unins000.exe') -Arguments @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
        ('/LOG="' + (Join-Path $logRoot 'fixture-uninstall.log') + '"'))
    if (Test-Path -LiteralPath $uninstallKey) { throw 'The updated fixture failed to uninstall its registration.' }
    if ($null -ne (Get-StartupValue)) { throw 'Fixture uninstall failed to remove its own startup registration.' }
    if ((Get-Sha256 $configPath) -ne $script:configHash) { throw 'The fixture uninstaller changed the preserved account-directory fixture.' }
    $ownedStartup = $null
    $installedRoot = $null
    Write-Host 'Both production update helper modes passed with inert next-version executables; no real GUI or account was opened.'
} finally {
    Pop-Location
    if ($null -ne $ownedStartup -and (Get-StartupValue) -ceq $ownedStartup) {
        Remove-ItemProperty -LiteralPath $runKey -Name 'LeigodGuard'
    }
    if ($createdConfig -and (Test-Path -LiteralPath $configPath -PathType Leaf) -and
        (Get-Sha256 $configPath) -ceq $script:configHash) {
        Remove-Item -LiteralPath $configPath
        if (@(Get-ChildItem -LiteralPath $configRoot -Force).Count -eq 0) {
            Remove-Item -LiteralPath $configRoot
        }
    }
    # On failures, leave the unique fixture directories/registration for runner
    # disposal. Never recursively delete a computed installation or cache path.
}
