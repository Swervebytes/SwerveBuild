//! Terminal MCP surface (Roadmap Step 6) — grant-gated one-shot command runner.
//!
//! Safety mirrors `app_ui`: a human Settings grant (off by default) is the
//! choke point; every run is confined to the open project via the junction-safe
//! check from `acp.rs`; per-stream output is size-capped and truncation-flagged;
//! the child is tree-killed on timeout; command + result are logged.
//!
//! Scope is deliberately ONE-SHOT. Persistent shell sessions do NOT live in the
//! sidecar — it is spawned per ACP connection and dies with it, so session state
//! could neither persist nor be shared. See `design/terminal-tools.md`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const GRANT_FILE: &str = "term_grant.json";
const LOG_FILE: &str = "term_runs.jsonl";

/// Per-stream (stdout/stderr) output cap in bytes; overflow sets `truncated`.
const OUTPUT_CAP_BYTES: usize = 96 * 1024;
/// Longest command string accepted.
const COMMAND_MAX: usize = 8_000;
/// Run timeout bounds (seconds).
const TIMEOUT_DEFAULT_SECS: u64 = 30;
const TIMEOUT_MAX_SECS: u64 = 120;
/// Run-log lines retained (newest kept).
const LOG_KEEP: usize = 200;

// --------------------------------------------------------------------- grant

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermGrant {
    pub granted: bool,
    /// Timestamp string from the app (display / audit only).
    #[serde(default)]
    pub updated_at: String,
}

impl Default for TermGrant {
    fn default() -> Self {
        Self { granted: false, updated_at: String::new() }
    }
}

fn grant_path() -> PathBuf {
    crate::paths::data_dir().join(GRANT_FILE)
}

pub fn load_grant() -> TermGrant {
    let path = grant_path();
    if !path.is_file() {
        return TermGrant::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn is_granted() -> bool {
    load_grant().granted
}

pub fn set_granted(granted: bool) -> Result<TermGrant, String> {
    let grant = TermGrant { granted, updated_at: crate::store::Store::now() };
    let path = grant_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(&grant).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())?;
    Ok(grant)
}

/// Pure gate — split out so grant logic is unit-testable without touching the
/// real data dir (the message is the human-facing denial).
fn grant_gate(granted: bool) -> Result<(), String> {
    if granted {
        Ok(())
    } else {
        Err("terminal not granted. Human must enable \"Allow agent to run terminal commands\" in Settings → Agent terminal.".into())
    }
}

pub fn require_grant() -> Result<(), String> {
    grant_gate(is_granted())
}

// ---------------------------------------------------------------- shell + cwd

#[derive(Debug, Clone, Copy, PartialEq)]
enum ShellKind {
    PowerShell,
    Cmd,
}

impl ShellKind {
    /// Windows default is PowerShell; `cmd` is the fallback (per design doc).
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("powershell") | Some("pwsh") | Some("ps") => Ok(ShellKind::PowerShell),
            Some("cmd") | Some("cmd.exe") => Ok(ShellKind::Cmd),
            Some(other) => Err(format!("unknown shell: {other} (use powershell or cmd)")),
        }
    }

    fn exe(&self) -> &'static str {
        match self {
            ShellKind::PowerShell => "powershell.exe",
            ShellKind::Cmd => "cmd.exe",
        }
    }
}

fn build_command(shell: ShellKind, command: &str) -> Command {
    let mut cmd = crate::util::hidden_command(shell.exe());
    match shell {
        // -NonInteractive so a prompt can't hang the child; -NoProfile for a
        // predictable, fast environment.
        ShellKind::PowerShell => {
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        }
        ShellKind::Cmd => {
            cmd.args(["/C", command]);
        }
    }
    cmd
}

fn validate_command(command: &str) -> Result<&str, String> {
    let c = command.trim();
    if c.is_empty() {
        return Err("command required".into());
    }
    if c.len() > COMMAND_MAX {
        return Err(format!("command too long (max {COMMAND_MAX} chars)"));
    }
    Ok(c)
}

/// The most-recently-opened project's folder, or the one named by `project_id`.
fn resolve_project_root(project_id: Option<&str>) -> Result<PathBuf, String> {
    let store = crate::store::Store::load();
    let project = match project_id {
        Some(id) => store
            .projects
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("project not found: {id}"))?,
        None => store
            .projects
            .iter()
            .max_by(|a, b| a.last_opened_at.cmp(&b.last_opened_at))
            .ok_or_else(|| "no project is open — open a project first".to_string())?,
    };
    let path = project.path.trim();
    if path.is_empty() {
        return Err("the open project has no folder path".into());
    }
    if !Path::new(path).is_dir() {
        return Err(format!("project folder not found on disk: {path}"));
    }
    Ok(PathBuf::from(path))
}

/// Confine an optional sub-path to the project root using the junction-safe
/// check shared with the ACP file tools (audit A4). `None`/"" → the root itself
/// (canonicalized). Escapes return an error, never a path outside the project.
fn confine_cwd(root: &Path, sub: Option<&str>) -> Result<PathBuf, String> {
    let requested = match sub.map(str::trim) {
        None | Some("") => ".",
        Some(s) => s,
    };
    crate::acp::confine_to_cwd(requested, &root.to_string_lossy())
        .ok_or_else(|| format!("cwd escapes the project folder: {requested}"))
}

// ------------------------------------------------------------------- reading

/// Read a child stream up to `cap` bytes; report whether more was discarded.
fn read_capped(mut stream: impl Read, cap: usize) -> (String, bool) {
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() >= cap {
                    truncated = true;
                    continue; // keep draining so the pipe never blocks the child
                }
                let room = cap - buf.len();
                let take = room.min(n);
                buf.extend_from_slice(&chunk[..take]);
                if take < n {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&buf).into_owned(), truncated)
}

// ------------------------------------------------------------------- run

/// Run one command to completion in the open project. Tool-level failures
/// (grant, confinement, spawn) are `Err`; a non-zero exit or a timeout is a
/// normal `Ok` result the agent branches on (`ok: false`).
pub fn run_command(
    command: &str,
    project_id: Option<&str>,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    shell: Option<&str>,
) -> Result<Value, String> {
    require_grant()?;
    let command = validate_command(command)?;
    let shell = ShellKind::parse(shell)?;
    let root = resolve_project_root(project_id)?;
    let workdir = confine_cwd(&root, cwd)?;
    let timeout = timeout_secs.unwrap_or(TIMEOUT_DEFAULT_SECS).clamp(1, TIMEOUT_MAX_SECS);

    let mut child = build_command(shell, command)
        .current_dir(&workdir)
        .stdin(Stdio::null()) // never pipe stdin — a CLI reading to EOF would hang
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to launch {}: {e}", shell.exe()))?;
    let pid = child.id();

    // Drain both streams on their own threads so a full pipe can't deadlock the
    // child while we wait, and so the size cap is applied as data arrives.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (otx, orx) = mpsc::channel();
    let (etx, erx) = mpsc::channel();
    thread::spawn(move || {
        let r = stdout.map(|s| read_capped(s, OUTPUT_CAP_BYTES)).unwrap_or_default();
        let _ = otx.send(r);
    });
    thread::spawn(move || {
        let r = stderr.map(|s| read_capped(s, OUTPUT_CAP_BYTES)).unwrap_or_default();
        let _ = etx.send(r);
    });

    let start = Instant::now();
    let deadline = start + Duration::from_secs(timeout);
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) => {
                if Instant::now() >= deadline {
                    crate::jobs::tree_kill(pid);
                    let _ = child.wait();
                    break (None, true);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("waiting on child: {e}")),
        }
    };

    let (stdout_s, out_trunc) = orx.recv_timeout(Duration::from_secs(3)).unwrap_or_default();
    let (stderr_s, err_trunc) = erx.recv_timeout(Duration::from_secs(3)).unwrap_or_default();
    let duration_ms = start.elapsed().as_millis() as u64;
    let exit_code = status.as_ref().and_then(std::process::ExitStatus::code);

    let result = json!({
        "ok": !timed_out && exit_code == Some(0),
        "exitCode": exit_code,
        "timedOut": timed_out,
        "stdout": stdout_s,
        "stderr": stderr_s,
        "truncated": out_trunc || err_trunc,
        "durationMs": duration_ms,
        "cwd": workdir.to_string_lossy(),
        "shell": shell.exe(),
        "command": command,
    });
    log_run(&result);
    Ok(result)
}

/// Append a compact record (no output payload) to the rolling run log.
fn log_run(result: &Value) {
    let path = crate::paths::data_dir().join(LOG_FILE);
    let entry = json!({
        "at": crate::store::Store::now(),
        "command": result.get("command"),
        "cwd": result.get("cwd"),
        "shell": result.get("shell"),
        "exitCode": result.get("exitCode"),
        "timedOut": result.get("timedOut"),
        "truncated": result.get("truncated"),
        "durationMs": result.get("durationMs"),
    });
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .map(|raw| raw.lines().map(String::from).collect())
        .unwrap_or_default();
    lines.push(entry.to_string());
    let keep = lines.len().saturating_sub(LOG_KEEP);
    let body = lines[keep..].join("\n");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = crate::paths::write_atomic(&path, format!("{body}\n").as_bytes());
}

/// Structured status for MCP `term_state`: grant, resolved project, defaults.
pub fn state_report() -> Value {
    let grant = load_grant();
    let root = resolve_project_root(None).ok();
    json!({
        "granted": grant.granted,
        "grantUpdatedAt": grant.updated_at,
        "projectRoot": root.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "defaults": {
            "shell": "powershell.exe",
            "timeoutSecs": TIMEOUT_DEFAULT_SECS,
            "timeoutMaxSecs": TIMEOUT_MAX_SECS,
            "outputCapBytes": OUTPUT_CAP_BYTES,
        },
        "tools": {
            "term_state": "available",
            "term_run": if grant.granted { "available" } else { "not_granted (enable in Settings → Agent terminal)" },
        },
        "note": "One-shot runner. Persistent shell sessions are not in the sidecar; see design/terminal-tools.md.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grant_is_denied() {
        let g = TermGrant::default();
        assert!(!g.granted);
        let raw = serde_json::to_string(&g).unwrap();
        assert!(raw.contains("\"granted\":false") || raw.contains("\"granted\": false"));
    }

    #[test]
    fn grant_gate_allows_only_when_granted() {
        assert!(grant_gate(true).is_ok());
        let err = grant_gate(false).unwrap_err();
        assert!(err.contains("not granted"), "got: {err}");
        assert!(err.contains("Settings"), "message should point at Settings: {err}");
    }

    #[test]
    fn shell_parse_defaults_to_powershell() {
        assert_eq!(ShellKind::parse(None).unwrap(), ShellKind::PowerShell);
        assert_eq!(ShellKind::parse(Some("")).unwrap(), ShellKind::PowerShell);
        assert_eq!(ShellKind::parse(Some("pwsh")).unwrap(), ShellKind::PowerShell);
        assert_eq!(ShellKind::parse(Some("CMD")).unwrap(), ShellKind::Cmd);
        assert!(ShellKind::parse(Some("bash")).is_err());
    }

    #[test]
    fn powershell_args_are_noninteractive() {
        let cmd = build_command(ShellKind::PowerShell, "echo hi");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"-NonInteractive".to_string()));
        assert!(args.contains(&"-NoProfile".to_string()));
        assert_eq!(args.last().unwrap(), "echo hi");
    }

    #[test]
    fn validate_command_rejects_empty_and_overlong() {
        assert!(validate_command("   ").is_err());
        assert_eq!(validate_command("  git status ").unwrap(), "git status");
        let long = "x".repeat(COMMAND_MAX + 1);
        assert!(validate_command(&long).is_err());
    }

    #[test]
    fn read_capped_flags_truncation() {
        let (s, trunc) = read_capped(std::io::Cursor::new(b"hello".to_vec()), 1024);
        assert_eq!(s, "hello");
        assert!(!trunc);

        let big = vec![b'a'; 5000];
        let (s, trunc) = read_capped(std::io::Cursor::new(big), 100);
        assert_eq!(s.len(), 100);
        assert!(trunc);
    }

    #[test]
    fn confine_cwd_keeps_inside_and_blocks_escape() {
        // Real dirs are required — confine canonicalizes (junction-safe).
        let root = std::env::temp_dir().join(format!("swerve-term-{}", uuid::Uuid::new_v4()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        let inside = confine_cwd(&root, Some("sub")).expect("sub is inside");
        assert!(inside.to_string_lossy().to_lowercase().contains("sub"));

        // Root itself for None/empty.
        assert!(confine_cwd(&root, None).is_ok());

        // Escapes are refused.
        assert!(confine_cwd(&root, Some("..")).is_err());
        assert!(confine_cwd(&root, Some("../../Windows")).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
