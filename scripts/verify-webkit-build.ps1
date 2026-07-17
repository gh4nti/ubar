param([Parameter(Mandatory)][string]$BuildDirectory)

$ErrorActionPreference = 'Stop'
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$StatePath = Join-Path $BuildDirectory '.ubar-source-state'
if (-not (Test-Path $StatePath)) { throw "Pinned WebKit build state missing: $StatePath" }
$Metadata = Get-Content (Join-Path $Root 'vendor\webkit\UPSTREAM.toml') -Raw
$ExpectedCommit = [regex]::Match($Metadata, '(?m)^commit = "([^"]+)"').Groups[1].Value
$ExpectedPatchset = (Get-FileHash (Join-Path $Root 'vendor\webkit\patches\series') -Algorithm SHA256).Hash.ToLowerInvariant()
$State = @{}
Get-Content $StatePath | ForEach-Object {
    $Key, $Value = $_ -split '=', 2
    $State[$Key] = $Value
}
if ($State.upstream -ne $ExpectedCommit) { throw 'WebKit build commit mismatch' }
if ($State.patchset_sha256 -ne $ExpectedPatchset) { throw 'WebKit build patchset mismatch' }
