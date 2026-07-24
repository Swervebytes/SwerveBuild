# Unattended live product smoke for session-close.
#
# Opens installed Swerve Build (minimized / no desktop focus), ensures App UI
# grant + CDP, runs MCP App UI smoke and optional media encode smoke, then
# closes the app.
#
# Usage:
#   npm run smoke:session
#   npm run smoke:session -- -Profile core
#   npm run smoke:session -- core
#   .\scripts\session-smoke.ps1 -Profile media
#   .\scripts\session-smoke.ps1 -Profile docs
#   .\scripts\session-smoke.ps1 -SkipLaunch
#   .\scripts\session-smoke.ps1 -NoKill
#
# Exit 0 = pass; non-zero = fail (do not close session green).

param(
    [Parameter(Position = 0)]
    [ValidateSet("core", "media", "full", "docs", "deep")]
    [string]$Profile = "full",
    [switch]$SkipLaunch,
    [switch]$NoKill,
    [switch]$NoGrantWrite,
    [string]$AppExe = "",
    [string]$McpExe = "",
    [int]$ReadyTimeoutSec = 90
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\app-version.ps1"
$root = Get-SwerveRepoRoot

function Write-Step {
    param([string]$Msg, [string]$Color = "Cyan")
    Write-Host $Msg -ForegroundColor $Color
}

function Stop-SwerveBuildApp {
    $names = @("swerve-build", "Swerve Build")
    foreach ($n in $names) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Step "Stopping $($_.ProcessName) (pid $($_.Id))..." "Yellow"
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
        }
    }
    Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match 'swerve-build' } |
        ForEach-Object {
            Write-Step "Stopping $($_.Name) (pid $($_.ProcessId))..." "Yellow"
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        }
    Start-Sleep -Seconds 1
}

function Resolve-SwerveAppExe {
    param([string]$Override)
    if ($Override -and (Test-Path -LiteralPath $Override)) { return $Override }
    if ($env:SWERVE_APP_EXE -and (Test-Path -LiteralPath $env:SWERVE_APP_EXE)) {
        return $env:SWERVE_APP_EXE
    }
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "Swerve Build\swerve-build.exe"),
        (Join-Path $env:LOCALAPPDATA "SwerveBuild\swerve-build.exe"),
        (Join-Path $root "src-tauri\target\release\swerve-build.exe")
    )
    foreach ($c in $candidates) {
        if (Test-Path -LiteralPath $c) { return $c }
    }
    throw "swerve-build.exe not found. Run npm run install:local first, or set SWERVE_APP_EXE."
}

function Resolve-SwerveMcpExe {
    param([string]$Override, [string]$AppPath)
    if ($Override -and (Test-Path -LiteralPath $Override)) { return $Override }
    if ($env:SWERVE_MCP_EXE -and (Test-Path -LiteralPath $env:SWERVE_MCP_EXE)) {
        return $env:SWERVE_MCP_EXE
    }
    if ($AppPath) {
        $nextToApp = Join-Path (Split-Path $AppPath -Parent) "swervebuild-mcp.exe"
        if (Test-Path -LiteralPath $nextToApp) { return $nextToApp }
    }
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "Swerve Build\swervebuild-mcp.exe"),
        (Join-Path $root "src-tauri\target\release\swervebuild_mcp.exe"),
        (Join-Path $root "src-tauri\binaries\swervebuild-mcp-x86_64-pc-windows-msvc.exe")
    )
    foreach ($c in $candidates) {
        if (Test-Path -LiteralPath $c) { return $c }
    }
    throw "swervebuild-mcp.exe not found."
}

function Ensure-AppUiGrant {
    $data = Join-Path $env:USERPROFILE ".swervebuild"
    if (-not (Test-Path -LiteralPath $data)) {
        New-Item -ItemType Directory -Path $data -Force | Out-Null
    }
    $grantPath = Join-Path $data "app_ui_grant.json"
    $now = (Get-Date).ToUniversalTime().ToString("o")
    # No UTF-8 BOM -- serde_json rejects BOM and would treat grant as default denied.
    $json = "{`n  `"granted`": true,`n  `"updatedAt`": `"$now`"`n}`n"
    $utf8NoBom = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($grantPath, $json, $utf8NoBom)
    # Verify round-trip
    $check = Get-Content -LiteralPath $grantPath -Raw | ConvertFrom-Json
    if (-not $check.granted) {
        throw "Failed to write app_ui grant (granted != true) at $grantPath"
    }
    Write-Step "App UI grant ON -> $grantPath" "DarkGray"
}

function Wait-CdpReady {
    param([int]$TimeoutSec)
    $cdpPath = Join-Path $env:USERPROFILE ".swervebuild\app_ui_cdp.json"
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path -LiteralPath $cdpPath) {
            try {
                $ep = Get-Content -LiteralPath $cdpPath -Raw | ConvertFrom-Json
                $hostName = if ($ep.host) { [string]$ep.host } else { "127.0.0.1" }
                $port = [int]$ep.port
                if ($port -gt 0) {
                    $uri = "http://${hostName}:${port}/json/version"
                    $resp = Invoke-WebRequest -Uri $uri -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
                    if ($resp.StatusCode -eq 200) {
                        Write-Step "CDP ready on ${hostName}:${port}" "Green"
                        return $ep
                    }
                }
            } catch {
                # still starting
            }
        }
        Start-Sleep -Milliseconds 500
    }
    throw "CDP not ready within ${TimeoutSec}s (grant on + restart required for release). Path: $cdpPath"
}

function Start-SwerveMinimized {
    param([string]$Exe)
    $env:SWERVE_ENABLE_CDP = "1"
    Write-Step "Launching (minimized, SWERVE_ENABLE_CDP=1): $Exe" "Cyan"
    $p = Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe -Parent) `
        -WindowStyle Minimized -PassThru
    if (-not $p) { throw "Start-Process failed for $Exe" }
    Write-Step "Started pid $($p.Id)" "DarkGray"
    return $p
}

function Invoke-McpSmoke {
    param(
        [string]$McpPath,
        [string]$PyProfile
    )
    $env:SWERVE_MCP_EXE = $McpPath
    $pyScript = Join-Path $PSScriptRoot "session-smoke-mcp.py"
    if (-not (Test-Path -LiteralPath $pyScript)) {
        throw "Missing $pyScript"
    }

    Write-Step "MCP smoke profile=$PyProfile" "Cyan"
    $exitCode = 1
    $ran = $false

    try {
        $null = & python --version 2>&1
        if ($LASTEXITCODE -eq 0) {
            & python $pyScript --profile $PyProfile
            $exitCode = $LASTEXITCODE
            $ran = $true
        }
    } catch { }

    if (-not $ran) {
        try {
            $null = & py -3 --version 2>&1
            & py -3 $pyScript --profile $PyProfile
            $exitCode = $LASTEXITCODE
            $ran = $true
        } catch { }
    }

    if (-not $ran) {
        throw "Python not found (need python or py -3 for MCP smoke)"
    }
    if ($exitCode -ne 0) {
        throw "MCP smoke failed (exit $exitCode)"
    }
    Write-Step "MCP smoke OK" "Green"
}

function Invoke-MediaSmoke {
    Write-Step "Media smoke: live_encode_clip_audio (ignored cargo test)" "Cyan"
    Push-Location $root
    try {
        & cargo test --manifest-path src-tauri/Cargo.toml -p swerve-build --lib `
            media_worker::tests::live_encode_clip_audio -- --ignored --nocapture
        if ($LASTEXITCODE -ne 0) {
            throw "Media live encode smoke failed (exit $LASTEXITCODE)"
        }
    } finally {
        Pop-Location
    }
    Write-Step "Media smoke OK" "Green"
}

# --- main ---
$failed = $false
$startedHere = $false
$report = New-Object System.Collections.Generic.List[string]
$appPath = $null

try {
    Write-Step "=== session-smoke profile=$Profile ===" "Green"

    if ($Profile -eq "docs") {
        Write-Step "Docs-only profile - no app launch (pass)." "DarkGray"
        $report.Add("Live smoke: skipped (docs-only)")
        Write-Host ($report -join "`n")
        exit 0
    }

    if (-not $NoGrantWrite) {
        Ensure-AppUiGrant
    }

    $needUi = $Profile -in @("core", "full", "deep")
    $needMedia = $Profile -in @("media", "full")

    if ($needUi -and -not $SkipLaunch) {
        Stop-SwerveBuildApp
        $cdpPath = Join-Path $env:USERPROFILE ".swervebuild\app_ui_cdp.json"
        if (Test-Path -LiteralPath $cdpPath) {
            Remove-Item -LiteralPath $cdpPath -Force -ErrorAction SilentlyContinue
        }
        $appPath = Resolve-SwerveAppExe -Override $AppExe
        Start-SwerveMinimized -Exe $appPath | Out-Null
        $startedHere = $true
        Wait-CdpReady -TimeoutSec $ReadyTimeoutSec | Out-Null
    } elseif ($needUi -and $SkipLaunch) {
        Write-Step "SkipLaunch: waiting for existing CDP..." "DarkGray"
        Wait-CdpReady -TimeoutSec $ReadyTimeoutSec | Out-Null
        try { $appPath = Resolve-SwerveAppExe -Override $AppExe } catch { $appPath = "" }
    }

    if ($needUi) {
        if (-not $appPath) {
            try { $appPath = Resolve-SwerveAppExe -Override $AppExe } catch { $appPath = "" }
        }
        $mcp = Resolve-SwerveMcpExe -Override $McpExe -AppPath $appPath
        $pyProfile = if ($Profile -eq "deep") { "deep" } else { "core" }
        Invoke-McpSmoke -McpPath $mcp -PyProfile $pyProfile
        $report.Add("MCP/CDP App UI smoke: PASS ($pyProfile)")
    }

    if ($needMedia) {
        Invoke-MediaSmoke
        $report.Add("Media encode live smoke: PASS")
    }

    Write-Step "=== session-smoke PASS ===" "Green"
    $report.Add("session-smoke: PASS profile=$Profile")
} catch {
    $failed = $true
    Write-Host "=== session-smoke FAIL ===" -ForegroundColor Red
    Write-Host $_.Exception.Message -ForegroundColor Red
    $report.Add("session-smoke: FAIL - $($_.Exception.Message)")
} finally {
    if ($startedHere -and -not $NoKill) {
        Write-Step "Closing app after smoke..." "DarkGray"
        Stop-SwerveBuildApp
    } elseif ($startedHere -and $NoKill) {
        Write-Step "Leaving app running (-NoKill)" "Yellow"
    }
}

Write-Host ""
Write-Host "--- smoke report (paste into S##_progress Live smoke) ---" -ForegroundColor DarkGray
$report | ForEach-Object { Write-Host $_ }
Write-Host "-------------------------------------------------------" -ForegroundColor DarkGray

if ($failed) { exit 1 }
exit 0
