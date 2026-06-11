param(
    [string]$Profile = "lgpl"
)

$ErrorActionPreference = "Stop"

$distDir = Join-Path $PSScriptRoot "..\third_party\dist\$Profile\ffmpeg"
$includeDir = Join-Path $distDir "include"
$libDir = Join-Path $distDir "lib"

if ((Test-Path $includeDir) -and (Test-Path $libDir)) {
    Write-Host "FFmpeg already exists at $distDir" -ForegroundColor Green
    exit 0
}

Write-Host "Downloading FFmpeg dev and shared builds for Windows x86_64..." -ForegroundColor Cyan

$baseUrl = "https://github.com/GyanD/codexffmpeg/releases/download/7.1.1-4"
$devZip = "ffmpeg-7.1.1-4-full_build-shared.7z"
$sharedZip = "ffmpeg-7.1.1-4-full_build-shared.7z"

$tempDir = Join-Path $env:TEMP "erika-ffmpeg-download"
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

$devUrl = "$baseUrl/$devZip"
$devZipPath = Join-Path $tempDir $devZip

Write-Host "  Downloading $devUrl ..." -ForegroundColor Yellow
Invoke-WebRequest -Uri $devUrl -OutFile $devZipPath -UseBasicParsing

Write-Host "  Extracting..." -ForegroundColor Yellow
$extractDir = Join-Path $tempDir "ffmpeg-extract"
New-Item -ItemType Directory -Path $extractDir -Force | Out-Null

Push-Location $extractDir
try {
    7z x $devZipPath -y 2>$null
} finally {
    Pop-Location
}

$srcDir = Get-ChildItem $extractDir -Directory | Select-Object -First 1
if (-not $srcDir) {
    Write-Error "Failed to find extracted FFmpeg directory"
    exit 1
}

New-Item -ItemType Directory -Path (Join-Path $distDir "include") -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $distDir "lib") -Force | Out-Null

Copy-Item -Path (Join-Path $srcDir.FullName "include\*") -Destination (Join-Path $distDir "include") -Recurse -Force
Copy-Item -Path (Join-Path $srcDir.FullName "lib\*") -Destination (Join-Path $distDir "lib") -Recurse -Force

Write-Host "FFmpeg installed to $distDir" -ForegroundColor Green
Write-Host "  include: $includeDir"
Write-Host "  lib: $libDir"

Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue