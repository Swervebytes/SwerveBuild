//! App-managed local inference — no Ollama required.
//!
//! Swerve Build downloads a pinned llama.cpp `llama-server` build on first use,
//! registers user GGUF files as models, and runs ONE server at a time on
//! 127.0.0.1 with a generated API key. Grok reaches it through the managed
//! `[model.swerve-local-<slug>]` blocks in `~/.grok/config.toml` (see
//! `grok_config::apply_local_models`) — so a local model is just another `-m`
//! choice in chats and automations, and code never leaves the machine.
//!
//! **GPU arbitration (Step 4):** only one GGUF is loaded at a time. Chats and
//! automations take named leases on that model. Same-model concurrent use is
//! allowed (`--parallel 2`); switching models while any lease is held is
//! refused with a clear error so a chat is never yanked out from under an
//! automation (or the reverse).
//!
//! Downloads, hashing, and unzip shell out to OS tools (curl.exe,
//! Get-FileHash, Expand-Archive) to keep the dependency footprint at zero.

use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Pinned engine release — recorded 2026-07-17 from the ggml-org/llama.cpp
/// GitHub release (asset digest included in the release API). Upgrade
/// deliberately: bump all four constants together.
pub const ENGINE_TAG: &str = "b10063";
const ENGINE_URL: &str =
    "https://github.com/ggml-org/llama.cpp/releases/download/b10063/llama-b10063-bin-win-vulkan-x64.zip";
const ENGINE_SHA256: &str = "5a6dd9b7eb31900f0b1d4d560151e35d233e75e634112eb6c4026727f613a0e9";
const ENGINE_SIZE: u64 = 33_271_430;

/// Model load can take minutes for big GGUFs on first read (cold disk cache).
const HEALTH_TIMEOUT_SECS: u64 = 240;
/// Total context tokens shared across parallel slots.
/// Grok assembles large agentic prompts — small contexts break tool calling.
const CTX_TOKENS: u32 = 32768;
/// Concurrent OpenAI-compatible slots so a chat + an automation can share one
/// loaded model without serializing into a single slot.
const PARALLEL_SLOTS: u32 = 2;

/// Lease holder for a chat session (`chat:<chat_id>`).
pub fn chat_holder(chat_id: &str) -> String {
    format!("chat:{chat_id}")
}

/// Lease holder for an automation run (`auto:<run_id>`).
pub fn auto_holder(run_id: &str) -> String {
    format!("auto:{run_id}")
}

/// Pure decision used by `acquire` / `ensure_for_model` (unit-tested).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnsureAction {
    /// Already serving `want` — no process change.
    Keep,
    /// Idle or stopped — start/swap to `want` is allowed.
    StartOrSwap,
}

/// Decide whether we may load/keep `want` given the currently loaded model and
/// active lease holders. Leases always refer to the loaded model.
pub(crate) fn decide_ensure(
    loaded: Option<&str>,
    ready: bool,
    lease_holders: &[String],
    want: &str,
) -> Result<EnsureAction, String> {
    if ready && loaded == Some(want) {
        return Ok(EnsureAction::Keep);
    }
    if !lease_holders.is_empty() {
        let who = lease_holders.join(", ");
        let have = loaded.unwrap_or("(unknown)");
        return Err(format!(
            "Local model \"{have}\" is in use ({who}). Finish or close that work before switching to \"{want}\"."
        ));
    }
    Ok(EnsureAction::StartOrSwap)
}

pub fn engine_dir() -> PathBuf {
    crate::paths::data_dir().join("engine").join(ENGINE_TAG)
}

pub fn engine_exe() -> PathBuf {
    engine_dir().join("llama-server.exe")
}

pub fn engine_installed() -> bool {
    engine_exe().is_file()
}

// ------------------------------------------------------------------ install

fn emit_progress(app: &AppHandle, phase: &str, received: u64, total: u64) {
    let _ = app.emit(
        "local-engine-progress",
        json!({ "phase": phase, "received": received, "total": total }),
    );
}

/// Download + verify + unpack the pinned engine. Blocking (call from a
/// blocking task); progress streams via `local-engine-progress` events.
pub fn install_engine(app: &AppHandle) -> Result<String, String> {
    if engine_installed() {
        return Ok(format!("Engine {ENGINE_TAG} is already installed."));
    }
    let dir = engine_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create engine dir: {e}"))?;
    let zip = dir.join("engine.zip");

    // Progress reporter: poll the growing file against the known asset size.
    let done = Arc::new(AtomicBool::new(false));
    let done_bg = Arc::clone(&done);
    let app_bg = app.clone();
    let zip_bg = zip.clone();
    let reporter = thread::spawn(move || {
        while !done_bg.load(Ordering::SeqCst) {
            let received = fs::metadata(&zip_bg).map(|m| m.len()).unwrap_or(0);
            emit_progress(&app_bg, "downloading", received, ENGINE_SIZE);
            thread::sleep(Duration::from_millis(400));
        }
    });

    // curl.exe ships with Windows 10+; -C - resumes a partial download.
    let status = crate::util::hidden_command("curl.exe")
        .args([
            "-L", "--fail", "--retry", "3", "-C", "-",
            "-o", &zip.display().to_string(),
            ENGINE_URL,
        ])
        .status();
    done.store(true, Ordering::SeqCst);
    let _ = reporter.join();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(format!("engine download failed (curl exit {:?})", s.code())),
        Err(e) => return Err(format!("could not run curl.exe: {e}")),
    }

    emit_progress(app, "verifying", ENGINE_SIZE, ENGINE_SIZE);
    let hash = file_sha256(&zip)?;
    if !hash.eq_ignore_ascii_case(ENGINE_SHA256) {
        let _ = fs::remove_file(&zip);
        return Err(format!(
            "engine checksum mismatch (got {hash}); download removed — try again"
        ));
    }

    emit_progress(app, "unpacking", ENGINE_SIZE, ENGINE_SIZE);
    let unzip = crate::util::hidden_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                ps_quote(&zip.display().to_string()),
                ps_quote(&dir.display().to_string())
            ),
        ])
        .status();
    match unzip {
        Ok(s) if s.success() => {}
        _ => return Err("engine unzip failed".to_string()),
    }
    let _ = fs::remove_file(&zip);

    // Release zips normally unpack flat; tolerate a nested layout by finding
    // the exe and flattening its directory into ours.
    if !engine_installed() {
        if let Some(found) = find_file(&dir, "llama-server.exe") {
            if let Some(parent) = found.parent() {
                if let Ok(entries) = fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        let dest = dir.join(entry.file_name());
                        let _ = fs::rename(entry.path(), dest);
                    }
                }
            }
        }
    }
    if !engine_installed() {
        return Err("engine unpacked but llama-server.exe was not found".to_string());
    }

    emit_progress(app, "done", ENGINE_SIZE, ENGINE_SIZE);
    Ok(format!("Engine {ENGINE_TAG} installed."))
}

fn file_sha256(path: &PathBuf) -> Result<String, String> {
    let output = crate::util::hidden_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash",
                ps_quote(&path.display().to_string())
            ),
        ])
        .output()
        .map_err(|e| format!("hash: {e}"))?;
    if !output.status.success() {
        return Err("hashing the download failed".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Escape a string for embedding inside a PowerShell single-quoted literal: a
/// literal `'` becomes `''`. Without this, a data dir under a home path that
/// contains an apostrophe (e.g. `C:\Path\O'Connor`) would break — or let content
/// break out of — the `Expand-Archive` / `Get-FileHash` command strings below.
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn find_file(root: &PathBuf, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(p);
        }
        if p.is_dir() {
            if let Some(found) = find_file(&p, name) {
                return Some(found);
            }
        }
    }
    None
}

// ------------------------------------------------------------------ manager

#[derive(Debug, Clone, PartialEq)]
enum State {
    Stopped,
    Starting,
    Ready,
    Failed(String),
}

struct Inner {
    state: State,
    model_id: Option<String>,
    port: Option<u16>,
    child: Option<Child>,
    /// Active users of the loaded model (`chat:…`, `auto:…`). Non-empty blocks
    /// model swaps and idle stops so concurrent paths can't yank VRAM.
    leases: HashMap<String, ()>,
    /// Ring buffer of recent stderr lines for error reporting.
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

pub struct LlmManager {
    inner: Mutex<Inner>,
}

#[derive(Serialize, Clone)]
pub struct ServerStatus {
    pub state: String,
    pub model_id: Option<String>,
    pub port: Option<u16>,
    pub message: Option<String>,
    /// Holders currently leasing the loaded model (empty when idle).
    pub leases: Vec<String>,
}

static MANAGER: OnceLock<LlmManager> = OnceLock::new();

/// Process-wide singleton — the local server is inherently app-global state,
/// and both the chat path and the automation runner need to reach it.
pub fn manager() -> &'static LlmManager {
    MANAGER.get_or_init(|| LlmManager {
        inner: Mutex::new(Inner {
            state: State::Stopped,
            model_id: None,
            port: None,
            child: None,
            leases: HashMap::new(),
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
        }),
    })
}

fn emit_status(app: &AppHandle, status: &ServerStatus) {
    let _ = app.emit("local-llm-status", status);
}

impl LlmManager {
    pub fn status(&self) -> ServerStatus {
        let inner = self.inner.lock().expect("llm lock");
        let (state, message) = match &inner.state {
            State::Stopped => ("stopped", None),
            State::Starting => ("starting", None),
            State::Ready => ("ready", None),
            State::Failed(e) => ("failed", Some(e.clone())),
        };
        let mut leases: Vec<String> = inner.leases.keys().cloned().collect();
        leases.sort();
        ServerStatus {
            state: state.to_string(),
            model_id: inner.model_id.clone(),
            port: inner.port,
            message,
            leases,
        }
    }

    fn lease_holders_locked(inner: &Inner) -> Vec<String> {
        let mut h: Vec<String> = inner.leases.keys().cloned().collect();
        h.sort();
        h
    }

    /// Make sure the server is Ready and serving `model_id`. Swaps only when
    /// no leases are held. Blocking — call from a blocking task/thread.
    pub fn ensure_for_model(&self, app: &AppHandle, model_id: &str) -> Result<(), String> {
        let action = {
            let inner = self.inner.lock().expect("llm lock");
            decide_ensure(
                inner.model_id.as_deref(),
                inner.state == State::Ready,
                &Self::lease_holders_locked(&inner),
                model_id,
            )?
        };
        match action {
            EnsureAction::Keep => Ok(()),
            EnsureAction::StartOrSwap => {
                self.stop(app);
                self.start(app, model_id)
            }
        }
    }

    /// Ensure `model_id` is loaded and record `holder` as using it until
    /// [`release`]. Same-model concurrent holders share the server; different
    /// models are blocked while any holder remains.
    pub fn acquire(
        &self,
        app: &AppHandle,
        holder: &str,
        model_id: &str,
    ) -> Result<(), String> {
        self.ensure_for_model(app, model_id)?;
        let mut inner = self.inner.lock().expect("llm lock");
        // Re-check after start: another path may have raced (shouldn't with the
        // process-wide mutex held across ensure's critical sections, but keep
        // the lease book consistent with the loaded model).
        if inner.state != State::Ready || inner.model_id.as_deref() != Some(model_id) {
            return Err(format!(
                "Local model \"{model_id}\" failed to stay ready for {holder}"
            ));
        }
        inner.leases.insert(holder.to_string(), ());
        drop(inner);
        emit_status(app, &self.status());
        Ok(())
    }

    /// Drop a holder lease. Does not stop the server (leave it warm).
    pub fn release(&self, holder: &str) {
        let mut inner = self.inner.lock().expect("llm lock");
        inner.leases.remove(holder);
    }

    /// Release every lease whose holder starts with `prefix` (e.g. `chat:`).
    pub fn release_prefix(&self, prefix: &str) {
        let mut inner = self.inner.lock().expect("llm lock");
        inner.leases.retain(|h, _| !h.starts_with(prefix));
    }

    /// Stop only when idle (no leases). Used by Settings stop / remove.
    pub fn stop_if_idle(&self, app: &AppHandle) -> Result<(), String> {
        {
            let inner = self.inner.lock().expect("llm lock");
            if !inner.leases.is_empty() {
                let who = Self::lease_holders_locked(&inner).join(", ");
                return Err(format!(
                    "Local model is in use ({who}). Close those chats/runs before stopping the server."
                ));
            }
        }
        self.stop(app);
        Ok(())
    }

    fn start(&self, app: &AppHandle, model_id: &str) -> Result<(), String> {
        if !engine_installed() {
            return Err(
                "Local engine isn't installed — open Settings → Local models and install it."
                    .to_string(),
            );
        }
        let store = crate::providers::ProviderStore::load();
        let model = store
            .local
            .models
            .iter()
            .find(|m| m.id == model_id)
            .cloned()
            .ok_or_else(|| format!("Unknown local model: {model_id}"))?;
        if !PathBuf::from(&model.path).is_file() {
            return Err(format!(
                "Model file is missing: {} — was it moved or deleted?",
                model.path
            ));
        }
        let port = crate::providers::ensure_local_port()?;
        let token = crate::providers::ensure_local_token()?;

        let tail_handle = {
            let mut inner = self.inner.lock().expect("llm lock");
            inner.state = State::Starting;
            inner.model_id = Some(model_id.to_string());
            inner.port = Some(port);
            Arc::clone(&inner.stderr_tail)
        };
        if let Ok(mut tail) = tail_handle.lock() {
            tail.clear();
        }
        emit_status(app, &self.status());

        let mut command = crate::util::hidden_command(engine_exe());
        command
            .args([
                "-m", &model.path,
                "--host", "127.0.0.1",
                "--port", &port.to_string(),
                "--api-key", &token,
                // b10063 spells it `-a/--alias` (verified against the binary's
                // --help; `--model-alias` is rejected with exit 1).
                "-a", model_id,
                "-ngl", "999",
                "-c", &CTX_TOKENS.to_string(),
                // Chat + automation may share one loaded GGUF (lease arbitration
                // still forbids a different model while either holds a lease).
                "--parallel", &PARALLEL_SLOTS.to_string(),
                "--jinja",
                "--no-webui",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| self.fail(app, format!("failed to start llama-server: {e}")))?;

        // Drain output; keep a stderr tail for error messages. llama.cpp logs
        // to stderr, so this also prevents pipe-buffer stalls.
        let tail = tail_handle;
        if let Some(stderr) = child.stderr.take() {
            let tail = Arc::clone(&tail);
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if let Ok(mut tail) = tail.lock() {
                        if tail.len() >= 40 {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                }
            });
        }
        if let Some(stdout) = child.stdout.take() {
            thread::spawn(move || {
                for _ in BufReader::new(stdout).lines().map_while(Result::ok) {}
            });
        }

        {
            let mut inner = self.inner.lock().expect("llm lock");
            inner.child = Some(child);
        }

        // Poll /health until the model is loaded. curl exit 0 + "200" = ready.
        let deadline = Instant::now() + Duration::from_secs(HEALTH_TIMEOUT_SECS);
        loop {
            {
                let mut inner = self.inner.lock().expect("llm lock");
                if let Some(child) = inner.child.as_mut() {
                    if let Ok(Some(code)) = child.try_wait() {
                        inner.child = None;
                        drop(inner);
                        let tail_text = tail
                            .lock()
                            .map(|t| t.iter().rev().take(6).cloned().collect::<Vec<_>>().join(" | "))
                            .unwrap_or_default();
                        return Err(self.fail(
                            app,
                            format!("llama-server exited early ({code}). {tail_text}"),
                        ));
                    }
                }
            }
            let ok = crate::util::hidden_command("curl.exe")
                .args([
                    "-s", "-o", "NUL", "-w", "%{http_code}",
                    "--max-time", "3",
                    &format!("http://127.0.0.1:{port}/health"),
                ])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "200")
                .unwrap_or(false);
            if ok {
                break;
            }
            if Instant::now() > deadline {
                self.stop(app);
                return Err(self.fail(
                    app,
                    format!("local model didn't become ready within {HEALTH_TIMEOUT_SECS}s"),
                ));
            }
            thread::sleep(Duration::from_millis(700));
        }

        {
            let mut inner = self.inner.lock().expect("llm lock");
            inner.state = State::Ready;
        }
        emit_status(app, &self.status());
        Ok(())
    }

    fn fail(&self, app: &AppHandle, message: String) -> String {
        {
            let mut inner = self.inner.lock().expect("llm lock");
            inner.state = State::Failed(message.clone());
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        emit_status(app, &self.status());
        message
    }

    pub fn stop(&self, app: &AppHandle) {
        {
            let mut inner = self.inner.lock().expect("llm lock");
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            inner.state = State::Stopped;
            inner.model_id = None;
            // Force-stop drops leases — process is gone; holders must re-acquire.
            inner.leases.clear();
        }
        emit_status(app, &self.status());
    }

    /// App-exit cleanup — no events, just make sure the server dies with us.
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().expect("llm lock");
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        inner.state = State::Stopped;
        inner.model_id = None;
        inner.leases.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_model_keeps_server_even_with_leases() {
        let action = decide_ensure(
            Some("swerve-local-a"),
            true,
            &["chat:1".into(), "auto:run".into()],
            "swerve-local-a",
        );
        assert_eq!(action, Ok(EnsureAction::Keep));
    }

    #[test]
    fn idle_swap_allowed() {
        let action = decide_ensure(Some("swerve-local-a"), true, &[], "swerve-local-b");
        assert_eq!(action, Ok(EnsureAction::StartOrSwap));
    }

    #[test]
    fn busy_swap_blocked() {
        let err = decide_ensure(
            Some("swerve-local-a"),
            true,
            &["chat:1".into()],
            "swerve-local-b",
        )
        .expect_err("must block");
        assert!(err.contains("in use"), "{err}");
        assert!(err.contains("swerve-local-a"), "{err}");
        assert!(err.contains("swerve-local-b"), "{err}");
        assert!(err.contains("chat:1"), "{err}");
    }

    #[test]
    fn stopped_starts_even_if_stale_holders_absent() {
        // No leases → start from stopped.
        let action = decide_ensure(None, false, &[], "swerve-local-a");
        assert_eq!(action, Ok(EnsureAction::StartOrSwap));
    }

    #[test]
    fn automation_and_chat_holders_format() {
        assert_eq!(chat_holder("abc"), "chat:abc");
        assert_eq!(auto_holder("run-1"), "auto:run-1");
    }
}

/// Find a free localhost port by binding to :0 and reading the assignment.
pub fn find_free_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("no free port: {e}"))
}
