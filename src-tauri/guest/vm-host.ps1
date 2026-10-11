# Host-side Hyper-V driver. One argument: path to a request JSON file.
# The guest sign-in secret is DPAPI text for this Windows user. This script
# never prints that secret.
param([Parameter(Mandatory = $true)][string]$RequestPath)
$ErrorActionPreference = 'Stop'
$req = Get-Content -Raw -Path $RequestPath | ConvertFrom-Json
$resultPath = [string]$req.resultPath
$guestRoot = 'C:\ProgramData\SwerveBuild'

function Write-Result($obj) {
  $obj | ConvertTo-Json -Compress -Depth 6 | Set-Content -Path $resultPath -Encoding utf8
}

function Get-GuestCredential {
  if (-not (Test-Path -LiteralPath ([string]$req.secretPath))) {
    throw 'Save the guest sign-in secret in Settings, under Test virtual machine.'
  }
  $secure = Get-Content -Raw -Path ([string]$req.secretPath) | ConvertTo-SecureString
  New-Object System.Management.Automation.PSCredential ([string]$req.username, $secure)
}

function Enable-GuestCopy {
  $svc = Get-VMIntegrationService -VMName ([string]$req.vmName) |
    Where-Object { $_.Name -match 'Guest Service' }
  if ($svc -and -not $svc.Enabled) {
    Enable-VMIntegrationService -VMName ([string]$req.vmName) -Name $svc.Name
  }
}

function Wait-GuestFile {
  $cred = Get-GuestCredential
  $deadline = (Get-Date).AddSeconds(80)
  do {
    Start-Sleep -Seconds 1
    $raw = Invoke-Command -VMName ([string]$req.vmName) -Credential $cred -ScriptBlock {
      $path = 'C:\ProgramData\SwerveBuild\result.json'
      if (Test-Path -LiteralPath $path) { Get-Content -Raw -Path $path } else { '' }
    }
    if ($raw) { return [string]$raw }
  } while ((Get-Date) -lt $deadline)
  throw 'The logged-on desktop did not answer. The snapshot needs a user already signed in on the console.'
}

function Copy-GuestShot {
  if (-not $req.hostOut) { return }
  $dest = [string]$req.hostOut
  $parent = Split-Path -Parent $dest
  if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
  $copied = $false
  for ($i = 0; $i -lt 8; $i++) {
    try {
      Copy-VMFile -Name ([string]$req.vmName) -SourcePath "$guestRoot\screen.png" -DestinationPath $dest -FileSource Guest -Force
      $copied = $true
      break
    } catch {
      Start-Sleep -Milliseconds 400
    }
  }
  if (-not $copied) { throw 'The guest took a screenshot but the host could not copy it (enable Guest services).' }
}

function Invoke-OnDesktop([string]$action) {
  $cred = Get-GuestCredential
  $guestReq = @{
    action = $action
    controlName = [string]$req.controlName
    text = [string]$req.text
    key = [string]$req.key
  } | ConvertTo-Json -Compress
  Invoke-Command -VMName ([string]$req.vmName) -Credential $cred -ScriptBlock {
    param($script, $json, $user)
    $dir = 'C:\ProgramData\SwerveBuild'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Set-Content -Path "$dir\drive.ps1" -Value $script -Encoding utf8
    Set-Content -Path "$dir\request.json" -Value $json -Encoding utf8
    Remove-Item -LiteralPath "$dir\result.json","$dir\screen.png" -Force -ErrorAction SilentlyContinue
    $task = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\ProgramData\SwerveBuild\drive.ps1'
    & schtasks.exe /Create /TN SwerveBuildDrive /TR $task /SC ONCE /ST 23:59 /RU $user /IT /F | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "could not schedule the desktop task (exit $LASTEXITCODE). The guest user must be logged on." }
    & schtasks.exe /Run /TN SwerveBuildDrive | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "could not start the desktop task (exit $LASTEXITCODE)." }
  } -ArgumentList ([string]$req.guestScript), $guestReq, ([string]$req.username)
  $raw = Wait-GuestFile
  $parsed = $raw | ConvertFrom-Json
  if (-not $parsed.ok) { throw [string]$parsed.error }
  Copy-GuestShot
}

try {
  switch ([string]$req.action) {
    'status' {
      $cmd = Get-Command Get-VM -ErrorAction SilentlyContinue
      if (-not $cmd) { throw 'Hyper-V PowerShell tools are not installed on this PC.' }
      $vm = Get-VM -Name ([string]$req.vmName) -ErrorAction Stop
      $snap = @()
      foreach ($s in @(Get-VMSnapshot -VMName $vm.Name -ErrorAction SilentlyContinue)) {
        $snap += [string]$s.Name
      }
      Write-Result @{
        ok = $true
        vmName = [string]$vm.Name
        state = [string]$vm.State
        snapshots = $snap
      }
    }
    'prepare' {
      $name = [string]$req.vmName
      $snap = [string]$req.snapshot
      $vm = Get-VM -Name $name -ErrorAction Stop
      if ($vm.State -ne 'Off') {
        Stop-VM -Name $name -TurnOff -Force
      }
      Restore-VMSnapshot -VMName $name -Name $snap -Confirm:$false
      Start-VM -Name $name
      Enable-GuestCopy
      $deadline = (Get-Date).AddMinutes(3)
      $heartbeat = ''
      do {
        Start-Sleep -Seconds 2
        $hb = Get-VMIntegrationService -VMName $name | Where-Object { $_.Name -match 'Heartbeat' }
        $heartbeat = [string]$hb.PrimaryStatusDescription
        if ($heartbeat -match 'OK') { break }
      } while ((Get-Date) -lt $deadline)
      Write-Result @{ ok = $true; vmName = $name; snapshot = $snap; heartbeat = $heartbeat }
    }
    'screenshot' { Invoke-OnDesktop 'screenshot'; Write-Result @{ ok = $true; path = [string]$req.hostOut } }
    'invoke' { Invoke-OnDesktop 'invoke'; Write-Result @{ ok = $true; path = [string]$req.hostOut; control = [string]$req.controlName } }
    'type' { Invoke-OnDesktop 'type'; Write-Result @{ ok = $true; path = [string]$req.hostOut; control = [string]$req.controlName } }
    'press' { Invoke-OnDesktop 'press'; Write-Result @{ ok = $true; path = [string]$req.hostOut; key = [string]$req.key } }
    'console' {
      $exe = Join-Path $env:SystemRoot 'System32\vmconnect.exe'
      if (-not (Test-Path -LiteralPath $exe)) {
        $exe = Join-Path $env:ProgramFiles 'Hyper-V\vmconnect.exe'
      }
      if (-not (Test-Path -LiteralPath $exe)) { throw 'vmconnect.exe is not installed (Hyper-V management tools).' }
      Start-Process -FilePath $exe -ArgumentList @('localhost', [string]$req.vmName)
      Write-Result @{ ok = $true; opened = $true }
    }
    default { throw "unknown action $($req.action)" }
  }
} catch {
  Write-Result @{ ok = $false; error = $_.Exception.Message }
  exit 1
}
