# Build release binaries, stage the MCP sidecar, and produce an installer.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$tauri = Join-Path $root "src-tauri"
$confPath = Join-Path $tauri "tauri.conf.json"
$triple = "x86_64-pc-windows-msvc"

function Restore-TauriConf([string]$original) {
    Set-Content -Path $confPath -Value $original -NoNewline
}

Push-Location $root
try {
    Write-Host "Building frontend..." -ForegroundColor Cyan
    npm run build

    Push-Location $tauri
    Write-Host "Building release binaries..." -ForegroundColor Cyan
    cargo build --release --bins

    $mcpSrc = Join-Path $tauri "target\release\swervebuild-mcp.exe"
    if (-not (Test-Path $mcpSrc)) {
        throw "Missing $mcpSrc"
    }

    $binDir = Join-Path $tauri "binaries"
    New-Item -Force -ItemType Directory $binDir | Out-Null
    $mcpDest = Join-Path $binDir "swervebuild-mcp-$triple.exe"
    Copy-Item $mcpSrc $mcpDest -Force
    Write-Host "Staged sidecar: $mcpDest" -ForegroundColor Green
    Pop-Location

    $confOriginal = Get-Content $confPath -Raw
    $confPatched = $confOriginal
    if ($confPatched -notmatch '"externalBin"') {
        $confPatched = $confPatched -replace '("targets": [^\n]+,)', "`$1`n    `"externalBin`": [`"binaries/swervebuild-mcp`"],"
        Set-Content -Path $confPath -Value $confPatched -NoNewline
        Write-Host "Temporarily enabled bundle.externalBin in tauri.conf.json" -ForegroundColor Yellow
    }

    try {
        Write-Host "Running Tauri bundle..." -ForegroundColor Cyan
        npm run tauri build
    } finally {
        if ($confPatched -ne $confOriginal) {
            Restore-TauriConf $confOriginal
            Write-Host "Restored tauri.conf.json (externalBin removed for dev builds)" -ForegroundColor Yellow
        }
    }

    Write-Host "`nRelease artifacts: src-tauri/target/release/bundle/" -ForegroundColor Green
} finally {
    Pop-Location
}