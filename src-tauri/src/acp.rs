use crate::providers::AcpLaunch;
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const CONNECT_TIMEOUT_SECS: u64 = 45;
const PROMPT_TIMEOUT_SECS: u64 = 300;
const MAX_CONCURRENT_SESSIONS: usize = 3;

pub struct AcpManager {
    sessions: Mutex<HashMap<String, ActiveSession>>,
}

/// Shared ACP transport — must be usable without holding `AcpManager::sessions` lock
/// so permission responses can be sent while `session/prompt` is in flight.
struct SessionTransport {
    stdin: Arc<Mutex<ChildStdin>>,
    request_id: AtomicU64,
    responses: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>>,
    /// While true, `session/update` notifications are dropped instead of emitted.
    /// `session/load` replays the whole history as updates before it responds;
    /// the app already persists messages itself, so forwarding the replay would
    /// duplicate every saved message in the UI.
    suppress_updates: AtomicBool,
}

struct ActiveSession {
    chat_id: String,
    session_id: String,
    child: Child,
    transport: Arc<SessionTransport>,
    last_accessed: AtomicU64,
}

impl Default for AcpManager {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl AcpManager {
    pub fn list_active(&self) -> Vec<String> {
        self.sessions
            .lock()
            .map(|guard| guard.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn close_chat(&self, chat_id: &str) {
        if let Ok(mut guard) = self.sessions.lock() {
            if let Some(mut active) = guard.remove(chat_id) {
                let _ = active.child.kill();
                let _ = active.child.wait();
            }
        }
    }

    pub fn close_all(&self) {
        if let Ok(mut guard) = self.sessions.lock() {
            for (_, mut active) in guard.drain() {
                let _ = active.child.kill();
                let _ = active.child.wait();
            }
        }
    }

    pub fn ensure_session(
        &self,
        app: AppHandle,
        launch: &AcpLaunch,
        cwd: &str,
        chat_id: &str,
        stored_session_id: Option<&str>,
    ) -> Result<String, String> {
        if let Ok(guard) = self.sessions.lock() {
            if guard.contains_key(chat_id) {
                let session_id = guard.get(chat_id).map(|s| s.session_id.clone());
                drop(guard);
                self.touch(chat_id);
                if let Some(session_id) = session_id {
                    let _ = app.emit(
                        "chat-session-ready",
                        json!({ "chatId": chat_id, "sessionId": session_id }),
                    );
                    return Ok(session_id);
                }
            }
        }

        self.evict_if_needed();
        self.spawn_session(app, launch, cwd, chat_id, stored_session_id)
    }

    fn touch(&self, chat_id: &str) {
        if let Ok(guard) = self.sessions.lock() {
            if let Some(session) = guard.get(chat_id) {
                session.bump_access();
            }
        }
    }

    fn session_id_for(&self, chat_id: &str) -> Result<String, String> {
        let guard = self
            .sessions
            .lock()
            .map_err(|_| "ACP session lock poisoned".to_string())?;
        guard
            .get(chat_id)
            .map(|s| s.session_id.clone())
            .ok_or_else(|| "Chat session not found".to_string())
    }

    fn evict_if_needed(&self) {
        let mut guard = match self.sessions.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        while guard.len() >= MAX_CONCURRENT_SESSIONS {
            let lru = guard
                .iter()
                .min_by_key(|(_, session)| session.last_accessed.load(Ordering::SeqCst))
                .map(|(id, _)| id.clone());
            if let Some(id) = lru {
                if let Some(mut active) = guard.remove(&id) {
                    let _ = active.child.kill();
                    let _ = active.child.wait();
                }
            } else {
                break;
            }
        }
    }

    fn spawn_session(
        &self,
        app: AppHandle,
        launch: &AcpLaunch,
        cwd: &str,
        chat_id: &str,
        stored_session_id: Option<&str>,
    ) -> Result<String, String> {
        let mut command = crate::util::hidden_command(&launch.command);
        command
            .args(&launch.args)
            .envs(launch.env.iter().cloned())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| format!("Failed to start {} agent: {e}", launch.label))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open grok stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open grok stdout".to_string())?;
        let stderr = child.stderr.take();

        if let Some(stderr) = stderr {
            let chat_for_stderr = chat_id.to_string();
            let app_for_stderr = app.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = app_for_stderr.emit(
                        "chat-log",
                        json!({ "chatId": chat_for_stderr, "line": line, "stream": "stderr" }),
                    );
                }
            });
        }

        let transport = Arc::new(SessionTransport {
            stdin: Arc::new(Mutex::new(stdin)),
            request_id: AtomicU64::new(1),
            responses: Arc::new(Mutex::new(HashMap::new())),
            suppress_updates: AtomicBool::new(false),
        });
        let transport_for_reader = Arc::clone(&transport);
        let app_for_reader = app.clone();
        let chat_for_reader = chat_id.to_string();
        // The agent's fs/read|write requests are confined to this project dir.
        let cwd_for_reader = cwd.to_string();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }

                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };

                if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                    if method == "session/update" {
                        if !transport_for_reader.suppress_updates.load(Ordering::SeqCst) {
                            let _ = app_for_reader.emit(
                                "chat-update",
                                json!({
                                    "chatId": chat_for_reader,
                                    "params": value.get("params").cloned().unwrap_or(Value::Null),
                                }),
                            );
                        }
                        continue;
                    }

                    if method == "session/request_permission" {
                        if let Some(id) = jsonrpc_id(&value) {
                            let _ = app_for_reader.emit(
                                "chat-permission-request",
                                json!({
                                    "chatId": chat_for_reader,
                                    "requestId": id,
                                    "params": value.get("params").cloned().unwrap_or(Value::Null),
                                }),
                            );
                        }
                        continue;
                    }

                    if let Some(id) = value.get("id").cloned().filter(|v| !v.is_null()) {
                        // Every agent->client request gets a reply. Unhandled
                        // methods and out-of-scope fs paths get a JSON-RPC error
                        // instead of silence (silence hangs a strict peer) or an
                        // unrestricted filesystem read/write (the old behavior).
                        let reply = match handle_client_request(
                            method,
                            value.get("params"),
                            &cwd_for_reader,
                        ) {
                            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                            Err((code, message)) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": code, "message": message },
                            }),
                        };
                        if let Ok(mut writer) = transport_for_reader.stdin.lock() {
                            let _ = writeln!(writer, "{}", reply);
                            let _ = writer.flush();
                        }
                        continue;
                    }
                }

                if let Some(id) = jsonrpc_id(&value) {
                    if let Ok(mut pending) = transport_for_reader.responses.lock() {
                        if let Some(sender) = pending.remove(&id) {
                            let _ = sender.send(value);
                        }
                    }
                }
            }
        });

        let mut active = ActiveSession {
            chat_id: chat_id.to_string(),
            session_id: String::new(),
            child,
            transport: Arc::clone(&transport),
            last_accessed: AtomicU64::new(now_secs()),
        };

        let agent_caps = active.transport.rpc(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true }
                },
                "clientInfo": {
                    "name": "swerve-build",
                    "title": "Swerve Build",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            CONNECT_TIMEOUT_SECS,
        )?;

        let mcp_servers = mcp_servers_config()?;
        // Grok (v0.2.x) advertises `agentCapabilities.loadSession: true`;
        // `sessionCapabilities.resume` is kept as a forward-compat check for
        // agents on the newer capability shape.
        let caps = agent_caps.get("agentCapabilities");
        let can_load = caps
            .and_then(|c| c.get("loadSession"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || caps
                .and_then(|c| c.get("sessionCapabilities"))
                .and_then(|c| c.get("resume"))
                .is_some();

        let session_id = if let Some(stored) = stored_session_id.filter(|_| can_load) {
            active
                .transport
                .suppress_updates
                .store(true, Ordering::SeqCst);
            let loaded = active.transport.rpc(
                "session/load",
                json!({
                    "sessionId": stored,
                    "cwd": cwd,
                    "mcpServers": mcp_servers,
                }),
                CONNECT_TIMEOUT_SECS,
            );
            active
                .transport
                .suppress_updates
                .store(false, Ordering::SeqCst);
            match loaded {
                Ok(_) => stored.to_string(),
                Err(_) => Self::create_new_session(&mut active, cwd, &mcp_servers)?,
            }
        } else {
            Self::create_new_session(&mut active, cwd, &mcp_servers)?
        };

        active.session_id = session_id.clone();
        active.bump_access();

        let _ = app.emit(
            "chat-session-ready",
            json!({ "chatId": chat_id, "sessionId": session_id }),
        );

        if let Ok(mut guard) = self.sessions.lock() {
            guard.insert(chat_id.to_string(), active);
        }

        Ok(session_id)
    }

    fn create_new_session(
        active: &mut ActiveSession,
        cwd: &str,
        mcp_servers: &Value,
    ) -> Result<String, String> {
        let result = active.transport.rpc(
            "session/new",
            json!({
                "cwd": cwd,
                "mcpServers": mcp_servers,
            }),
            CONNECT_TIMEOUT_SECS,
        )?;

        result
            .get("sessionId")
            .and_then(|id| id.as_str())
            .map(|id| id.to_string())
            .ok_or_else(|| "ACP session/new did not return sessionId".to_string())
    }

    pub fn respond_permission(
        &self,
        chat_id: &str,
        request_id: u64,
        option_id: &str,
    ) -> Result<(), String> {
        let transport = {
            let guard = self
                .sessions
                .lock()
                .map_err(|_| "ACP session lock poisoned".to_string())?;
            let active = guard
                .get(chat_id)
                .ok_or_else(|| "Chat session not active".to_string())?;
            Arc::clone(&active.transport)
        };

        let reply = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "outcome": {
                    "outcome": "selected",
                    "optionId": option_id
                }
            }
        });

        transport.write_json_line(&reply)
    }

    pub fn send_prompt(&self, chat_id: &str, text: &str, images: &[String]) -> Result<(), String> {
        let (transport, session_id) = {
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| "ACP session lock poisoned".to_string())?;
            let active = guard
                .get_mut(chat_id)
                .ok_or_else(|| "No active session for this chat. Open the chat to connect.".to_string())?;
            active.bump_access();
            (
                Arc::clone(&active.transport),
                active.session_id.clone(),
            )
        };

        let mut prompt_text = text.trim().to_string();
        for (index, image) in images.iter().enumerate() {
            if !prompt_text.is_empty() {
                prompt_text.push('\n');
            }
            prompt_text.push_str(&format!("[Image #{}: {}]", index + 1, image));
        }

        if prompt_text.is_empty() {
            return Err("Message cannot be empty".to_string());
        }

        transport.rpc(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": prompt_text }]
            }),
            PROMPT_TIMEOUT_SECS,
        )?;

        Ok(())
    }
}

impl ActiveSession {
    fn bump_access(&self) {
        self.last_accessed
            .store(now_secs(), Ordering::SeqCst);
    }
}

impl SessionTransport {
    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    fn write_json_line(&self, value: &Value) -> Result<(), String> {
        let mut writer = self
            .stdin
            .lock()
            .map_err(|_| "ACP stdin lock poisoned".to_string())?;
        let line = serde_json::to_string(value).map_err(|e| e.to_string())?;
        writeln!(writer, "{line}")
            .map_err(|e| format!("Failed to write ACP message: {e}"))?;
        writer
            .flush()
            .map_err(|e| format!("Failed to flush ACP stdin: {e}"))?;
        Ok(())
    }

    fn rpc(&self, method: &str, params: Value, timeout_secs: u64) -> Result<Value, String> {
        let id = self.next_id();
        let (tx, rx) = mpsc::channel();

        {
            let mut pending = self
                .responses
                .lock()
                .map_err(|_| "ACP response lock poisoned".to_string())?;
            pending.insert(id, tx);
        }

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.write_json_line(&request)?;

        let response = rx
            .recv_timeout(std::time::Duration::from_secs(timeout_secs))
            .map_err(|_| {
                format!(
                    "Timed out after {timeout_secs}s waiting for Grok ({method}). Approve any pending tool prompts, check sign-in, and try again."
                )
            })?;

        if let Some(error) = response.get("error") {
            return Err(error.to_string());
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn jsonrpc_id(value: &Value) -> Option<u64> {
    let id = value.get("id")?;
    id.as_u64()
        .or_else(|| id.as_str().and_then(|s| s.parse().ok()))
        .or_else(|| id.as_i64().filter(|&n| n >= 0).map(|n| n as u64))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mcp_servers_config() -> Result<Value, String> {
    let mcp_path = resolve_mcp_binary()?;
    let mut servers = vec![json!({
        "name": "swervebuild",
        "command": mcp_path,
        "args": [],
        "env": []
    })];

    // SwerveBytes engine (swervebytes-core): the verified-Byte runtime.
    // Optional — auto-included whenever its MCP server is installed on PATH
    // (`pip install swervebytes` puts `swervebytes-mcp` there). Chat agents
    // then get run/verify/retire/audit tools over the Byte registry.
    if let Some(sb) = crate::which_on_path("swervebytes-mcp") {
        servers.push(json!({
            "name": "swervebytes",
            "command": sb.display().to_string(),
            "args": [],
            "env": []
        }));
    }

    Ok(Value::Array(servers))
}

fn resolve_mcp_binary() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or_else(|| "No exe parent dir".to_string())?;

    let candidates = [
        dir.join("swervebuild-mcp.exe"),
        dir.join("swervebuild-mcp"),
        dir.join("swervegrok-mcp.exe"),
        dir.join("swervegrok-mcp"),
        PathBuf::from("target/debug/swervebuild-mcp.exe"),
        PathBuf::from("target/debug/swervebuild-mcp"),
        PathBuf::from("target/debug/swervegrok-mcp.exe"),
        PathBuf::from("target/debug/swervegrok-mcp"),
    ];

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate.display().to_string());
        }
    }

    Err("swervebuild-mcp binary not found. Rebuild the project.".to_string())
}

/// Handle an agent->client JSON-RPC request. Returns `Ok(result)` or
/// `Err((code, message))` (a JSON-RPC error object). The two `fs/*` methods are
/// confined to `cwd` (the chat's project directory) — an agent can no longer
/// read or overwrite arbitrary files on disk. Everything else is method-not-found.
fn handle_client_request(
    method: &str,
    params: Option<&Value>,
    cwd: &str,
) -> Result<Value, (i64, String)> {
    match method {
        "fs/read_text_file" => {
            let path = params
                .and_then(|p| p.get("path"))
                .and_then(|p| p.as_str())
                .ok_or((-32602, "missing path".to_string()))?;
            let target = confine_to_cwd(path, cwd)
                .ok_or((-32001, format!("path outside project directory: {path}")))?;
            let content = fs::read_to_string(&target)
                .map_err(|e| (-32000, format!("read failed: {e}")))?;
            Ok(json!({ "content": content }))
        }
        "fs/write_text_file" => {
            let path = params
                .and_then(|p| p.get("path"))
                .and_then(|p| p.as_str())
                .ok_or((-32602, "missing path".to_string()))?;
            let content = params
                .and_then(|p| p.get("content"))
                .and_then(|p| p.as_str())
                .ok_or((-32602, "missing content".to_string()))?;
            let target = confine_to_cwd(path, cwd)
                .ok_or((-32001, format!("path outside project directory: {path}")))?;
            if let Some(parent) = target.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&target, content).map_err(|e| (-32000, format!("write failed: {e}")))?;
            Ok(Value::Null)
        }
        other => Err((-32601, format!("method not supported: {other}"))),
    }
}

/// Resolve `requested` (absolute or relative to `cwd`) and return it only if it
/// stays inside the canonicalized `cwd`. `..` is collapsed lexically and the
/// prefix check is case-insensitive (Windows). Returns None on any escape.
fn confine_to_cwd(requested: &str, cwd: &str) -> Option<PathBuf> {
    let root = strip_verbatim(&fs::canonicalize(cwd).ok()?);
    let req = Path::new(requested);
    let abs = if req.is_absolute() {
        req.to_path_buf()
    } else {
        root.join(req)
    };
    let normalized = normalize_lexical(&abs);

    let n = normalized.to_string_lossy().to_lowercase();
    let r = root.to_string_lossy().to_lowercase();
    let inside = n == r
        || n.starts_with(&format!("{r}\\"))
        || n.starts_with(&format!("{r}/"));
    inside.then_some(normalized)
}

/// Collapse `.` and `..` components without touching the filesystem.
fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Strip Windows' `\\?\` verbatim prefix that `canonicalize` adds, so paths
/// compare and print naturally.
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

pub fn save_grok_session_id(chat_id: &str, session_id: &str) -> Result<(), String> {
    let _guard = Store::lock();
    let mut store = Store::load();
    if let Some(chat) = store.chats.iter_mut().find(|c| c.id == chat_id) {
        chat.grok_session_id = Some(session_id.to_string());
        Store::save(&store)?;
    }
    Ok(())
}

pub fn attachments_dir() -> PathBuf {
    crate::paths::attachments_dir()
}

pub fn save_image_base64(data: &str) -> Result<String, String> {
    use base64::Engine;

    let payload = data
        .split_once(',')
        .map(|(_, value)| value)
        .unwrap_or(data);

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("Invalid image data: {e}"))?;

    let dir = attachments_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.join(format!("{}.png", Store::new_id()));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}