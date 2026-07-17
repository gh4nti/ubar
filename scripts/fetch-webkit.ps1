param([string]$Destination = (Join-Path $PSScriptRoot '..\vendor\webkit\src'))

$ErrorActionPreference = 'Stop'
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Metadata = Get-Content (Join-Path $Root 'vendor\webkit\UPSTREAM.toml') -Raw
$Repository = [regex]::Match($Metadata, '(?m)^repository = "([^"]+)"').Groups[1].Value
$Commit = [regex]::Match($Metadata, '(?m)^commit = "([^"]+)"').Groups[1].Value
$Series = Join-Path $Root 'vendor\webkit\patches\series'
$Destination = [IO.Path]::GetFullPath($Destination)
if (-not $Repository -or -not $Commit) { throw 'Invalid WebKit metadata' }

if (-not (Test-Path (Join-Path $Destination '.git'))) {
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    git -C $Destination init
    git -C $Destination remote add origin $Repository
}
git -C $Destination fetch --depth=1 origin $Commit
git -C $Destination checkout --detach FETCH_HEAD
$Actual = (git -C $Destination rev-parse HEAD).Trim()
if ($Actual -ne $Commit) { throw "WebKit pin mismatch: expected $Commit, got $Actual" }

foreach ($Line in Get-Content $Series) {
    if (-not $Line.Trim() -or $Line.TrimStart().StartsWith('#')) { continue }
    $Expected, $Patch = $Line.Trim() -split '\s+', 2
    $File = Join-Path $Root "vendor\webkit\patches\$Patch"
    if (-not (Test-Path $File)) { throw "Missing WebKit patch: $Patch" }
    $ActualHash = (Get-FileHash $File -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualHash -ne $Expected) { throw "WebKit patch hash mismatch: $Patch" }
    git -C $Destination am --3way $File
}

Get-ChildItem $Destination -Recurse -File | Where-Object {
    $_.Name -like 'COPYING*' -or $_.Name -like 'LICENSE*'
} | ForEach-Object { $_.FullName.Substring($Destination.Length + 1).Replace('\', '/') } |
    Sort-Object | Set-Content (Join-Path $Destination '.ubar-license-files')
$SeriesHash = (Get-FileHash $Series -Algorithm SHA256).Hash.ToLowerInvariant()
@("upstream=$Commit", "patchset_sha256=$SeriesHash") |
    Set-Content (Join-Path $Destination '.ubar-source-state')
Write-Host "WebKit prepared at $Commit (patchset $SeriesHash)"
