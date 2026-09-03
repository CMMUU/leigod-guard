[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$packages = @{}
foreach ($target in @('x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu')) {
    $metadataJson = & cargo metadata --manifest-path (Join-Path $repoRoot 'Cargo.toml') --locked --format-version 1 --filter-platform $target
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $metadata = ($metadataJson -join "`n") | ConvertFrom-Json
    $nodes = @{}
    $byId = @{}
    foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }
    foreach ($package in $metadata.packages) { $byId[$package.id] = $package }
    $queue = [Collections.Generic.Queue[string]]::new()
    $visited = [Collections.Generic.HashSet[string]]::new()
    $queue.Enqueue($metadata.resolve.root)
    while ($queue.Count -gt 0) {
        $id = $queue.Dequeue()
        if (-not $visited.Add($id)) { continue }
        if ($id -ne $metadata.resolve.root) { $packages[$id] = $byId[$id] }
        foreach ($dependency in $nodes[$id].dependencies) { $queue.Enqueue($dependency) }
    }
}
$notice = [Text.StringBuilder]::new()
[void]$notice.AppendLine('Third-party notices for LeigodGuard (Windows x64)')
[void]$notice.AppendLine('Generated from Cargo.lock and the published dependency source packages.')
[void]$notice.AppendLine('Dependencies retain their own licenses and copyright notices; the project MIT license does not replace them.')
[void]$notice.AppendLine('This inventory includes build-time dependencies for the Windows MSVC and GNU targets.')
foreach ($package in ($packages.Values | Sort-Object name, version)) {
    [void]$notice.AppendLine("`n========================================================================")
    [void]$notice.AppendLine("$($package.name) $($package.version)")
    [void]$notice.AppendLine("Declared license: $($package.license)")
    [void]$notice.AppendLine("Source: https://crates.io/crates/$($package.name)/$($package.version)")
    if ($package.repository) { [void]$notice.AppendLine("Repository: $($package.repository)") }
    $sourceDir = Split-Path -Parent $package.manifest_path
    $licenseFiles = @(Get-ChildItem -LiteralPath $sourceDir -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|UNLICENSE)(\.|-|$)' })
    if ($package.license_file) { $licenseFiles += Get-Item -LiteralPath (Join-Path $sourceDir $package.license_file) }
    foreach ($file in ($licenseFiles | Sort-Object FullName -Unique)) {
        [void]$notice.AppendLine("`n--- $($file.Name) ---")
        [void]$notice.AppendLine([IO.File]::ReadAllText($file.FullName))
    }
    if ($licenseFiles.Count -eq 0) { Write-Warning "No root license text bundled by $($package.name) $($package.version); review its source package." }
}
$supplementRoot = Join-Path $repoRoot 'licenses'
if (Test-Path -LiteralPath $supplementRoot -PathType Container) {
    [void]$notice.AppendLine("`n========================================================================")
    [void]$notice.AppendLine('Additional upstream licenses, font licenses, SDK terms, and source provenance')
    [void]$notice.AppendLine('See licenses/README.md for package-to-license mapping and verified source versions.')
    foreach ($file in (Get-ChildItem -LiteralPath $supplementRoot -Recurse -File | Sort-Object FullName)) {
        $relative = $file.FullName.Substring($supplementRoot.Length + 1).Replace('\', '/')
        [void]$notice.AppendLine("`n--- licenses/$relative ---")
        [void]$notice.AppendLine([IO.File]::ReadAllText($file.FullName))
    }
}
[IO.File]::WriteAllText((Join-Path $repoRoot 'THIRD_PARTY_NOTICES.txt'), $notice.ToString(), [Text.UTF8Encoding]::new($false))
Write-Output "Wrote notices for $($packages.Count) dependencies."
