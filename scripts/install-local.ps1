# Build the release installer and install it over the local desktop app.
# Intended for session-close: after product work, desktop icon = this build.
#
# Usage:
#   npm run install:local
#   npm run install:local -- -Bump minor
#   .\scripts\install-local.ps1 -SkipBuild          # reinstall last built NSIS only
#   .\scripts\install-local.ps1 -NoKill             # do not stop running app first
param(
    [ValidateSet("patch", "minor", "major", "")]
    [string]$Bump = "",
    [string]$Set = "",
    [switch]$SkipBuild,
    [switch]$NoKill
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\app-version.ps1"
$root = Get-SwerveRepoRoot

function Stop-SwerveBuildApp {
    $names = @("swerve-build", "Swerve Build")
    $stopped = @()
    foreach ($n in $names) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "Stopping $($_.ProcessName) (pid $($_.Id))..." -ForegroundColor Yellow
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
            $stopped += $_.Id
        }
    }
    # Binary name from Cargo package (default-run = swerve-build)
    Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match 'swerve-build|Swerve Build' } |
        ForEach-Object {
            Write-Host "Stopping process $($_.Name) (pid $($_.ProcessId))..." -ForegroundColor Yellow
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
            $stopped += $_.ProcessId
        }
    if ($stopped.Count -gt 0) {
        Start-Sleep -Seconds 2
    } else {
        Write-Host "No running Swerve Build process found." -ForegroundColor DarkGray
    }
}

function Find-NsisInstaller {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Version
    )
    $nsisDir = Join-Path $Root "src-tauri\target\release\bundle\nsis"
    $exact = Join-Path $nsisDir "Swerve Build_${Version}_x64-setup.exe"
    if (Test-Path -LiteralPath $exact) { return $exact }
    if (-not (Test-Path -LiteralPath $nsisDir)) {
        throw "NSIS bundle dir missing: $nsisDir (run without -SkipBuild)"
    }
    $latest = Get-ChildItem -LiteralPath $nsisDir -Filter "Swerve Build_*_x64-setup.exe" |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $latest) {
        throw "No NSIS installer found under $nsisDir"
    }
    Write-Host "Exact version installer not found; using newest: $($latest.Name)" -ForegroundColor Yellow
    return $latest.FullName
}

Push-Location $root
try {
    if ($Set) {
        Write-Host "Setting version to $Set..." -ForegroundColor Cyan
        & "$PSScriptRoot\bump-version.ps1" -Set $Set
    } elseif ($Bump) {
        Write-Host "Bumping version ($Bump)..." -ForegroundColor Cyan
        & "$PSScriptRoot\bump-version.ps1" -Part $Bump
    }

    $version = Assert-SwerveVersionsInSync -Root $root
    Write-Host "App version: $version" -ForegroundColor Green

    if (-not $NoKill) {
        Stop-SwerveBuildApp
    }

    if (-not $SkipBuild) {
        Write-Host "Building release installer (npm run release)..." -ForegroundColor Cyan
        & "$PSScriptRoot\prepare-release.ps1"
        if ($LASTEXITCODE -ne 0) { throw "prepare-release.ps1 failed with exit $LASTEXITCODE" }
    }

    $installer = Find-NsisInstaller -Root $root -Version $version
    if (-not (Test-Path -LiteralPath $installer)) {
        throw "Installer not found: $installer"
    }
    Write-Host "Installing: $installer" -ForegroundColor Cyan

    # Tauri NSIS: /S = silent. installMode is currentUser in tauri.conf.json.
    $proc = Start-Process -FilePath $installer -ArgumentList "/S" -Wait -PassThru
    if ($proc.ExitCode -ne 0 -and $null -ne $proc.ExitCode) {
        # Some NSIS builds return null/0 on success; non-zero is failure.
        throw "Installer exited with code $($proc.ExitCode)"
    }

    Write-Host ""
    Write-Host "Local install complete - v$version" -ForegroundColor Green
    Write-Host "Open Swerve Build from the desktop/start menu icon to run this build." -ForegroundColor DarkGray
    Write-Host "Installer: $installer" -ForegroundColor DarkGray
} finally {
    Pop-Location
}
