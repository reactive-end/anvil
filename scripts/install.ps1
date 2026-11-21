#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Builds and installs the `anvil` release binary on Windows.
.PARAMETER InstallDir
    Target installation directory. Defaults to "$HOME\.cargo\bin" if Cargo is present,
    otherwise "$env:LOCALAPPDATA\anvil\bin".
.PARAMETER SkipBuild
    Copy an existing target\release\anvil.exe without invoking cargo build.
#>
[CmdletBinding()]
param(
    [string]$InstallDir,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not $InstallDir) {
    $CargoBin = Join-Path $HOME ".cargo\bin"
    if (Test-Path $CargoBin) {
        $InstallDir = $CargoBin
    } else {
        $InstallDir = Join-Path $env:LOCALAPPDATA "anvil\bin"
    }
}

if (-not $SkipBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo is not on PATH. Install Rust via https://rustup.rs/ first."
    }
    Write-Host "Building anvil release binary (LTO=fat, strip=true)..."
    Push-Location $RepoRoot
    try {
        & cargo build --release -p anvil-cli
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release -p anvil-cli failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

$SourceExe = Join-Path $RepoRoot "target\release\anvil.exe"
if (-not (Test-Path $SourceExe)) {
    throw "Release binary not found at $SourceExe"
}

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$DestExe = Join-Path $InstallDir "anvil.exe"
Copy-Item -Path $SourceExe -Destination $DestExe -Force
Write-Host "Installed anvil.exe to $DestExe"

$PathEntries = ($env:PATH -split ";") | Where-Object { $_ } | ForEach-Object { $_.TrimEnd("\") }
$NormalizedInstall = $InstallDir.TrimEnd("\")
if ($PathEntries -notcontains $NormalizedInstall) {
    $UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    $NewUserPath = if ([string]::IsNullOrWhiteSpace($UserPath)) {
        $InstallDir
    } else {
        "$UserPath;$InstallDir"
    }
    [Environment]::SetEnvironmentVariable("PATH", $NewUserPath, "User")
    $env:PATH = "$InstallDir;$env:PATH"
    Write-Host "Added $InstallDir to User PATH."
} else {
    Write-Host "Verified $InstallDir is already on PATH."
}

$Resolved = Get-Command anvil -ErrorAction SilentlyContinue
if (-not $Resolved) {
    Write-Warning "anvil not yet resolved via Get-Command; invoking directly."
}

& $DestExe --version
if ($LASTEXITCODE -ne 0) {
    throw "anvil --version verification failed"
}
