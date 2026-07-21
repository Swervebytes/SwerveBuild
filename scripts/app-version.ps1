# Shared app-version helpers for Swerve Build manifests.
# Dot-source from other scripts: . "$PSScriptRoot\app-version.ps1"
$ErrorActionPreference = "Stop"

function Get-SwerveRepoRoot {
    if ($PSScriptRoot) {
        return (Split-Path -Parent $PSScriptRoot)
    }
    return (Get-Location).Path
}

function Get-SwerveManifestPaths {
    param([string]$Root = (Get-SwerveRepoRoot))
    return [pscustomobject]@{
        Root        = $Root
        PackageJson = Join-Path $Root "package.json"
        PackageLock = Join-Path $Root "package-lock.json"
        CargoToml   = Join-Path $Root "src-tauri\Cargo.toml"
        TauriConf   = Join-Path $Root "src-tauri\tauri.conf.json"
    }
}

function Read-JsonFile {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json)
}

function Get-PackageJsonVersion {
    param([Parameter(Mandatory = $true)][string]$Path)
    $j = Read-JsonFile -Path $Path
    if (-not $j.version) { throw "No version in $Path" }
    return [string]$j.version
}

function Get-CargoPackageVersion {
    param([Parameter(Mandatory = $true)][string]$Path)
    $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    # First [package] version = "..." only (not workspace crate deps).
    if ($raw -notmatch '(?m)^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"') {
        throw "Could not parse package version in $Path"
    }
    return $Matches[1]
}

function Get-TauriConfVersion {
    param([Parameter(Mandatory = $true)][string]$Path)
    $j = Read-JsonFile -Path $Path
    if (-not $j.version) { throw "No version in $Path" }
    return [string]$j.version
}

function Get-SwerveVersions {
    param([string]$Root = (Get-SwerveRepoRoot))
    $p = Get-SwerveManifestPaths -Root $Root
    $pkg = Get-PackageJsonVersion -Path $p.PackageJson
    $cargo = Get-CargoPackageVersion -Path $p.CargoToml
    $tauri = Get-TauriConfVersion -Path $p.TauriConf
    $lock = $null
    if (Test-Path -LiteralPath $p.PackageLock) {
        try { $lock = Get-PackageJsonVersion -Path $p.PackageLock } catch { $lock = $null }
    }
    return [pscustomobject]@{
        PackageJson = $pkg
        CargoToml   = $cargo
        TauriConf   = $tauri
        PackageLock = $lock
        Paths       = $p
    }
}

function Assert-SwerveVersionsInSync {
    param([string]$Root = (Get-SwerveRepoRoot))
    $v = Get-SwerveVersions -Root $Root
    $canonical = $v.PackageJson
    $bad = @()
    if ($v.CargoToml -ne $canonical) {
        $bad += "src-tauri/Cargo.toml=$($v.CargoToml) (package.json=$canonical)"
    }
    if ($v.TauriConf -ne $canonical) {
        $bad += "src-tauri/tauri.conf.json=$($v.TauriConf) (package.json=$canonical)"
    }
    if ($null -ne $v.PackageLock -and $v.PackageLock -ne $canonical) {
        $bad += "package-lock.json=$($v.PackageLock) (package.json=$canonical)"
    }
    if ($bad.Count -gt 0) {
        throw @"
App version drift detected. Canonical is package.json ($canonical).
  $($bad -join "`n  ")
Fix with: npm run version:bump -- -Set $canonical
Or:       npm run version:set -- $canonical
"@
    }
    return $canonical
}

function Test-Semver {
    param([Parameter(Mandatory = $true)][string]$Version)
    return [bool]($Version -match '^\d+\.\d+\.\d+$')
}

function Step-Semver {
    param(
        [Parameter(Mandatory = $true)][string]$Version,
        [Parameter(Mandatory = $true)][ValidateSet("patch", "minor", "major")][string]$Part
    )
    if (-not (Test-Semver $Version)) {
        throw "Not a simple semver X.Y.Z: $Version"
    }
    $bits = $Version.Split(".") | ForEach-Object { [int]$_ }
    $major = $bits[0]; $minor = $bits[1]; $patch = $bits[2]
    switch ($Part) {
        "major" { $major++; $minor = 0; $patch = 0 }
        "minor" { $minor++; $patch = 0 }
        "patch" { $patch++ }
    }
    return "$major.$minor.$patch"
}

function Set-PackageJsonVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Version
    )
    $current = Get-PackageJsonVersion -Path $Path
    if ($current -eq $Version) { return }
    $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    $updated = [regex]::Replace(
        $raw,
        '(?m)^(\s*"version"\s*:\s*")[^"]+(")',
        { param($m) $m.Groups[1].Value + $Version + $m.Groups[2].Value },
        1
    )
    if ($updated -eq $raw) { throw "Failed to update version in $Path" }
    [System.IO.File]::WriteAllText($Path, $updated)
}

function Set-CargoPackageVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Version
    )
    $current = Get-CargoPackageVersion -Path $Path
    if ($current -eq $Version) { return }
    $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    # Only the [package] block's version line (first version = after [package]).
    $updated = [regex]::Replace(
        $raw,
        '(?ms)(^\[package\]\s*(?:(?!^\[).)*?^version\s*=\s*")[^"]+(")',
        { param($m) $m.Groups[1].Value + $Version + $m.Groups[2].Value },
        1
    )
    if ($updated -eq $raw) { throw "Failed to update version in $Path" }
    [System.IO.File]::WriteAllText($Path, $updated)
}

function Set-TauriConfVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Version
    )
    $current = Get-TauriConfVersion -Path $Path
    if ($current -eq $Version) { return }
    $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    $updated = [regex]::Replace(
        $raw,
        '(?m)^(\s*"version"\s*:\s*")[^"]+(")',
        { param($m) $m.Groups[1].Value + $Version + $m.Groups[2].Value },
        1
    )
    if ($updated -eq $raw) { throw "Failed to update version in $Path" }
    [System.IO.File]::WriteAllText($Path, $updated)
}

function Set-PackageLockRootVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Version
    )
    if (-not (Test-Path -LiteralPath $Path)) { return }
    # Prefer npm so the lockfile stays valid JSON/formatting.
    $root = Split-Path -Parent $Path
    Push-Location $root
    try {
        & npm version $Version --no-git-tag-version --allow-same-version 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            # Fallback: patch root "version" fields only.
            $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
            $updated = $raw
            # Top-level version
            $updated = [regex]::Replace($updated, '(?m)^(\s*"version"\s*:\s*")[^"]+(")', {
                    param($m) $m.Groups[1].Value + $Version + $m.Groups[2].Value
                }, 1)
            # packages[""].version — first version inside packages.{""}
            $updated = [regex]::Replace(
                $updated,
                '(?ms)("packages"\s*:\s*\{\s*""\s*:\s*\{(?:(?!"version").)*"version"\s*:\s*")[^"]+(")',
                { param($m) $m.Groups[1].Value + $Version + $m.Groups[2].Value },
                1
            )
            [System.IO.File]::WriteAllText($Path, $updated)
        }
    } finally {
        Pop-Location
    }
}

function Set-SwerveAppVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Version,
        [string]$Root = (Get-SwerveRepoRoot)
    )
    if (-not (Test-Semver $Version)) {
        throw "Version must be X.Y.Z (got: $Version)"
    }
    $p = Get-SwerveManifestPaths -Root $Root

    # package.json + package-lock root via npm (keeps lockfile valid).
    # Capture/discard stdout so it does not become the function's return value.
    Push-Location $Root
    try {
        $npmOut = & npm version $Version --no-git-tag-version --allow-same-version 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Host ($npmOut | Out-String) -ForegroundColor DarkGray
            Set-PackageJsonVersion -Path $p.PackageJson -Version $Version
            Set-PackageLockRootVersion -Path $p.PackageLock -Version $Version
        }
    } finally {
        Pop-Location
    }

    # Ensure package.json is exact (npm can be picky about pre-existing drift).
    Set-PackageJsonVersion -Path $p.PackageJson -Version $Version
    Set-CargoPackageVersion -Path $p.CargoToml -Version $Version
    Set-TauriConfVersion -Path $p.TauriConf -Version $Version

    # Re-sync lock root if npm left it behind package.json
    $lockVer = $null
    if (Test-Path -LiteralPath $p.PackageLock) {
        try { $lockVer = Get-PackageJsonVersion -Path $p.PackageLock } catch { $lockVer = $null }
    }
    if ($null -ne $lockVer -and $lockVer -ne $Version) {
        Set-PackageLockRootVersion -Path $p.PackageLock -Version $Version
    }

    $synced = Assert-SwerveVersionsInSync -Root $Root
    # Single return value only (no accidental pipeline pollution).
    return ,$synced
}
