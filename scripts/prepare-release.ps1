# Build release binaries, stage the MCP sidecar, and produce an installer.
# Does not mutate tauri.conf.json: externalBin is applied via a merge overlay
# so a killed build cannot leave the tracked conf dirty (F4).
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$tauri = Join-Path $root "src-tauri"
$triple = "x86_64-pc-windows-msvc"
$overlay = Join-Path $root "scripts\tauri-release-externalbin.json"

if (-not (Test-Path $overlay)) {
    throw "Missing release overlay: $overlay"
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

    Write-Host "Running Tauri bundle (externalBin via overlay, conf untouched)..." -ForegroundColor Cyan
    # Call the CLI binary directly. `npm run tauri -- build --config …` is unsafe:
    # npm steals `--config` as its own flag and forwards the path to cargo.
    $tauriCli = Join-Path $root "node_modules\.bin\tauri.cmd"
    if (-not (Test-Path $tauriCli)) {
        throw "Missing Tauri CLI: $tauriCli (run npm install)"
    }
    & $tauriCli build --config $overlay
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }

    Write-Host "`nRelease artifacts: src-tauri/target/release/bundle/" -ForegroundColor Green
    Write-Host "Tag path (after main is clean + CI green): git tag -a v0.2.1 -m 'v0.2.1'; git push origin main --tags" -ForegroundColor DarkGray
} finally {
    Pop-Location
}
