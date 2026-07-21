//! Terminal MCP surface (Roadmap Step 6).
//!
//! Two levels, one grant (`term_grant`, off by default — the choke point):
//! - **One-shot** `term_run`: spawn → wait → capture → return, sidecar-resident.
//! - **Persistent sessions** (S11): live PowerShell REPLs owned by the app
//!   process (`SessionManager`), reachable from the per-connection sidecar over a
//!   loopback control server. See `design/terminal-tools.md`.
//!
//! Safety mirrors `app_ui`: the human grant gates everything; cwd is confined via
//! the junction-safe check from `acp.rs`; output is size-capped and
//! truncation-flagged; children are tree-killed on timeout / app exit.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
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
    let control = load_control();
    json!({
        "granted": grant.granted,
        "grantUpdatedAt": grant.updated_at,
        "projectRoot": root.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "controlReady": control.is_some(),
        "controlNote": if control.is_some() {
            "Persistent-session control server published; term_start/exec/read available when granted."
        } else {
            "No control server — start/restart SwerveBuild to host persistent sessions."
        },
        "defaults": {
            "shell": "powershell.exe",
            "timeoutSecs": TIMEOUT_DEFAULT_SECS,
            "timeoutMaxSecs": TIMEOUT_MAX_SECS,
            "outputCapBytes": OUTPUT_CAP_BYTES,
        },
        "tools": {
            "term_state": "available",
            "term_run": if grant.granted { "available" } else { "not_granted (enable in Settings → Agent terminal)" },
            "term_start": if grant.granted && control.is_some() { "available" } else { "not_granted_or_no_app" },
            "term_exec": if grant.granted && control.is_some() { "available" } else { "not_granted_or_no_app" },
            "term_read": if grant.granted && control.is_some() { "available" } else { "not_granted_or_no_app" },
        },
        "note": "One-shot term_run is sidecar-resident; persistent sessions are hosted by the app process and proxied. See design/terminal-tools.md.",
    })
}

// ==================================================================
// Persistent sessions (S11) — app-hosted live PowerShell REPLs, reached
// from the per-connection sidecar over a loopback control server.
// ==================================================================

const CONTROL_FILE: &str = "term_control.json";
/// Per-exec captured output cap (bytes).
const SESSION_OUTPUT_CAP: usize = 96 * 1024;
/// Per-session cumulative buffer cap; older bytes drop (paging base advances).
const SESSION_BUFFER_CAP: usize = 512 * 1024;
const EXEC_TIMEOUT_DEFAULT_SECS: u64 = 30;
const EXEC_TIMEOUT_MAX_SECS: u64 = 300;
/// A redirected-stdin PowerShell REPL prints its prompt AND echoes each input
/// line. We set the prompt to this unique token so every echoed input line is
/// prefixed with it and can be stripped from captured output, leaving only real
/// command output.
const PROMPT_TOKEN: &str = "<<PSPROMPT>>";

/// Loopback control endpoint the app publishes and the sidecar dials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermControl {
    pub host: String,
    pub port: u16,
    pub pid: u32,
    pub token: String,
    #[serde(default)]
    pub updated_at: String,
}

fn control_path() -> PathBuf {
    crate::paths::data_dir().join(CONTROL_FILE)
}

pub fn load_control() -> Option<TermControl> {
    let path = control_path();
    if !path.is_file() {
        return None;
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn publish_control(port: u16, token: &str) -> Result<(), String> {
    let ctrl = TermControl {
        host: "127.0.0.1".into(),
        port,
        pid: std::process::id(),
        token: token.to_string(),
        updated_at: crate::store::Store::now(),
    };
    let path = control_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(&ctrl).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())
}

// ---- pure helpers (unit-tested without a process) ----

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn trim_trailing_newlines(b: &[u8]) -> &[u8] {
    let mut end = b.len();
    while end > 0 && (b[end - 1] == b'\n' || b[end - 1] == b'\r') {
        end -= 1;
    }
    &b[..end]
}

/// Find the EXECUTED end marker `<<TERMEND <marker>=<code>>>` in `buf` and return
/// the raw bytes that preceded it plus the parsed exit code. `None` until the
/// marker arrives. The marker is emitted via string concatenation
/// (`"<<TERM"+"END "+…`) so the shell's echo of that command — which contains the
/// pieces but not the contiguous needle — can never false-match.
fn extract_marked_output(buf: &[u8], marker: &str) -> Option<(Vec<u8>, i32)> {
    let needle = format!("<<TERMEND {marker}=");
    let pos = find_sub(buf, needle.as_bytes())?;
    let after = &buf[pos + needle.len()..];
    let end = find_sub(after, b">>")?;
    let code = std::str::from_utf8(&after[..end])
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(-1);
    Some((buf[..pos].to_vec(), code))
}

/// Strip the REPL's echoed input lines (prefixed with [`PROMPT_TOKEN`]) from a
/// captured region, leaving just real command output.
fn clean_output(raw: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(raw);
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with(PROMPT_TOKEN))
        .collect();
    trim_trailing_newlines(kept.join("\n").as_bytes()).to_vec()
}

fn cap_bytes(mut b: Vec<u8>, cap: usize) -> (Vec<u8>, bool) {
    if b.len() > cap {
        b.truncate(cap);
        (b, true)
    } else {
        (b, false)
    }
}

/// Cumulative session output with a bounded window and a logical base offset so
/// `term_read` paging survives front-dropping.
struct BufState {
    data: Vec<u8>,
    base: usize,
    truncated: bool,
}

impl BufState {
    fn logical_len(&self) -> usize {
        self.base + self.data.len()
    }
    fn append(&mut self, bytes: &[u8], cap: usize) {
        self.data.extend_from_slice(bytes);
        if self.data.len() > cap {
            let drop = self.data.len() - cap;
            self.data.drain(0..drop);
            self.base += drop;
            self.truncated = true;
        }
    }
    fn slice_from(&self, from: usize) -> Vec<u8> {
        let phys = from.saturating_sub(self.base).min(self.data.len());
        self.data[phys..].to_vec()
    }
}

struct SessionShared {
    buf: Mutex<BufState>,
    cv: Condvar,
    alive: AtomicBool,
}

struct SessionHandle {
    id: String,
    shell: &'static str,
    start_cwd: String,
    pid: u32,
    stdin: Mutex<std::process::ChildStdin>,
    child: Mutex<std::process::Child>,
    shared: Arc<SessionShared>,
}

fn spawn_reader(mut stream: impl Read + Send + 'static, shared: Arc<SessionShared>) {
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut b) = shared.buf.lock() {
                        b.append(&chunk[..n], SESSION_BUFFER_CAP);
                    }
                    shared.cv.notify_all();
                }
                Err(_) => break,
            }
        }
        shared.alive.store(false, Ordering::SeqCst);
        shared.cv.notify_all();
    });
}

fn spawn_session(cwd: &Path) -> Result<Arc<SessionHandle>, String> {
    // v1 persistent shell is PowerShell only — one marker/capture convention.
    let mut cmd = crate::util::hidden_command("powershell.exe");
    cmd.args(["-NoProfile", "-NoLogo"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("failed to launch powershell: {e}"))?;
    let pid = child.id();
    let stdin = child.stdin.take().ok_or("no stdin on shell")?;
    let stdout = child.stdout.take().ok_or("no stdout on shell")?;
    let stderr = child.stderr.take().ok_or("no stderr on shell")?;
    let shared = Arc::new(SessionShared {
        buf: Mutex::new(BufState { data: Vec::new(), base: 0, truncated: false }),
        cv: Condvar::new(),
        alive: AtomicBool::new(true),
    });
    spawn_reader(stdout, shared.clone());
    spawn_reader(stderr, shared.clone());
    let handle = Arc::new(SessionHandle {
        id: format!("t-{}", uuid::Uuid::new_v4().simple()),
        shell: "powershell.exe",
        start_cwd: cwd.to_string_lossy().into_owned(),
        pid,
        stdin: Mutex::new(stdin),
        child: Mutex::new(child),
        shared,
    });
    // Tag the prompt with a unique token (so echoed input lines are strippable)
    // and silence progress bars.
    if let Ok(mut si) = handle.stdin.lock() {
        let _ = writeln!(si, "function prompt {{ '{PROMPT_TOKEN}' }}; $ProgressPreference='SilentlyContinue'");
        let _ = si.flush();
    }
    Ok(handle)
}

fn kill_handle(handle: &SessionHandle) {
    crate::jobs::tree_kill(handle.pid);
    if let Ok(mut c) = handle.child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    handle.shared.alive.store(false, Ordering::SeqCst);
    handle.shared.cv.notify_all();
}

/// One live command against a session: writes the command + a unique end marker,
/// waits for the marker, returns the output captured since it plus the exit code.
fn exec_on(handle: &Arc<SessionHandle>, command: &str, timeout: u64) -> Result<Value, String> {
    if !handle.shared.alive.load(Ordering::SeqCst) {
        return Err("session is not alive (the shell exited)".into());
    }
    let command = command.trim();
    if command.is_empty() {
        return Err("command required".into());
    }
    let marker = uuid::Uuid::new_v4().simple().to_string();
    let start = handle.shared.buf.lock().map_err(|_| "buf lock")?.logical_len();
    {
        let mut si = handle.stdin.lock().map_err(|_| "stdin lock")?;
        writeln!(si, "{command}").map_err(|e| format!("write to shell: {e}"))?;
        // Emit the end marker via string concat so the shell's echo of THIS line
        // (which shows the pieces, not the joined needle) can't false-match.
        writeln!(
            si,
            "Write-Output (\"<<TERM\"+\"END \"+\"{marker}=\"+$(if($null -ne $LASTEXITCODE){{$LASTEXITCODE}}else{{0}})+\">>\")"
        )
        .map_err(|e| format!("write marker: {e}"))?;
        si.flush().map_err(|e| format!("flush shell: {e}"))?;
    }

    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        let slice = handle.shared.buf.lock().map_err(|_| "buf lock")?.slice_from(start);
        if let Some((raw, code)) = extract_marked_output(&slice, &marker) {
            let (capped, truncated) = cap_bytes(clean_output(&raw), SESSION_OUTPUT_CAP);
            return Ok(json!({
                "ok": code == 0,
                "sessionId": handle.id,
                "exitCode": code,
                "output": String::from_utf8_lossy(&capped),
                "truncated": truncated,
                "timedOut": false,
                "seq": start,
            }));
        }
        if !handle.shared.alive.load(Ordering::SeqCst) {
            // Shell died mid-command — return what we have rather than hang.
            let (capped, truncated) = cap_bytes(clean_output(&slice), SESSION_OUTPUT_CAP);
            return Ok(json!({
                "ok": false, "sessionId": handle.id, "exitCode": null,
                "output": String::from_utf8_lossy(&capped), "truncated": truncated,
                "timedOut": false, "shellExited": true, "seq": start,
            }));
        }
        if Instant::now() >= deadline {
            kill_handle(handle); // a hung command wedges the single REPL — kill it
            let (capped, truncated) = cap_bytes(clean_output(&slice), SESSION_OUTPUT_CAP);
            return Ok(json!({
                "ok": false, "sessionId": handle.id, "exitCode": null,
                "output": String::from_utf8_lossy(&capped), "truncated": truncated,
                "timedOut": true, "seq": start,
            }));
        }
        let guard = handle.shared.buf.lock().map_err(|_| "buf lock")?;
        let _ = handle.shared.cv.wait_timeout(guard, Duration::from_millis(100));
    }
}

/// The app-process owner of live shells. `Arc`-managed Tauri state, like
/// `JobManager`. The `token` authorizes control-server callers.
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<SessionHandle>>>,
    token: String,
}

impl Default for SessionManager {
    fn default() -> Self {
        SessionManager {
            sessions: Mutex::new(HashMap::new()),
            token: uuid::Uuid::new_v4().simple().to_string(),
        }
    }
}

impl SessionManager {
    pub fn token(&self) -> &str {
        &self.token
    }

    fn get(&self, id: &str) -> Option<Arc<SessionHandle>> {
        self.sessions.lock().ok()?.get(id).cloned()
    }

    fn op_start(&self, cwd: Option<&str>, shell: Option<&str>) -> Result<Value, String> {
        if let Some(s) = shell {
            if ShellKind::parse(Some(s))? != ShellKind::PowerShell {
                return Err("persistent sessions support powershell only in v1".into());
            }
        }
        let root = resolve_project_root(None)?;
        let workdir = confine_cwd(&root, cwd)?;
        let handle = spawn_session(&workdir)?;
        let out = json!({
            "ok": true,
            "sessionId": handle.id,
            "cwd": handle.start_cwd,
            "shell": handle.shell,
        });
        self.sessions.lock().map_err(|_| "sessions lock")?.insert(handle.id.clone(), handle);
        Ok(out)
    }

    fn op_exec(&self, id: &str, command: &str, timeout: Option<u64>) -> Result<Value, String> {
        let handle = self.get(id).ok_or_else(|| format!("no such session: {id}"))?;
        let t = timeout.unwrap_or(EXEC_TIMEOUT_DEFAULT_SECS).clamp(1, EXEC_TIMEOUT_MAX_SECS);
        let result = exec_on(&handle, command, t)?;
        // A timed-out session was killed; drop it from the registry.
        if result.get("timedOut").and_then(|v| v.as_bool()) == Some(true) {
            self.sessions.lock().ok().map(|mut g| g.remove(id));
        }
        Ok(result)
    }

    fn op_read(&self, id: &str, offset: Option<usize>) -> Result<Value, String> {
        let handle = self.get(id).ok_or_else(|| format!("no such session: {id}"))?;
        let guard = handle.shared.buf.lock().map_err(|_| "buf lock")?;
        let from = offset.unwrap_or(guard.base).max(guard.base);
        let chunk = guard.slice_from(from);
        Ok(json!({
            "ok": true,
            "sessionId": id,
            "chunk": String::from_utf8_lossy(&chunk),
            "base": guard.base,
            "nextOffset": guard.logical_len(),
            "truncated": guard.truncated,
            "atEnd": !handle.shared.alive.load(Ordering::SeqCst),
        }))
    }

    fn op_close(&self, id: &str) -> Result<Value, String> {
        if let Some(handle) = self.sessions.lock().map_err(|_| "sessions lock")?.remove(id) {
            kill_handle(&handle);
            Ok(json!({ "ok": true, "sessionId": id, "closed": true }))
        } else {
            Ok(json!({ "ok": true, "sessionId": id, "closed": false, "note": "no such session" }))
        }
    }

    fn op_list(&self) -> Value {
        let sessions = match self.sessions.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "ok": true, "sessions": [] }),
        };
        let list: Vec<Value> = sessions
            .values()
            .map(|h| {
                let bytes = h.shared.buf.lock().map(|b| b.logical_len()).unwrap_or(0);
                json!({
                    "sessionId": h.id,
                    "cwd": h.start_cwd,
                    "shell": h.shell,
                    "alive": h.shared.alive.load(Ordering::SeqCst),
                    "bytes": bytes,
                })
            })
            .collect();
        json!({ "ok": true, "sessions": list })
    }

    /// Kill every live shell — joined into the app-exit path so no `powershell.exe`
    /// tree is orphaned.
    pub fn kill_all(&self) {
        if let Ok(mut g) = self.sessions.lock() {
            for (_, handle) in g.drain() {
                kill_handle(&handle);
            }
        }
    }
}

/// Start the loopback control server and publish `term_control.json`. Call once
/// from the app's `setup()`. Returns the bound port.
pub fn serve(manager: Arc<SessionManager>) -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind control server: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    publish_control(port, manager.token())?;
    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(stream) = stream {
                let mgr = manager.clone();
                thread::spawn(move || {
                    let _ = handle_conn(&mgr, stream);
                });
            }
        }
    });
    Ok(port)
}

fn handle_conn(mgr: &SessionManager, stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(EXEC_TIMEOUT_MAX_SECS + 30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let resp = dispatch(mgr, line.trim());
    let mut w = stream;
    writeln!(w, "{resp}")?;
    w.flush()?;
    Ok(())
}

/// App-side dispatch — the security choke point: token check + grant check gate
/// every session op.
fn dispatch(mgr: &SessionManager, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": format!("bad request json: {e}") }),
    };
    if req.get("token").and_then(|v| v.as_str()) != Some(mgr.token()) {
        return json!({ "ok": false, "error": "bad or missing control token" });
    }
    if let Err(e) = require_grant() {
        return json!({ "ok": false, "error": e });
    }
    let op = req.get("op").and_then(|v| v.as_str()).unwrap_or("");
    let id = req.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
    let result = match op {
        "start" => mgr.op_start(
            req.get("cwd").and_then(|v| v.as_str()),
            req.get("shell").and_then(|v| v.as_str()),
        ),
        "exec" => {
            let command = req.get("command").and_then(|v| v.as_str()).unwrap_or("");
            mgr.op_exec(id, command, req.get("timeoutSecs").and_then(|v| v.as_u64()))
        }
        "read" => mgr.op_read(id, req.get("offset").and_then(|v| v.as_u64()).map(|x| x as usize)),
        "close" => mgr.op_close(id),
        "list" => Ok(mgr.op_list()),
        other => Err(format!("unknown op: {other}")),
    };
    result.unwrap_or_else(|e| json!({ "ok": false, "error": e }))
}

// ---- sidecar side: proxy client + tool wrappers ----

/// Dial the app's control server, forward one request, return one reply. Injects
/// the token from `term_control.json`.
fn proxy_call(mut request: Value) -> Result<Value, String> {
    let ctrl = load_control()
        .ok_or("terminal control server not running — open SwerveBuild to host persistent sessions")?;
    request["token"] = json!(ctrl.token);
    let addr: std::net::SocketAddr = format!("{}:{}", ctrl.host, ctrl.port)
        .parse()
        .map_err(|e| format!("bad control address: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("connect control server: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(EXEC_TIMEOUT_MAX_SECS + 20)))
        .ok();
    writeln!(stream, "{request}").map_err(|e| format!("send request: {e}"))?;
    stream.flush().ok();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| format!("read reply: {e}"))?;
    serde_json::from_str(&line).map_err(|e| format!("bad control reply: {e}"))
}

pub fn session_start(cwd: Option<&str>, shell: Option<&str>) -> Result<Value, String> {
    require_grant()?;
    let mut req = json!({ "op": "start" });
    if let Some(c) = cwd {
        req["cwd"] = json!(c);
    }
    if let Some(s) = shell {
        req["shell"] = json!(s);
    }
    proxy_call(req)
}

pub fn session_exec(session_id: &str, command: &str, timeout_secs: Option<u64>) -> Result<Value, String> {
    require_grant()?;
    let mut req = json!({ "op": "exec", "sessionId": session_id, "command": command });
    if let Some(t) = timeout_secs {
        req["timeoutSecs"] = json!(t);
    }
    proxy_call(req)
}

pub fn session_read(session_id: &str, offset: Option<u64>) -> Result<Value, String> {
    require_grant()?;
    let mut req = json!({ "op": "read", "sessionId": session_id });
    if let Some(o) = offset {
        req["offset"] = json!(o);
    }
    proxy_call(req)
}

pub fn session_close(session_id: &str) -> Result<Value, String> {
    require_grant()?;
    proxy_call(json!({ "op": "close", "sessionId": session_id }))
}

pub fn session_list() -> Result<Value, String> {
    require_grant()?;
    proxy_call(json!({ "op": "list" }))
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

    // ---- persistent session helpers (S11) ----

    #[test]
    fn find_sub_locates_and_misses() {
        assert_eq!(find_sub(b"hello world", b"world"), Some(6));
        assert_eq!(find_sub(b"hello", b"xyz"), None);
        assert_eq!(find_sub(b"ab", b""), None);
    }

    #[test]
    fn extract_marked_output_pulls_output_and_code() {
        let m = "abc123";
        let buf = b"line one\nline two\n<<TERMEND abc123=0>>\n";
        let (raw, code) = extract_marked_output(buf, m).expect("marker present");
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&clean_output(&raw)), "line one\nline two");
    }

    #[test]
    fn extract_marked_output_parses_nonzero_and_waits() {
        // Absent until the marker arrives.
        assert!(extract_marked_output(b"partial output no marker yet", "zz").is_none());
        let (raw, code) = extract_marked_output(b"boom\n<<TERMEND zz=2>>", "zz").unwrap();
        assert_eq!(code, 2);
        assert_eq!(String::from_utf8_lossy(&clean_output(&raw)), "boom");
    }

    #[test]
    fn extract_marked_output_ignores_other_markers() {
        // A different exec's marker must not satisfy this exec.
        let buf = b"out\n<<TERMEND other=0>>\nmore\n<<TERMEND mine=5>>";
        let (raw, code) = extract_marked_output(buf, "mine").expect("mine present");
        assert_eq!(code, 5);
        assert!(String::from_utf8_lossy(&raw).contains("more"));
    }

    #[test]
    fn clean_output_strips_echoed_prompt_lines() {
        // Echoed input lines carry the prompt token; real output does not.
        let raw = b"<<PSPROMPT>>Get-Location\nE:\\proj\n<<PSPROMPT>>Write-Output x\n";
        assert_eq!(String::from_utf8_lossy(&clean_output(raw)), "E:\\proj");
    }

    #[test]
    fn bufstate_pages_and_drops_front_with_base() {
        let mut b = BufState { data: Vec::new(), base: 0, truncated: false };
        b.append(b"0123456789", 100);
        assert_eq!(b.logical_len(), 10);
        assert_eq!(String::from_utf8_lossy(&b.slice_from(4)), "456789");
        // Exceed cap → front drops, base advances, logical offsets stay valid.
        b.append(&vec![b'x'; 100], 50);
        assert!(b.truncated);
        assert_eq!(b.logical_len(), 110);
        assert_eq!(b.data.len(), 50);
        assert_eq!(b.base, 60);
        // A read from a dropped offset clamps to what remains, never panics.
        assert_eq!(b.slice_from(0).len(), 50);
        assert_eq!(b.slice_from(200).len(), 0);
    }

    #[test]
    fn cap_bytes_flags_overflow() {
        let (b, t) = cap_bytes(vec![1, 2, 3], 10);
        assert_eq!(b.len(), 3);
        assert!(!t);
        let (b, t) = cap_bytes(vec![0u8; 20], 10);
        assert_eq!(b.len(), 10);
        assert!(t);
    }

    #[test]
    fn control_roundtrips_json() {
        let c = TermControl {
            host: "127.0.0.1".into(),
            port: 51234,
            pid: 42,
            token: "deadbeef".into(),
            updated_at: "now".into(),
        };
        let raw = serde_json::to_string(&c).unwrap();
        let back: TermControl = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.port, 51234);
        assert_eq!(back.token, "deadbeef");
    }

    #[test]
    fn dispatch_rejects_bad_token_and_ungranted() {
        let mgr = SessionManager::default();
        // Bad token is refused before any grant/op work.
        let bad = dispatch(&mgr, &json!({ "op": "list", "token": "wrong" }).to_string());
        assert_eq!(bad.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert!(bad.get("error").unwrap().as_str().unwrap().contains("token"));
        // Right token but (default) no grant → refused with the grant message.
        let ungranted = dispatch(&mgr, &json!({ "op": "list", "token": mgr.token() }).to_string());
        if ungranted.get("ok").and_then(|v| v.as_bool()) == Some(false) {
            assert!(ungranted.get("error").unwrap().as_str().unwrap().contains("not granted"));
        }
        // (If the dev machine has the grant on, list succeeds — either way, no panic.)
    }
}
