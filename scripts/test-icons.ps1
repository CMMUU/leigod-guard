#Requires -Version 5.1
<#
.SYNOPSIS
Verify that the release executable and Setup contain the committed application icon.
.DESCRIPTION
Cloud-only, read-only resource inspection. It never starts the application or
installer. Windows loads each PE as a data/resource file; every image in a group
must match the corresponding image bytes in assets/app-icon.ico.
#>
[CmdletBinding()]
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$OutputDirectory = 'dist'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
    $env:RUNNER_OS -ne 'Windows' -or -not $env:RUNNER_TEMP) {
    throw 'Icon resource verification runs only on a GitHub-hosted Windows runner, not a personal or self-hosted computer.'
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$outputRoot = if ([IO.Path]::IsPathRooted($OutputDirectory)) {
    [IO.Path]::GetFullPath($OutputDirectory)
} else {
    [IO.Path]::GetFullPath((Join-Path $repositoryRoot $OutputDirectory))
}
$iconPath = Join-Path $repositoryRoot 'assets\app-icon.ico'

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

public static class LeigodIconResources
{
    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate bool EnumResourceName(IntPtr module, IntPtr type, IntPtr name, IntPtr parameter);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern IntPtr LoadLibraryExW(string file, IntPtr reserved, uint flags);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool FreeLibrary(IntPtr module);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern bool EnumResourceNamesW(IntPtr module, IntPtr type, EnumResourceName callback, IntPtr parameter);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern IntPtr FindResourceW(IntPtr module, IntPtr name, IntPtr type);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint SizeofResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LockResource(IntPtr resource);

    private static string FrameKey(int width, int height, byte[] bytes)
    {
        using (var sha = SHA256.Create())
            return width + "x" + height + ":" + BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "");
    }

    private static byte[] ReadResource(IntPtr module, IntPtr name, int type)
    {
        IntPtr resource = FindResourceW(module, name, new IntPtr(type));
        if (resource == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "Icon resource was not found");
        uint length = SizeofResource(module, resource);
        if (length == 0 || length > 64 * 1024 * 1024) throw new InvalidDataException("Icon resource has an invalid size");
        IntPtr loaded = LoadResource(module, resource);
        if (loaded == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not read icon resource");
        IntPtr data = LockResource(loaded);
        if (data == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not access icon resource");
        var bytes = new byte[(int)length];
        Marshal.Copy(data, bytes, 0, bytes.Length);
        return bytes;
    }

    private static string[] ExpectedFrames(string path)
    {
        byte[] file = File.ReadAllBytes(path);
        if (file.Length < 6 || BitConverter.ToUInt16(file, 0) != 0 || BitConverter.ToUInt16(file, 2) != 1)
            throw new InvalidDataException("Application icon is not an ICO file");
        int count = BitConverter.ToUInt16(file, 4);
        if (count == 0 || count > 64 || file.Length < 6 + count * 16)
            throw new InvalidDataException("Application icon directory is invalid");
        var frames = new List<string>();
        var sizes = new HashSet<int>();
        for (int i = 0; i < count; i++)
        {
            int entry = 6 + i * 16;
            int width = file[entry] == 0 ? 256 : file[entry];
            int height = file[entry + 1] == 0 ? 256 : file[entry + 1];
            uint length = BitConverter.ToUInt32(file, entry + 8);
            uint offset = BitConverter.ToUInt32(file, entry + 12);
            if (width != height || length == 0 || offset < 6 + count * 16 || (ulong)offset + length > (ulong)file.Length)
                throw new InvalidDataException("Application icon contains an invalid image entry");
            var image = new byte[(int)length];
            Buffer.BlockCopy(file, (int)offset, image, 0, image.Length);
            frames.Add(FrameKey(width, height, image));
            sizes.Add(width);
        }
        foreach (int required in new[] { 16, 32, 48, 64, 256 })
            if (!sizes.Contains(required)) throw new InvalidDataException("Application icon is missing the " + required + "px image");
        return frames.OrderBy(value => value, StringComparer.Ordinal).ToArray();
    }

    public static int AssertMatches(string executable, string icon)
    {
        string[] expected = ExpectedFrames(icon);
        // LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE: the PE is
        // inspected as data; imports and entry points are never executed.
        IntPtr module = LoadLibraryExW(executable, IntPtr.Zero, 0x00000002 | 0x00000020);
        if (module == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not open PE resources: " + executable);
        try
        {
            int groups = 0;
            bool matched = false;
            Exception failure = null;
            EnumResourceName callback = (library, type, name, parameter) =>
            {
                try
                {
                    byte[] group = ReadResource(library, name, 14); // RT_GROUP_ICON
                    if (group.Length < 6 || BitConverter.ToUInt16(group, 0) != 0 || BitConverter.ToUInt16(group, 2) != 1)
                        throw new InvalidDataException("Invalid PE icon group");
                    int count = BitConverter.ToUInt16(group, 4);
                    if (count == 0 || count > 64 || group.Length < 6 + count * 14)
                        throw new InvalidDataException("Invalid PE icon group entries");
                    var frames = new List<string>();
                    for (int i = 0; i < count; i++)
                    {
                        int entry = 6 + i * 14;
                        int width = group[entry] == 0 ? 256 : group[entry];
                        int height = group[entry + 1] == 0 ? 256 : group[entry + 1];
                        uint length = BitConverter.ToUInt32(group, entry + 8);
                        int id = BitConverter.ToUInt16(group, entry + 12);
                        byte[] image = ReadResource(library, new IntPtr(id), 3); // RT_ICON
                        if (length != image.Length) throw new InvalidDataException("PE icon image size differs from its group directory");
                        frames.Add(FrameKey(width, height, image));
                    }
                    groups++;
                    if (frames.OrderBy(value => value, StringComparer.Ordinal).SequenceEqual(expected)) matched = true;
                    return true;
                }
                catch (Exception error)
                {
                    failure = error;
                    return false;
                }
            };
            bool enumerated = EnumResourceNamesW(module, new IntPtr(14), callback, IntPtr.Zero);
            GC.KeepAlive(callback);
            if (failure != null) throw failure;
            if (!enumerated || groups == 0) throw new InvalidDataException("No readable PE icon group was found in " + executable);
            if (!matched) throw new InvalidDataException("PE icon images do not match the committed application icon: " + executable);
            return expected.Length;
        }
        finally
        {
            FreeLibrary(module);
        }
    }
}
'@

Push-Location -LiteralPath $repositoryRoot
try {
    $metadataText = & cargo metadata --locked --offline --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read Cargo metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $packages = @($metadata.packages | Where-Object { $_.name -eq 'leigod-guard' })
    if ($packages.Count -ne 1) { throw 'Expected exactly one leigod-guard package.' }
    $version = $packages[0].version
    $executable = Join-Path $metadata.target_directory "$Target\release\leigod-guard.exe"
    $setup = Join-Path $outputRoot "leigod-guard-v$version-windows-x64-setup.exe"
    $zip = Join-Path $outputRoot "leigod-guard-v$version-windows-x64.zip"
    foreach ($requiredPath in @($iconPath, $executable, $setup, $zip)) {
        if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) { throw "Icon verification input is missing: $requiredPath" }
    }
    foreach ($candidate in @($executable, $setup)) {
        $count = [LeigodIconResources]::AssertMatches($candidate, $iconPath)
        Write-Host "Verified $count matching icon images in $([IO.Path]::GetFileName($candidate))"
    }

    # Inspect the executable actually distributed in the portable asset too.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    try {
        $entries = @($archive.Entries | Where-Object { $_.FullName -ceq 'leigod-guard.exe' })
        if ($entries.Count -ne 1) { throw 'The portable package must contain one root executable.' }
        $temporaryDirectory = Join-Path $env:RUNNER_TEMP ('leigod-icon-test-' + [guid]::NewGuid().ToString('N'))
        [IO.Directory]::CreateDirectory($temporaryDirectory) | Out-Null
        $portableExecutable = Join-Path $temporaryDirectory 'leigod-guard.exe'
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0], $portableExecutable)
        $count = [LeigodIconResources]::AssertMatches($portableExecutable, $iconPath)
        if ((Get-FileHash -LiteralPath $portableExecutable -Algorithm SHA256).Hash -cne
            (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash) {
            throw 'The portable executable differs from the verified release build.'
        }
        Write-Host "Verified $count matching icon images in the portable ZIP executable"
    } finally {
        $archive.Dispose()
    }
} finally {
    Pop-Location
}
