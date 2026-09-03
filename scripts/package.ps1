#Requires -Version 5.1
<#
.SYNOPSIS
Build, test, and package the Windows x64 portable release.
.DESCRIPTION
Run from any directory. Relative output paths are resolved from the repository root.
Use -SkipBuild only after testing and building the same source and target yourself.
Cargo metadata supplies the package version and honors CARGO_TARGET_DIR.
#>
[CmdletBinding()]
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$OutputDirectory = 'dist',
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$temporaryZip = $null

function Invoke-CheckedCargo {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Find-WebViewLoader {
    param([string]$ProfileDirectory)
    $buildDirectory = Join-Path $ProfileDirectory 'build'
    $loaders = @()
    if (Test-Path -LiteralPath $buildDirectory -PathType Container) {
        $buildFolders = Get-ChildItem -LiteralPath $buildDirectory -Directory -Filter 'webview2-com-sys-*'
        foreach ($folder in $buildFolders) {
            $candidate = Join-Path $folder.FullName 'out\x64\WebView2Loader.dll'
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $loaders += Get-Item -LiteralPath $candidate
            }
        }
    }
    if ($loaders.Count -eq 0) {
        throw "GNU build requires WebView2Loader.dll under $buildDirectory."
    }
    $loaderHashes = @($loaders | ForEach-Object {
        (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    } | Select-Object -Unique)
    if ($loaderHashes.Count -ne 1) {
        throw 'Multiple different WebView2 loaders exist. Rebuild in a clean CARGO_TARGET_DIR.'
    }
    return $loaders[0].FullName
}

Push-Location -LiteralPath $repositoryRoot
try {
    $metadataText = & cargo metadata --locked --offline --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $packages = @($metadata.packages | Where-Object { $_.name -eq 'leigod-guard' })
    if ($packages.Count -ne 1) { throw 'Expected exactly one leigod-guard package.' }
    $version = $packages[0].version
    $archiveName = "leigod-guard-v$version-windows-x64.zip"
    $releaseDirectory = Join-Path $metadata.target_directory "$Target\release"
    $executable = Join-Path $releaseDirectory 'leigod-guard.exe'

    $documents = @(
        'README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.txt', 'CHANGELOG.md',
        'config.example.toml', 'docs/PRIVACY.md', 'docs/RELEASING.md'
    )
    foreach ($relativePath in $documents) {
        if (-not (Test-Path -LiteralPath (Join-Path $repositoryRoot $relativePath) -PathType Leaf)) {
            throw "Required release document is missing: $relativePath"
        }
    }
    $licenseRoot = Join-Path $repositoryRoot 'licenses'
    if (-not (Test-Path -LiteralPath $licenseRoot -PathType Container)) {
        throw 'Required release license directory is missing: licenses'
    }
    $licenseFiles = @(Get-ChildItem -LiteralPath $licenseRoot -Recurse -File)
    if ($licenseFiles.Count -eq 0) { throw 'The release license directory is empty.' }

    if (-not $SkipBuild) {
        if ($Target -eq 'x86_64-pc-windows-gnu') {
            # GNU test executables also need the loader before Rust can run any test.
            Invoke-CheckedCargo -CargoArguments @('test', '--no-run', '--locked', '--target', $Target)
            $debugDirectory = Join-Path $metadata.target_directory "$Target\debug"
            $testLoaderDirectory = Split-Path -Parent (Find-WebViewLoader -ProfileDirectory $debugDirectory)
            $previousPath = $env:PATH
            try {
                $env:PATH = $testLoaderDirectory + [System.IO.Path]::PathSeparator + $previousPath
                Invoke-CheckedCargo -CargoArguments @('test', '--locked', '--target', $Target)
            } finally {
                $env:PATH = $previousPath
            }
        } else {
            Invoke-CheckedCargo -CargoArguments @('test', '--locked', '--target', $Target)
        }
        Invoke-CheckedCargo -CargoArguments @('build', '--release', '--locked', '--target', $Target)
    }
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Release executable not found: $executable. Build the requested target first."
    }

    # Only these explicitly selected files enter the archive. Never include a user's
    # config.toml, logs, credentials, screenshots, or the complete working directory.
    $entries = @([pscustomobject]@{ Source = $executable; Name = 'leigod-guard.exe' })
    foreach ($relativePath in $documents) {
        $entries += [pscustomobject]@{
            Source = Join-Path $repositoryRoot $relativePath
            Name = $relativePath
        }
    }
    # This directory contains the audited, checked-in license texts only.
    foreach ($licenseFile in $licenseFiles) {
        $relativeName = $licenseFile.FullName.Substring($licenseRoot.Length + 1).Replace('\', '/')
        $entries += [pscustomobject]@{
            Source = $licenseFile.FullName
            Name = 'licenses/' + $relativeName
        }
    }

    if ($Target -eq 'x86_64-pc-windows-gnu') {
        # webview2-com-sys uses a DLL with GNU and a static loader with MSVC.
        $entries += [pscustomobject]@{
            Source = Find-WebViewLoader -ProfileDirectory $releaseDirectory
            Name = 'WebView2Loader.dll'
        }
    }

    if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
        $outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
    } else {
        $outputRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $OutputDirectory))
    }
    [System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null
    $archivePath = Join-Path $outputRoot $archiveName
    $temporaryZip = Join-Path $outputRoot ('.package-' + [guid]::NewGuid().ToString('N') + '.zip')
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::Open($temporaryZip, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($entry in $entries) {
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive, $entry.Source, $entry.Name, [System.IO.Compression.CompressionLevel]::Optimal
            ) | Out-Null
        }
    } finally {
        $archive.Dispose()
    }
    Move-Item -LiteralPath $temporaryZip -Destination $archivePath -Force
    $temporaryZip = $null
    $hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $checksumPath = Join-Path $outputRoot 'SHA256SUMS.txt'
    [System.IO.File]::WriteAllText($checksumPath, "$hash  $archiveName`n", [System.Text.UTF8Encoding]::new($false))
    Write-Host "Packaged leigod-guard $version ($Target)"
    Write-Host "ZIP: $archivePath"
    Write-Host "SHA-256: $hash"
    Write-Host "Checksums: $checksumPath"
} finally {
    # The only cleanup target is the unique temporary file created above.
    if ($temporaryZip -and (Test-Path -LiteralPath $temporaryZip -PathType Leaf)) {
        Remove-Item -LiteralPath $temporaryZip -Force
    }
    Pop-Location
}
