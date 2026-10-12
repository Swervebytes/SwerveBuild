//! Hyper-V guest desktop for agent app tests.
//!
//! One human grant (`vm_grant.json`, off by default). The configured guest is
//! reverted to a named snapshot, started, and driven on the logged-on desktop
//! by Windows UI Automation control name. The guest password is DPAPI-wrapped
//! for the current Windows user and is never returned to a tool caller.
//! Unattended automations set `SWERVE_AUTOMATION=1` and cannot drive the guest.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const GRANT_FILE: &str = "vm_grant.json";
const CONFIG_FILE: &str = "vm_guest.json";
const SECRET_FILE: &str = "vm_guest_secret.txt";
const HOST_SCRIPT: &str = "vm-host.ps1";

const LABEL_MAX: usize = 80;
const CONTROL_MAX: usize = 200;
const TEXT_MAX: usize = 2_000;

const HOST_PS1: &str = include_str!("../guest/vm-host.ps1");
const GUEST_PS1: &str = include_str!("../guest/vm-drive.ps1");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmGrant {
    pub granted: bool,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for VmGrant {
    fn default() -> Self {
        Self { granted: false, updated_at: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmConfig {
    pub vm_name: String,
    pub snapshot: String,
    pub username: String,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            vm_name: String::new(),
            snapshot: String::new(),
            username: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmStatus {
    pub granted: bool,
    pub configured: bool,
    pub password_saved: bool,
    pub vm_name: String,
    pub snapshot: String,
    pub username: String,
    pub hypervisor: Option<Value>,
}

fn grant_path() -> PathBuf {
    crate::paths::data_dir().join(GRANT_FILE)
}

fn config_path() -> PathBuf {
    crate::paths::data_dir().join(CONFIG_FILE)
}

fn secret_path() -> PathBuf {
    crate::paths::data_dir().join(SECRET_FILE)
}

pub fn load_grant() -> VmGrant {
    read_json(&grant_path()).unwrap_or_default()
}

pub fn is_granted() -> bool {
    load_grant().granted
}

pub fn set_granted(granted: bool) -> Result<VmGrant, String> {
    let grant = VmGrant { granted, updated_at: crate::store::Store::now() };
    write_json(&grant_path(), &grant)?;
    Ok(grant)
}

pub fn load_config() -> VmConfig {
    read_json(&config_path()).unwrap_or_default()
}

pub fn set_config(vm_name: &str, snapshot: &str, username: &str) -> Result<VmConfig, String> {
    let config = VmConfig {
        vm_name: validate_label(vm_name, "virtual machine name")?,
        snapshot: validate_label(snapshot, "snapshot name")?,
        username: validate_username(username)?,
    };
    write_json(&config_path(), &config)?;
    Ok(config)
}

pub fn password_saved() -> bool {
    secret_path().is_file()
}

/// Store the guest password with Windows DPAPI for the current user.
/// The plain password is piped to PowerShell stdin and is not written to a script.
pub fn store_password(password: &str) -> Result<(), String> {
    let password = password.trim();
    if password.is_empty() {
        return Err("password required".into());
    }
    if password.len() > 256 {
        return Err("password too long".into());
    }
    if password.chars().any(|c| c.is_control()) {
        return Err("password cannot contain control characters".into());
    }
    let path = secret_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let script = format!(
        "$p = [Console]::In.ReadToEnd().TrimEnd(\"`r\", \"`n\"); $s = ConvertTo-SecureString $p -AsPlainText -Force; $s | ConvertFrom-SecureString | Set-Content -LiteralPath '{}' -NoNewline -Encoding ascii",
        path.display().to_string().replace('\'', "''")
    );
    let mut child = powershell()
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start PowerShell: {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("password pipe failed")?;
        stdin.write_all(password.as_bytes()).map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("could not save the guest password: {}", err.trim()));
    }
    Ok(())
}

pub fn status() -> VmStatus {
    let config = load_config();
    let configured = !config.vm_name.is_empty() && !config.snapshot.is_empty() && !config.username.is_empty();
    VmStatus {
        granted: is_granted(),
        configured,
        password_saved: password_saved(),
        vm_name: config.vm_name,
        snapshot: config.snapshot,
        username: config.username,
        hypervisor: None,
    }
}

pub fn probe() -> Result<Value, String> {
    if std::env::var("SWERVE_AUTOMATION").ok().as_deref() == Some("1") {
        return Err("VM test is unavailable to unattended automations.".into());
    }
    let config = require_config()?;
    run_action(&config, "status", "", "", "", None, Duration::from_secs(40))
}

pub fn prepare() -> Result<Value, String> {
    require_drive()?;
    let config = require_config()?;
    run_action(&config, "prepare", "", "", "", None, Duration::from_secs(240))
}

pub fn screenshot() -> Result<Value, String> {
    require_drive()?;
    let config = require_config()?;
    require_password()?;
    let path = artifact_path("png");
    let body = run_action(&config, "screenshot", "", "", "", Some(&path), Duration::from_secs(100))?;
    Ok(json!({ "ok": true, "path": path.display().to_string(), "detail": body }))
}

pub fn click(control_name: &str) -> Result<Value, String> {
    require_drive()?;
    let config = require_config()?;
    require_password()?;
    let name = validate_control(control_name)?;
    let path = artifact_path("png");
    let body = run_action(&config, "invoke", &name, "", "", Some(&path), Duration::from_secs(100))?;
    Ok(json!({ "ok": true, "path": path.display().to_string(), "control": name, "detail": body }))
}

pub fn type_text(control_name: &str, text: &str) -> Result<Value, String> {
    require_drive()?;
    let config = require_config()?;
    require_password()?;
    let name = validate_control(control_name)?;
    let text = validate_text(text)?;
    let path = artifact_path("png");
    let body = run_action(&config, "type", &name, &text, "", Some(&path), Duration::from_secs(100))?;
    Ok(json!({ "ok": true, "path": path.display().to_string(), "control": name, "detail": body }))
}

pub fn press_key(control_name: Option<&str>, key: &str) -> Result<Value, String> {
    require_drive()?;
    let config = require_config()?;
    require_password()?;
    let name = match control_name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => validate_control(raw)?,
        None => String::new(),
    };
    let key = validate_key(key)?;
    let path = artifact_path("png");
    let body = run_action(&config, "press", &name, "", &key, Some(&path), Duration::from_secs(100))?;
    Ok(json!({ "ok": true, "path": path.display().to_string(), "key": key, "detail": body }))
}

pub fn open_console() -> Result<Value, String> {
    let config = require_config()?;
    run_action(&config, "console", "", "", "", None, Duration::from_secs(20))
}

/// Grant gate for agent tools. The Settings page may open the console without it.
pub fn ensure_agent_drive() -> Result<(), String> {
    require_drive()
}

const JOIN_ADMINS_PS1: &str = r#"
$ErrorActionPreference = 'Stop'
$group = 'Hyper-V Administrators'
$me = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$exists = Get-LocalGroupMember -Group $group -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -eq $me }
if (-not $exists) { Add-LocalGroupMember -Group $group -Member $me }
"#;

/// One UAC prompt. Adds the current Windows user to Hyper-V Administrators.
pub fn grant_hypervisor_access() -> Result<String, String> {
    let dir = crate::paths::data_dir().join("vm_work");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script_path = dir.join("join-hyperv-admins.ps1");
    fs::write(&script_path, JOIN_ADMINS_PS1).map_err(|e| e.to_string())?;
    let child = powershell()
        .args([
            "-NoProfile",
            "-Command",
            "Start-Process -FilePath powershell.exe -Verb RunAs -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',$env:SWERVE_VM_JOIN)",
        ])
        .env("SWERVE_VM_JOIN", &script_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not request administrator access: {e}"))?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let detail = err.trim();
        if detail.is_empty() {
            return Err("Hyper-V access was not granted.".into());
        }
        return Err(format!("Hyper-V access was not granted: {detail}"));
    }
    Ok("Approved. Sign out of Windows if Hyper-V still refuses this app.".into())
}

fn require_drive() -> Result<(), String> {
    if std::env::var("SWERVE_AUTOMATION").ok().as_deref() == Some("1") {
        return Err("VM test is unavailable to unattended automations.".into());
    }
    if is_granted() {
        Ok(())
    } else {
        Err("VM test is off. Turn on Settings → Test virtual machine.".into())
    }
}

fn require_config() -> Result<VmConfig, String> {
    let config = load_config();
    if config.vm_name.is_empty() || config.snapshot.is_empty() || config.username.is_empty() {
        return Err("Set the virtual machine, snapshot, and guest user in Settings → Test virtual machine.".into());
    }
    Ok(config)
}

fn require_password() -> Result<(), String> {
    if password_saved() {
        Ok(())
    } else {
        Err("Save the guest password in Settings → Test virtual machine.".into())
    }
}

fn run_action(
    config: &VmConfig,
    action: &str,
    control: &str,
    text: &str,
    key: &str,
    host_out: Option<&Path>,
    timeout: Duration,
) -> Result<Value, String> {
    let dir = crate::paths::data_dir().join("vm_work");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script_path = dir.join(HOST_SCRIPT);
    fs::write(&script_path, HOST_PS1).map_err(|e| e.to_string())?;
    let result_path = dir.join(format!("result-{}.json", crate::store::Store::new_id()));
    let request = json!({
        "action": action,
        "vmName": config.vm_name,
        "snapshot": config.snapshot,
        "username": config.username,
        "secretPath": secret_path(),
        "controlName": control,
        "text": text,
        "key": key,
        "hostOut": host_out.map(|p| p.display().to_string()).unwrap_or_default(),
        "guestScript": GUEST_PS1,
        "resultPath": result_path,
    });
    let request_path = dir.join(format!("request-{}.json", crate::store::Store::new_id()));
    fs::write(&request_path, serde_json::to_vec(&request).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let log_out = dir.join("host-out.log");
    let log_err = dir.join("host-err.log");
    let stdout = File::create(&log_out).map_err(|e| e.to_string())?;
    let stderr = File::create(&log_err).map_err(|e| e.to_string())?;
    let mut child = powershell()
        .args([
            OsStr::new("-NoProfile"),
            OsStr::new("-NonInteractive"),
            OsStr::new("-ExecutionPolicy"),
            OsStr::new("Bypass"),
            OsStr::new("-File"),
            script_path.as_os_str(),
            request_path.as_os_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| format!("could not start PowerShell: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{action} timed out after {}s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    let body = fs::read_to_string(&result_path).unwrap_or_default();
    let parsed: Value = serde_json::from_str(body.trim()).unwrap_or(Value::Null);
    if !status.success() || parsed.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let from_json = parsed.get("error").and_then(|v| v.as_str()).unwrap_or("");
        let err_log = fs::read_to_string(&log_err).unwrap_or_default();
        let detail = if !from_json.is_empty() {
            from_json.to_string()
        } else {
            let t = err_log.trim();
            if t.is_empty() { format!("{action} failed") } else { t.to_string() }
        };
        return Err(detail);
    }
    let _ = fs::remove_file(&request_path);
    Ok(parsed)
}

fn artifact_path(ext: &str) -> PathBuf {
    let dir = crate::paths::data_dir().join("vm_artifacts");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}.{}", crate::store::Store::new_id(), ext))
}

fn powershell() -> Command {
    crate::util::hidden_command("powershell.exe")
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(path, raw.as_bytes()).map_err(|e| e.to_string())
}

pub fn validate_label(raw: &str, what: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() || s.len() > LABEL_MAX {
        return Err(format!("{what} is required (max {LABEL_MAX} characters)"));
    }
    if s.chars().any(|c| !(c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.')) {
        return Err(format!("{what} can use letters, numbers, spaces, and . _ -"));
    }
    Ok(s.to_string())
}

pub fn validate_username(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() || s.len() > LABEL_MAX {
        return Err(format!("guest user is required (max {LABEL_MAX} characters)"));
    }
    if s.chars().any(|c| {
        !(c.is_ascii_alphanumeric() || matches!(c, '\\' | '@' | '.' | '-' | '_'))
    }) {
        return Err("guest user can use letters, numbers, and \\ @ . _ -".into());
    }
    Ok(s.to_string())
}

pub fn validate_control(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() || s.len() > CONTROL_MAX {
        return Err(format!("control name is required (max {CONTROL_MAX} characters)"));
    }
    if s.chars().any(|c| c.is_control()) {
        return Err("control name cannot contain control characters".into());
    }
    Ok(s.to_string())
}

pub fn validate_text(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.len() > TEXT_MAX {
        return Err(format!("text is required (max {TEXT_MAX} characters)"));
    }
    if raw.chars().any(|c| c == '\0' || c == '\r' || c == '\n') {
        return Err("text cannot contain line breaks".into());
    }
    Ok(raw.to_string())
}

pub fn validate_key(raw: &str) -> Result<String, String> {
    match raw.trim() {
        "Enter" | "Tab" | "Escape" | "Backspace" | "Space" | "Up" | "Down" | "Left" | "Right" => {
            Ok(raw.trim().to_string())
        }
        _ => Err("key must be Enter, Tab, Escape, Backspace, Space, Up, Down, Left, or Right".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_reject_shell_metacharacters() {
        assert!(validate_label("Win Test", "virtual machine name").is_ok());
        assert!(validate_label("clean-user", "snapshot name").is_ok());
        assert!(validate_label(r#"win"; Stop-VM"#, "virtual machine name").is_err());
        assert!(validate_label("", "snapshot name").is_err());
    }

    #[test]
    fn username_allows_a_local_account() {
        assert_eq!(validate_username(r"vm\tester").unwrap(), r"vm\tester");
        assert!(validate_username("tester;admin").is_err());
    }

    #[test]
    fn keys_are_an_allowlist() {
        assert!(validate_key("Enter").is_ok());
        assert!(validate_key("ctrl").is_err());
    }

    #[test]
    fn scripts_drive_a_snapshot_and_do_not_embed_a_password() {
        assert!(HOST_PS1.contains("Restore-VMSnapshot"));
        assert!(HOST_PS1.contains("Copy-VMFile"));
        assert!(!HOST_PS1.to_lowercase().contains("password"));
        assert!(GUEST_PS1.contains("InvokePattern"));
        assert!(GUEST_PS1.contains("Save-Shot"));
    }
}
