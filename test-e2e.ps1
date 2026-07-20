# Swerve Build end-to-end tests (CLI layer)
$ErrorActionPreference = "Stop"

# Windows PowerShell 5.1's .NET pipe writer prepends a UTF-8 BOM to child stdin,
# which strict JSON-RPC peers (grok agent stdio) reject at parse time. Re-run
# under pwsh when available; swervebuild-mcp itself tolerates the BOM either way.
if ($PSVersionTable.PSEdition -eq 'Desktop' -and (Get-Command pwsh -ErrorAction SilentlyContinue)) {
    & pwsh -NoProfile -ExecutionPolicy Bypass -File $MyInvocation.MyCommand.Path
    exit $LASTEXITCODE
}
$passed = 0
$failed = 0
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$tauriDir = Join-Path $root "src-tauri"
$mcpExe = Join-Path $tauriDir "target\debug\swervebuild-mcp.exe"
$desktopExe = Join-Path $tauriDir "target\debug\swerve-build.exe"
# Optional: set SWERVE_E2E_CWD to a local project folder for live ACP tests.
$testCwd = $env:SWERVE_E2E_CWD

function Assert-True($cond, [string]$name, [string]$detail = "") {
    if ($cond) {
        Write-Host "[PASS] $name" -ForegroundColor Green
        $script:passed++
    } else {
        Write-Host "[FAIL] $name" -ForegroundColor Red
        if ($detail) { Write-Host "       $detail" -ForegroundColor DarkRed }
        $script:failed++
    }
}

function Invoke-McpRpc([string]$method, $id = 1, $params = $null) {
    $req = @{ jsonrpc = "2.0"; id = $id; method = $method }
    if ($null -ne $params) { $req.params = $params }
    $line = ($req | ConvertTo-Json -Compress -Depth 10)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $mcpExe
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    # Write raw BOM-less UTF-8 bytes: under Windows PowerShell 5.1 the redirected
    # StandardInput writer can emit an encoding preamble that makes the first JSON
    # line unparseable (every RPC then times out as null). BaseStream sidesteps it.
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes("$line`n")
    $p.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
    $p.StandardInput.BaseStream.Flush()
    $p.StandardInput.Close()
    $out = $p.StandardOutput.ReadToEnd()
    $p.WaitForExit(5000) | Out-Null
    if (-not $p.HasExited) { $p.Kill() }
    return ($out.Trim() -split "`n" | Where-Object { $_ } | Select-Object -Last 1 | ConvertFrom-Json)
}

function Get-DataStorePath {
    $newPath = Join-Path $env:USERPROFILE ".swervebuild\data.json"
    $legacyPath = Join-Path $env:USERPROFILE ".swervegrok\data.json"
    if (Test-Path $newPath) { return $newPath }
    if (Test-Path $legacyPath) { return $legacyPath }
    return $newPath
}

Write-Host "`n=== Swerve Build E2E Tests ===`n" -ForegroundColor Cyan

# 1. Binaries
Assert-True (Test-Path $desktopExe) "Desktop binary exists" $desktopExe
Assert-True (Test-Path $mcpExe) "MCP binary exists" $mcpExe

# 2. MCP tools
$init = Invoke-McpRpc "initialize" 1 @{ protocolVersion = "2024-11-05"; capabilities = @{}; clientInfo = @{ name = "test" } }
Assert-True ($init.result.serverInfo.name -eq "swervebuild-mcp") "MCP initialize"

$tools = Invoke-McpRpc "tools/list" 2
Assert-True ($tools.result.tools.Count -ge 4) "MCP tools/list count" "got $($tools.result.tools.Count)"

$status = Invoke-McpRpc "tools/call" 3 @{ name = "swervebuild_get_app_status"; arguments = @{} }
Assert-True ($null -ne $status -and -not $status.result.isError) "MCP swervebuild_get_app_status"
Assert-True ($null -ne $status -and $null -ne $status.result.content -and $null -ne $status.result.content[0].text) "MCP app status returns text"

$projects = Invoke-McpRpc "tools/call" 4 @{ name = "swervebuild_list_projects"; arguments = @{} }
Assert-True ($projects.result.content[0].text -match "projects") "MCP swervebuild_list_projects"

# Automation tools are exposed to agents
$toolNames = $tools.result.tools | ForEach-Object { $_.name }
Assert-True ($toolNames -contains "swervebuild_list_automations") "MCP exposes swervebuild_list_automations"
Assert-True ($toolNames -contains "swervebuild_list_automation_runs") "MCP exposes swervebuild_list_automation_runs"
$autos = Invoke-McpRpc "tools/call" 6 @{ name = "swervebuild_list_automations"; arguments = @{} }
Assert-True (-not $autos.result.isError -and $autos.result.content[0].text -match "automations") "MCP swervebuild_list_automations"

# Legacy tool aliases still work
$legacyStatus = Invoke-McpRpc "tools/call" 5 @{ name = "swervegrok_get_app_status"; arguments = @{} }
Assert-True (-not $legacyStatus.result.isError) "MCP legacy swervegrok_get_app_status alias"

# 3. ACP session + MCP injection
Write-Host "`n--- ACP session test (may take ~60s) ---`n" -ForegroundColor Yellow
$mcpPath = $mcpExe
$sessionId = $null
$gotUpdate = $false
$permissionSeen = $false
$promptDone = $false

if (-not $testCwd -or -not (Test-Path $testCwd)) {
    Write-Host "[SKIP] ACP session tests - set SWERVE_E2E_CWD to a project folder to run live Grok ACP tests" -ForegroundColor DarkYellow
    $dataPath = Get-DataStorePath
    if (Test-Path $dataPath) {
        $store = Get-Content $dataPath -Raw | ConvertFrom-Json
        # Machine-state check, same policy as chats below: verify when present, skip when fresh.
        if ($store.projects.Count -gt 0) {
            Assert-True $true "Workspace data.json has projects"
        } else {
            Write-Host "[SKIP] Workspace data.json has no projects yet" -ForegroundColor DarkYellow
        }
        if ($store.chats.Count -ge 1) {
            Assert-True $true "Workspace data.json has chats"
        } else {
            Write-Host "[SKIP] Workspace data.json has no chats yet" -ForegroundColor DarkYellow
        }
    } else {
        Write-Host "[SKIP] Workspace data.json - no local ~/.swervebuild data on this machine" -ForegroundColor DarkYellow
    }
    Write-Host "`n=== Results: $passed passed, $failed failed ===`n" -ForegroundColor Cyan
    if ($failed -gt 0) { exit 1 }
    exit 0
}

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = (Get-Command grok).Source
$psi.Arguments = "agent stdio"
$psi.WorkingDirectory = $testCwd
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.CreateNoWindow = $true
$acp = [System.Diagnostics.Process]::Start($psi)

function Send-Acp($obj) {
    # BOM-less UTF-8 via BaseStream — see Invoke-McpRpc for why.
    $line = ($obj | ConvertTo-Json -Compress -Depth 12)
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes("$line`n")
    $script:acp.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
    $script:acp.StandardInput.BaseStream.Flush()
}

function Read-AcpResponse([int]$id, [int]$timeoutMs = 45000) {
    $deadline = (Get-Date).AddMilliseconds($timeoutMs)
    while ((Get-Date) -lt $deadline) {
        # A bare ReadLine() blocks indefinitely when the agent goes quiet, so the
        # deadline above never gets re-checked and the whole suite hangs. Wait on
        # the async read instead, so the timeout is actually enforced.
        $remainingMs = [int](($deadline - (Get-Date)).TotalMilliseconds)
        if ($remainingMs -le 0) { break }
        $read = $acp.StandardOutput.ReadLineAsync()
        if (-not $read.Wait($remainingMs)) { return $null }
        $line = $read.Result
        if ($null -eq $line) { return $null }  # stream closed
        if ($line.Trim() -eq "") { continue }
        try {
            $msg = $line | ConvertFrom-Json
        } catch { continue }

        if ($msg.method -eq "session/update") { $script:gotUpdate = $true }
        if ($msg.method -eq "session/request_permission") {
            $script:permissionSeen = $true
            $opt = $msg.params.options | Where-Object { $_.kind -match "allow" } | Select-Object -First 1
            if (-not $opt) { $opt = $msg.params.options | Select-Object -First 1 }
            Send-Acp @{
                jsonrpc = "2.0"
                id = $msg.id
                result = @{ outcome = @{ outcome = "selected"; optionId = $opt.optionId } }
            }
        }
        if ($msg.id -eq $id) { return $msg }
    }
    return $null
}

Send-Acp @{
    jsonrpc = "2.0"; id = 1; method = "initialize"
    params = @{
        protocolVersion = 1
        clientCapabilities = @{ fs = @{ readTextFile = $true; writeTextFile = $true } }
        clientInfo = @{ name = "swervebuild-test"; version = "0.1.0" }
    }
}
$initResp = Read-AcpResponse 1
Assert-True ($null -ne $initResp.result) "ACP initialize"
$canLoad = ($initResp.result.agentCapabilities.loadSession -eq $true) -or
    ($null -ne $initResp.result.agentCapabilities.sessionCapabilities.resume)

Send-Acp @{
    jsonrpc = "2.0"; id = 2; method = "session/new"
    params = @{
        cwd = $testCwd
        mcpServers = @(@{ name = "swervebuild"; command = $mcpPath; args = @(); env = @() })
    }
}
$newResp = Read-AcpResponse 2
$sessionId = $newResp.result.sessionId
Assert-True ($null -ne $sessionId) "ACP session/new returns sessionId" "$sessionId"

Send-Acp @{
    jsonrpc = "2.0"; id = 3; method = "session/prompt"
    params = @{
        sessionId = $sessionId
        prompt = @(@{ type = "text"; text = "Use swervebuild_list_projects MCP tool. Reply with project names only." })
    }
}
$promptResp = Read-AcpResponse 3 120000
$promptDone = ($null -ne $promptResp)
Assert-True $promptDone "ACP session/prompt completes"
Assert-True $gotUpdate "ACP session/update streaming"

try { $acp.Kill() } catch {}

# 4. Session load (gated the same way as acp.rs: loadSession, or sessionCapabilities.resume)
if ($sessionId -and $canLoad) {
    $acp2 = [System.Diagnostics.Process]::Start($psi)
    function Send-Acp2($obj) {
        # BOM-less UTF-8 via BaseStream — see Invoke-McpRpc for why.
        $line = ($obj | ConvertTo-Json -Compress -Depth 12)
        $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes("$line`n")
        $script:acp2.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
        $script:acp2.StandardInput.BaseStream.Flush()
    }
    function Read-Acp2([int]$id, [int]$timeoutMs = 45000) {
        $deadline = (Get-Date).AddMilliseconds($timeoutMs)
        while ((Get-Date) -lt $deadline) {
            # Same blocking-ReadLine hazard as Read-AcpResponse — enforce the deadline.
            $remainingMs = [int](($deadline - (Get-Date)).TotalMilliseconds)
            if ($remainingMs -le 0) { break }
            $read = $acp2.StandardOutput.ReadLineAsync()
            if (-not $read.Wait($remainingMs)) { return $null }
            $line = $read.Result
            if ($null -eq $line) { return $null }
            if ($line.Trim() -eq "") { continue }
            try { $msg = $line | ConvertFrom-Json } catch { continue }
            if ($msg.id -eq $id) { return $msg }
        }
        return $null
    }

    Send-Acp2 @{
        jsonrpc = "2.0"; id = 1; method = "initialize"
        params = @{
            protocolVersion = 1
            clientCapabilities = @{ fs = @{ readTextFile = $true; writeTextFile = $true } }
            clientInfo = @{ name = "swervebuild-test"; version = "0.1.0" }
        }
    }
    $null = Read-Acp2 1

    Send-Acp2 @{
        jsonrpc = "2.0"; id = 2; method = "session/load"
        params = @{
            sessionId = $sessionId
            cwd = $testCwd
            mcpServers = @(@{ name = "swervebuild"; command = $mcpPath; args = @(); env = @() })
        }
    }
    $loadResp = Read-Acp2 2
    $loaded = ($null -ne $loadResp) -and ($null -eq $loadResp.error)
    Assert-True $loaded "ACP session/load reuses session id" "$(if ($loadResp.error) { $loadResp.error } else { 'ok' })"
    try { $acp2.Kill() } catch {}
} elseif ($sessionId) {
    Write-Host "[SKIP] ACP session/load - agent advertises neither loadSession nor sessionCapabilities.resume" -ForegroundColor DarkYellow
}

# 5. Data store state (machine-dependent: verify when present, skip when fresh)
$dataPath = Get-DataStorePath
$store = if (Test-Path $dataPath) { Get-Content $dataPath -Raw | ConvertFrom-Json } else { $null }
if ($null -ne $store -and $store.projects.Count -gt 0) {
    Assert-True $true "Workspace data.json has projects"
} elseif ($null -ne $store) {
    Write-Host "[SKIP] Workspace data.json has no projects yet" -ForegroundColor DarkYellow
} else {
    Write-Host "[SKIP] Workspace data.json - no local ~/.swervebuild data on this machine" -ForegroundColor DarkYellow
}
if ($null -ne $store -and $store.chats.Count -ge 1) {
    Assert-True $true "Workspace data.json has chats"
} else {
    Write-Host "[SKIP] Workspace data.json has no chats yet" -ForegroundColor DarkYellow
}

Write-Host "`n=== Results: $passed passed, $failed failed ===`n" -ForegroundColor Cyan
if ($failed -gt 0) { exit 1 }
exit 0