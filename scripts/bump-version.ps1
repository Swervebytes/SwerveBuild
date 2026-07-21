# Bump or set the Swerve Build app version across all manifests.
# Canonical: package.json → also Cargo.toml, tauri.conf.json, package-lock.json.
#
# Usage:
#   .\scripts\bump-version.ps1 -Part minor          # 0.2.1 → 0.3.0
#   .\scripts\bump-version.ps1 -Part patch
#   .\scripts\bump-version.ps1 -Set 0.3.0
#   .\scripts\bump-version.ps1 -CheckOnly           # fail if manifests drift
#   npm run version:bump -- -Part minor
#   npm run version:set -- 0.3.0
param(
    [ValidateSet("patch", "minor", "major")]
    [string]$Part,
    [string]$Set,
    [switch]$CheckOnly
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\app-version.ps1"
$root = Get-SwerveRepoRoot

if ($CheckOnly) {
    $v = Assert-SwerveVersionsInSync -Root $root
    Write-Host "Versions in sync: $v" -ForegroundColor Green
    exit 0
}

# Canonical read: package.json wins even if other manifests drifted (repair path).
$versions = Get-SwerveVersions -Root $root
$current = $versions.PackageJson

if ($Set) {
    $next = $Set.Trim().TrimStart("v", "V")
} elseif ($Part) {
    # Stepping requires a known base; repair drift first if needed.
    if ($versions.CargoToml -ne $current -or $versions.TauriConf -ne $current) {
        Write-Host "Repairing manifest drift to package.json $current before -$Part..." -ForegroundColor Yellow
        $null = Set-SwerveAppVersion -Version $current -Root $root
    }
    $next = Step-Semver -Version $current -Part $Part
} else {
    throw "Specify -Part patch|minor|major, -Set X.Y.Z, or -CheckOnly"
}

if ($next -eq $current -and $versions.CargoToml -eq $next -and $versions.TauriConf -eq $next) {
    Write-Host "Already at $current (no change)." -ForegroundColor Yellow
    exit 0
}

Write-Host "Bumping $current → $next" -ForegroundColor Cyan
$applied = Set-SwerveAppVersion -Version $next -Root $root
Write-Host "OK: app version is now $applied" -ForegroundColor Green
Write-Host "  package.json, package-lock.json, src-tauri/Cargo.toml, src-tauri/tauri.conf.json" -ForegroundColor DarkGray
Write-Host "Tag when releasing: git tag -a v$applied -m `"v$applied`"" -ForegroundColor DarkGray
