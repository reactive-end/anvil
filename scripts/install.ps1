#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Installs the `anvil` binary on Windows, built from this checkout or downloaded from a GitHub release.
.PARAMETER InstallDir
    Target installation directory. Defaults to "$HOME\.cargo\bin" if Cargo is present,
    otherwise "$env:LOCALAPPDATA\anvil\bin".
.PARAMETER SkipBuild
    Copy an existing target\release\anvil.exe without invoking cargo build.
.PARAMETER FromRelease
    Download the archive from a GitHub release instead of building from source.
    Uses `gh release download` (works for private repositories after `gh auth login`).
.PARAMETER Version
    Release tag to install with -FromRelease (for example v0.1.0). Defaults to the latest release.
.NOTES
    The repository comes from the ANVIL_REPO environment variable (default: reactive-end/anvil).
#>
[CmdletBinding()]
param(
    [string]$InstallDir,
    [switch]$SkipBuild,
    [switch]$FromRelease,
    [string]$Version
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Repo = if ($env:ANVIL_REPO) { $env:ANVIL_REPO } else { "reactive-end/anvil" }

if ($Version -and -not $FromRelease) {
    throw "-Version only applies together with -FromRelease"
}

if (-not $InstallDir) {
    $CargoBin = Join-Path $HOME ".cargo\bin"
    if (Test-Path $CargoBin) {
        $InstallDir = $CargoBin
    } else {
        $InstallDir = Join-Path $env:LOCALAPPDATA "anvil\bin"
    }
}

function Get-ReleaseTarget {
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        "AMD64" { return "x86_64-pc-windows-msvc" }
        "ARM64" { return "aarch64-pc-windows-msvc" }
        default { throw "Unsupported Windows architecture '$arch'; build from source instead (run this script without -FromRelease)." }
    }
}

function Get-ReleaseBinary {
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        throw "-FromRelease needs the GitHub CLI (gh). Install it and run 'gh auth login' (required while the repository is private)."
    }

    $target = Get-ReleaseTarget
    $tag = $Version
    if (-not $tag) {
        $tag = (& gh release view --repo $Repo --json tagName -q .tagName)
        if ($LASTEXITCODE -ne 0 -or -not $tag) {
            throw "Could not read the latest release of $Repo with gh (run 'gh auth login' first)."
        }
        $tag = "$tag".Trim()
    }

    $asset = "anvil-$tag-$target.zip"
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("anvil-install-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmp -Force | Out-Null

    Write-Host "Downloading $asset from $Repo@$tag..."
    & gh release download $tag --repo $Repo --pattern $asset --pattern "$asset.sha256" --dir $tmp --clobber
    if ($LASTEXITCODE -ne 0) {
        throw "gh could not download $asset from $Repo@$tag."
    }

    $zip = Join-Path $tmp $asset
    $sumFile = Join-Path $tmp "$asset.sha256"
    if (-not (Test-Path $zip) -or -not (Test-Path $sumFile)) {
        throw "Release $tag of $Repo does not provide $asset and its .sha256 file."
    }

    $expected = ((Get-Content -Path $sumFile -TotalCount 1) -split "\s+")[0].ToLowerInvariant()
    $actual = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        throw "Checksum mismatch for $asset (expected $expected, got $actual); aborting."
    }

    $extract = Join-Path $tmp "extract"
    Expand-Archive -Path $zip -DestinationPath $extract -Force
    $exe = Join-Path $extract "anvil.exe"
    if (-not (Test-Path $exe)) {
        throw "Archive $asset does not contain anvil.exe."
    }
    return @{ Exe = $exe; TempDir = $tmp }
}

$TempDir = $null
try {
    if ($FromRelease) {
        $downloaded = Get-ReleaseBinary
        $SourceExe = $downloaded.Exe
        $TempDir = $downloaded.TempDir
    } else {
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
    }

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $DestExe = Join-Path $InstallDir "anvil.exe"

    if (Test-Path $DestExe) {
        # A running anvil (daemon, MCP server) keeps its exe open: Windows allows renaming it, not overwriting it.
        try {
            & $DestExe daemon stop *> $null
        } catch {
            Write-Verbose "anvil daemon stop failed (ignored): $_"
        }

        $OldExe = "$DestExe.old"
        if (Test-Path $OldExe) {
            try {
                Remove-Item -Path $OldExe -Force -ErrorAction Stop
            } catch {
                $OldExe = "$DestExe.old-" + (Get-Date -Format "yyyyMMddHHmmss")
            }
        }
        Move-Item -Path $DestExe -Destination $OldExe -Force
        Write-Host "Renamed the existing anvil.exe to $(Split-Path -Leaf $OldExe)."
    }

    Copy-Item -Path $SourceExe -Destination $DestExe -Force
    Write-Host "Installed anvil.exe to $DestExe"
    Write-Host "Note: an already running 'anvil mcp' server keeps the old version until its client restarts it."
} finally {
    if ($TempDir -and (Test-Path $TempDir)) {
        Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

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
