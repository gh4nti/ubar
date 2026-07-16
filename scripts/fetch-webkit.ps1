param(
    [string]$Destination = (Join-Path $PSScriptRoot '..\vendor\webkit\src')
)

$ErrorActionPreference = 'Stop'
$Repository = 'https://github.com/WebKit/WebKit.git'
$Commit = '3af9d4073d75a9b44668cebc03a41223dbef5745'
$Destination = [System.IO.Path]::GetFullPath($Destination)

if (-not (Test-Path (Join-Path $Destination '.git'))) {
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    git -C $Destination init
    git -C $Destination remote add origin $Repository
}

git -C $Destination fetch --depth=1 origin $Commit
git -C $Destination checkout --detach FETCH_HEAD
$Actual = (git -C $Destination rev-parse HEAD).Trim()
if ($Actual -ne $Commit) {
    throw "WebKit pin mismatch: expected $Commit, got $Actual"
}

Write-Host "WebKit pinned at $Actual"
