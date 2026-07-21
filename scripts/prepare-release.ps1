# Build release binaries, stage the MCP sidecar, and produce an installer.
# Does not mutate tauri.conf.json: externalBin is applied via a merge overlay
# so a killed build cannot leave the tracked conf dirty (F4).
#
# Preflight: package.json / Cargo.toml / tauri.conf.json versions must match.
$ErrorActionPreference = "Stop"
. "$PSScriptRoot\app-version.ps1"
$root = Get-SwerveRepoRoot
$tauri = Join-Path $root "src-tauri"
$triple = "x86_64-pc-windows-msvc"
$overlay = Join-Path $root "scripts\tauri-release-externalbin.json"

if (-not (Test-Path $overlay)) {
    throw "Missing release overlay: $overlay"
}

$version = Assert-SwerveVersionsInSync -Root $root
Write-Host "Release version (manifests in sync): $version" -ForegroundColor Green

Push-Location $root
try {
    Write-Host "Building frontend..." -ForegroundColor Cyan
    npm run build
    if ($LASTEXITCODE -ne 0) { throw "npm run build failed with exit $LASTEXITCODE" }

    Push-Location $tauri
    Write-Host "Building release binaries..." -ForegroundColor Cyan
    cargo build --release --bins
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed with exit $LASTEXITCODE" }

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

    $installer = Join-Path $tauri "target\release\bundle\nsis\Swerve Build_${version}_x64-setup.exe"
    Write-Host "`nRelease artifacts: src-tauri/target/release/bundle/" -ForegroundColor Green
    if (Test-Path -LiteralPath $installer) {
        Write-Host "Installer: $installer" -ForegroundColor Green
    }
    Write-Host "Local install:  npm run install:local" -ForegroundColor DarkGray
    Write-Host "Tag path (after main is clean + CI green): git tag -a v$version -m `"v$version`"; git push origin main --tags" -ForegroundColor DarkGray
} finally {
    Pop-Location
}
