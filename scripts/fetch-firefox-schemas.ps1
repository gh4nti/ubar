param(
    [string]$Destination = (Join-Path $PSScriptRoot '..\vendor\firefox-schemas\src')
)

$ErrorActionPreference = 'Stop'
$Repository = 'https://github.com/mozilla-firefox/firefox.git'
$Commit = '79223b6429f6d8844435cfdbf68ad699367bfec5'
$Destination = [System.IO.Path]::GetFullPath($Destination)

if (-not (Test-Path (Join-Path $Destination '.git'))) {
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    git -C $Destination init
    git -C $Destination remote add origin $Repository
    git -C $Destination config remote.origin.promisor true
    git -C $Destination config remote.origin.partialclonefilter blob:none
    git -C $Destination sparse-checkout init --cone
    git -C $Destination sparse-checkout set browser/components/extensions/schemas toolkit/components/extensions/schemas
}

git -C $Destination fetch --depth=1 --filter=blob:none origin $Commit
git -C $Destination checkout --detach FETCH_HEAD
$Actual = (git -C $Destination rev-parse HEAD).Trim()
if ($Actual -ne $Commit) {
    throw "Firefox pin mismatch: expected $Commit, got $Actual"
}

Write-Host "Firefox WebExtensions schemas pinned at $Actual"
