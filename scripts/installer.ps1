#Requires -Version 5.1
<#
.SYNOPSIS
Build the Windows x64 installer, portable ZIP, and their SHA-256 checksums.
.DESCRIPTION
Requires an installed Inno Setup 6.4 or later compiler. The compiler is never
downloaded or installed by this script. The Microsoft WebView2 bootstrapper is
downloaded from Microsoft's distribution link unless BootstrapperPath is supplied.
Both downloaded and supplied bootstrappers must have a valid Microsoft signature.
Use -SkipBuild only after testing and building the same source and target yourself.
Use -CheckCompilerOnly to discover the compiler and verify its identity by calling
ISCC /?. It does not build, download, install, or create output directories. Inno
6 reports only its major version in help; the .iss preprocessor enforces 6.4+ at
compile time using its authoritative Ver constant.
#>
[CmdletBinding()]
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$OutputDirectory = 'dist',
    [switch]$SkipBuild,
    [string]$BootstrapperPath,
    [string]$InnoCompiler,
    [switch]$CheckCompilerOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$stagingRoot = $null

function Resolve-RepositoryPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $Path))
}

function Get-InnoCompilerProbe {
    param([string]$Path)
    # Inno 6's PE FileVersion is not the compiler engine version. Its /? handler
    # prints a recognizable banner and exits 1 (newer versions may exit 0).
    # https://github.com/jrsoftware/issrc/blob/is-6_7_1/Projects/ISCC.dpr
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Path
    $startInfo.Arguments = '/?'
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) { throw 'Could not start the compiler help command.' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) {
            $process.Kill()
            throw 'The compiler help command did not finish within 10 seconds.'
        }
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Output = $stdout.GetAwaiter().GetResult() + "`n" + $stderr.GetAwaiter().GetResult()
        }
    } finally {
        $process.Dispose()
    }
}

function Find-InnoCompiler {
    if ($InnoCompiler) {
        $candidates = @(Resolve-RepositoryPath -Path $InnoCompiler)
    } else {
        $candidates = @()
        # Prefer the installed compiler over Chocolatey's PATH launcher shim.
        foreach ($programDirectory in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
            if ($programDirectory) { $candidates += Join-Path $programDirectory 'Inno Setup 6\ISCC.exe' }
        }
        $commands = @(Get-Command ISCC.exe -CommandType Application -All -ErrorAction SilentlyContinue)
        foreach ($command in $commands) { $candidates += $command.Source }
    }
    $rejectedCandidates = @()
    foreach ($candidate in @($candidates | Select-Object -Unique)) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $reason = ''
            try {
                $probe = Get-InnoCompilerProbe -Path $candidate
                $banner = [regex]::Match($probe.Output, '(?m)^Inno Setup (?<major>[0-9]+)(?:\.[0-9]+)* Command-Line Compiler\s*$')
                if ($probe.ExitCode -notin @(0, 1) -or -not $banner.Success) {
                    $reason = "The help command did not identify a working Inno Setup compiler (exit $($probe.ExitCode)) at $candidate"
                } elseif ([int]$banner.Groups['major'].Value -lt 6) {
                    $reason = "Inno Setup $($banner.Groups['major'].Value) is too old at $candidate"
                }
            } catch {
                $reason = "Could not verify the Inno Setup compiler at ${candidate}: $($_.Exception.Message)"
            }
            if ($reason) {
                if ($InnoCompiler) {
                    throw "$reason. -InnoCompiler must point to a working Inno Setup 6.4 or later ISCC.exe."
                }
                $rejectedCandidates += $reason
                Write-Verbose "Skipping compiler candidate: $reason"
                continue
            }
            Write-Host "Inno Setup compiler: $candidate (help identity verified; exact minimum 6.4 is checked during .iss preprocessing)"
            return $candidate
        } elseif ($InnoCompiler) {
            throw "The explicitly supplied -InnoCompiler file does not exist: $candidate. Supply the real Inno Setup 6.4 or later ISCC.exe path."
        }
    }
    $details = if ($rejectedCandidates.Count -gt 0) { ' Rejected candidates: ' + ($rejectedCandidates -join '; ') } else { '' }
    throw "Inno Setup 6.4 or later was not found. Install it from https://jrsoftware.org/isinfo.php or pass -InnoCompiler with an existing real ISCC.exe path.$details"
}

function Assert-MicrosoftBootstrapper {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "WebView2 bootstrapper does not exist: $Path"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
        -not $signature.SignerCertificate) {
        throw "The WebView2 bootstrapper does not have a valid Authenticode signature: $($signature.Status)."
    }
    $publisher = $signature.SignerCertificate.GetNameInfo(
        [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false
    )
    if ($publisher -cne 'Microsoft Corporation') {
        throw "The WebView2 bootstrapper publisher must be Microsoft Corporation; found $publisher."
    }
    Write-Host "Verified WebView2 bootstrapper signature: $publisher"
    Write-Host "WebView2 bootstrapper SHA-256: $((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant())"
}

$compiler = Find-InnoCompiler
if ($CheckCompilerOnly) {
    # The help probe's exit 1 is expected on Inno 6, not a failed preflight.
    # Also clear any stale native exit code before GitHub's pwsh wrapper reads it.
    $global:LASTEXITCODE = 0
    return
}
$outputRoot = Resolve-RepositoryPath -Path $OutputDirectory
[System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null

Push-Location -LiteralPath $repositoryRoot
try {
    $metadataText = & cargo metadata --locked --offline --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $packages = @($metadata.packages | Where-Object { $_.name -eq 'leigod-guard' })
    if ($packages.Count -ne 1) { throw 'Expected exactly one leigod-guard package.' }
    $version = $packages[0].version
    if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
        throw 'The Windows installer currently requires a stable major.minor.patch Cargo version.'
    }

    # package.ps1 is the single allowlist for files distributed with the app.
    & (Join-Path $PSScriptRoot 'package.ps1') -Target $Target -OutputDirectory $outputRoot -SkipBuild:$SkipBuild
    $archiveName = "leigod-guard-v$version-windows-x64.zip"
    $archivePath = Join-Path $outputRoot $archiveName
    $setupName = "leigod-guard-v$version-windows-x64-setup.exe"

    $stagingName = '.installer-' + [guid]::NewGuid().ToString('N')
    $stagingRoot = [System.IO.Path]::GetFullPath((Join-Path $outputRoot $stagingName))
    if (Test-Path -LiteralPath $stagingRoot) { throw 'The unique installer staging directory already exists.' }
    $payloadRoot = Join-Path $stagingRoot 'payload'
    $compilerOutput = Join-Path $stagingRoot 'compiled'
    [System.IO.Directory]::CreateDirectory($payloadRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($compilerOutput) | Out-Null

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($archivePath)
    try {
        foreach ($entry in $archive.Entries) {
            $entryPath = [System.IO.Path]::GetFullPath((Join-Path $payloadRoot $entry.FullName))
            if (-not $entryPath.StartsWith($payloadRoot + [System.IO.Path]::DirectorySeparatorChar,
                    [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Unsafe ZIP entry: $($entry.FullName)"
            }
        }
    } finally {
        $archive.Dispose()
    }
    [System.IO.Compression.ZipFile]::ExtractToDirectory($archivePath, $payloadRoot)

    if ($BootstrapperPath) {
        $bootstrapper = Resolve-RepositoryPath -Path $BootstrapperPath
    } else {
        $bootstrapper = Join-Path $stagingRoot 'MicrosoftEdgeWebview2Setup.exe'
        # Microsoft documents this Evergreen Bootstrapper distribution mechanism:
        # https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution
        $previousProtocol = [System.Net.ServicePointManager]::SecurityProtocol
        try {
            [System.Net.ServicePointManager]::SecurityProtocol = $previousProtocol -bor [System.Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -UseBasicParsing -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrapper
        } finally {
            [System.Net.ServicePointManager]::SecurityProtocol = $previousProtocol
        }
    }
    Assert-MicrosoftBootstrapper -Path $bootstrapper

    $compilerArguments = @(
        '/Qp', "/DAppVersion=$version", "/DSourceDir=$payloadRoot",
        "/DSourceRoot=$repositoryRoot",
        "/DOutputDir=$compilerOutput", "/DBootstrapperFile=$bootstrapper",
        (Join-Path $repositoryRoot 'installer/leigod-guard.iss')
    )
    & $compiler @compilerArguments
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed with exit code $LASTEXITCODE." }
    $compiledSetup = Join-Path $compilerOutput $setupName
    if (-not (Test-Path -LiteralPath $compiledSetup -PathType Leaf)) {
        throw "Inno Setup did not produce the expected installer: $setupName"
    }
    $setupPath = Join-Path $outputRoot $setupName
    Move-Item -LiteralPath $compiledSetup -Destination $setupPath -Force

    $checksumLines = foreach ($assetName in @($setupName, $archiveName)) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $outputRoot $assetName) -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $assetName"
    }
    [System.IO.File]::WriteAllText((Join-Path $outputRoot 'SHA256SUMS.txt'),
        (($checksumLines -join "`n") + "`n"), [System.Text.UTF8Encoding]::new($false))
    Write-Host "Installer: $setupPath"
    Write-Host "Portable ZIP: $archivePath"
    Write-Host "Checksums: $(Join-Path $outputRoot 'SHA256SUMS.txt')"
} finally {
    if ($stagingRoot -and (Test-Path -LiteralPath $stagingRoot -PathType Container)) {
        # Only remove the fresh GUID directory created by this invocation, after
        # checking its resolved absolute parent and its exact expected name.
        $resolvedStaging = (Resolve-Path -LiteralPath $stagingRoot).ProviderPath
        $resolvedOutput = (Resolve-Path -LiteralPath $outputRoot).ProviderPath
        $expectedStaging = [System.IO.Path]::GetFullPath((Join-Path $resolvedOutput $stagingName))
        if ($resolvedStaging -cne $expectedStaging -or
            -not $resolvedStaging.StartsWith($resolvedOutput.TrimEnd('\') + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing cleanup outside the expected installer staging directory: $resolvedStaging"
        }
        Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
    }
    Pop-Location
}
