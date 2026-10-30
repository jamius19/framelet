# Author: Jamius Siam
# Since: 31/10/2026
#
# Builds release binaries and creates dist\Framelet-<version>-win64.zip containing
# framelet.exe, framelet-settings.exe and ffmpeg.exe.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-FfmpegPath C:\path\to\ffmpeg.exe]
param([string]$FfmpegPath)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
if (-not $FfmpegPath) {
    $cmd = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    if (-not $cmd) { throw "ffmpeg.exe not found on PATH; pass -FfmpegPath" }
    $FfmpegPath = $cmd.Source
}

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$stage = Join-Path $root "dist\Framelet"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\framelet.exe, target\release\framelet-settings.exe, $FfmpegPath, README.md $stage

$zip = Join-Path $root "dist\Framelet-$version-win64.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Write-Host "Created $zip"
