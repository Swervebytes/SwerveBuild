use crate::providers::AcpLaunch;
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const CONNECT_TIMEOUT_SECS: u64 = 45;
const PROMPT_TIMEOUT_SECS: u64 = 300;
const MAX_CONCURRENT_SESSIONS: usize = 3;

pub struct AcpManager {
    sessions: Mutex<HashMap<String, ActiveSession>>,
}

struct ActiveSession {
    chat_id: String,
    session_id: String,
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    request_id: AtomicU64,
    responses: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>>,
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
        let mut command = Command::new(&launch.command);
        command
            .args(&launch.args)
            .envs(launch.env.iter().cloned())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

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

        let stdin = Arc::new(Mutex::new(stdin));
        let responses: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let responses_for_reader = Arc::clone(&responses);
        let stdin_for_reader = Arc::clone(&stdin);
        let app_for_reader = app.clone();
        let chat_for_reader = chat_id.to_string();

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
                        let _ = app_for_reader.emit(
                            "chat-update",
                            json!({
                                "chatId": chat_for_reader,
                                "params": value.get("params").cloned().unwrap_or(Value::Null),
                            }),
                        );
                        continue;
                    }

                    if method == "session/request_permission" {
                        if let Some(id) = value.get("id").and_then(|id| id.as_u64()) {
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

                    if let Some(id) = value.get("id").and_then(|id| id.as_u64()) {
                        if let Some(response) =
                            handle_client_request(method, value.get("params"))
                        {
                            let reply = json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": response,
                            });
                            if let Ok(mut writer) = stdin_for_reader.lock() {
                                let _ = writeln!(writer, "{}", reply.to_string());
                                let _ = writer.flush();
                            }
                        }
                        continue;
                    }
                }

                if let Some(id) = value.get("id").and_then(|id| id.as_u64()) {
                    if let Ok(mut pending) = responses_for_reader.lock() {
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
            stdin,
            request_id: AtomicU64::new(1),
            responses,
            last_accessed: AtomicU64::new(now_secs()),
        };

        let agent_caps = active.rpc(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true }
                },
                "clientInfo": {
                    "name": "swerve-build",
                    "title": "Swerve Build",
                    "version": "0.1.0"
                }
            }),
            CONNECT_TIMEOUT_SECS,
        )?;

        let mcp_servers = mcp_servers_config()?;
        let can_resume = agent_caps
            .get("agentCapabilities")
            .and_then(|c| c.get("sessionCapabilities"))
            .and_then(|c| c.get("resume"))
            .is_some();

        let session_id = if let Some(stored) = stored_session_id.filter(|_| can_resume) {
            match active.rpc(
                "session/resume",
                json!({
                    "sessionId": stored,
                    "cwd": cwd,
                    "mcpServers": mcp_servers,
                }),
                CONNECT_TIMEOUT_SECS,
            ) {
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
        let result = active.rpc(
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
        let guard = self
            .sessions
            .lock()
            .map_err(|_| "ACP session lock poisoned".to_string())?;
        let active = guard
            .get(chat_id)
            .ok_or_else(|| "Chat session not active".to_string())?;

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

        let mut writer = active
            .stdin
            .lock()
            .map_err(|_| "ACP stdin lock poisoned".to_string())?;
        writeln!(writer, "{}", reply.to_string())
            .map_err(|e| format!("Failed to send permission response: {e}"))?;
        writer
            .flush()
            .map_err(|e| format!("Failed to flush permission response: {e}"))?;
        Ok(())
    }

    pub fn send_prompt(&self, chat_id: &str, text: &str, images: &[String]) -> Result<(), String> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| "ACP session lock poisoned".to_string())?;

        let active = guard
            .get_mut(chat_id)
            .ok_or_else(|| "No active session for this chat. Open the chat to connect.".to_string())?;

        active.bump_access();

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

        let session_id = active.session_id.clone();
        active.rpc(
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

    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    fn rpc(&mut self, method: &str, params: Value, timeout_secs: u64) -> Result<Value, String> {
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

        {
            let mut writer = self
                .stdin
                .lock()
                .map_err(|_| "ACP stdin lock poisoned".to_string())?;
            let line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
            writeln!(writer, "{line}")
                .map_err(|e| format!("Failed to write ACP request: {e}"))?;
            writer
                .flush()
                .map_err(|e| format!("Failed to flush ACP stdin: {e}"))?;
        }

        let response = rx
            .recv_timeout(std::time::Duration::from_secs(timeout_secs))
            .map_err(|_| {
                format!(
                    "Timed out after {timeout_secs}s waiting for Grok ({method}). Check sign-in and try again."
                )
            })?;

        if let Some(error) = response.get("error") {
            return Err(error.to_string());
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mcp_servers_config() -> Result<Value, String> {
    let mcp_path = resolve_mcp_binary()?;
    Ok(json!([{
        "name": "swervebuild",
        "command": mcp_path,
        "args": [],
        "env": []
    }]))
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

fn handle_client_request(method: &str, params: Option<&Value>) -> Option<Value> {
    match method {
        "fs/read_text_file" => {
            let path = params?.get("path")?.as_str()?;
            let content = fs::read_to_string(path).ok()?;
            Some(json!({ "content": content }))
        }
        "fs/write_text_file" => {
            let path = params?.get("path")?.as_str()?;
            let content = params?.get("content")?.as_str()?;
            if let Some(parent) = Path::new(path).parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(path, content).ok()?;
            Some(Value::Null)
        }
        _ => None,
    }
}

pub fn save_grok_session_id(chat_id: &str, session_id: &str) -> Result<(), String> {
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