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
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const CONNECT_TIMEOUT_SECS: u64 = 45;
/// ACP `initialize.protocolVersion` we advertise. Snapshot of supported
/// providers lives in docs-internal/DEPENDENCIES.md (Step 2). Grok Build CLI
/// 0.2.x negotiates protocol 1 successfully (verified through daily use +
/// session/load capability probes below).
pub const ACP_PROTOCOL_VERSION: u64 = 1;
// Agentic coding turns can run many minutes; a short ceiling aborts a
// still-working agent and truncates its reply (the old 300s bug). Wait long, and
// rely on two faster signals instead: the reader thread unblocks this wait the
// moment grok's stdout closes (process died), and the user can Stop a turn
// (session/cancel).
const PROMPT_TIMEOUT_SECS: u64 = 1800;
/// How long the reader waits for the user to approve/deny an agent file write.
const WRITE_APPROVAL_TIMEOUT_SECS: u64 = 600;
const MAX_CONCURRENT_SESSIONS: usize = 3;

type SessionMap = HashMap<String, ActiveSession>;
type WriteApprovalMap = HashMap<(String, u64), mpsc::Sender<bool>>;

pub struct AcpManager {
    /// Arc so the stdout reader can remove a dead session without holding
    /// `AcpManager` itself (the reader outlives the spawn call).
    sessions: Arc<Mutex<SessionMap>>,
    /// Pending `fs/write_text_file` approvals keyed by (chat_id, JSON-RPC id).
    /// The reader thread blocks on the receiver until the UI calls
    /// `respond_permission` (or the session dies / times out).
    write_approvals: Arc<Mutex<WriteApprovalMap>>,
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
    /// Id of the in-flight `session/prompt`, so the reader can recognise its
    /// reply and announce end-of-turn in event order (see `chat-turn-end`).
    prompt_id: Mutex<Option<u64>>,
}

struct ActiveSession {
    session_id: String,
    child: Child,
    transport: Arc<SessionTransport>,
    last_accessed: Arc<AtomicU64>,
    /// Last injected env fingerprint (S21). When live env changes (model,
    /// media provider, grants, …), the next user turn re-prepends a fresh pack.
    /// Not stored in chat history — wire-only so the UI still shows the bare message.
    env_fingerprint: Mutex<Option<String>>,
}

impl Default for AcpManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            write_approvals: Arc::new(Mutex::new(HashMap::new())),
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
        reject_write_approvals_for_chat(&self.write_approvals, chat_id);
        if let Ok(mut guard) = self.sessions.lock() {
            if let Some(mut active) = guard.remove(chat_id) {
                let _ = active.child.kill();
                let _ = active.child.wait();
            }
        }
        // Free any local-model VRAM lease held by this chat.
        crate::local_llm::manager().release(&crate::local_llm::chat_holder(chat_id));
    }

    pub fn close_all(&self) {
        if let Ok(mut map) = self.write_approvals.lock() {
            for (_, tx) in map.drain() {
                let _ = tx.send(false);
            }
        }
        if let Ok(mut guard) = self.sessions.lock() {
            for (_, mut active) in guard.drain() {
                let _ = active.child.kill();
                let _ = active.child.wait();
            }
        }
        crate::local_llm::manager().release_prefix("chat:");
    }

    pub fn ensure_session(
        &self,
        app: AppHandle,
        launch: &AcpLaunch,
        cwd: &str,
        chat_id: &str,
        stored_session_id: Option<&str>,
        provider_id: &str,
        model_id: Option<&str>,
        running_automations: usize,
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
        self.spawn_session(
            app,
            launch,
            cwd,
            chat_id,
            stored_session_id,
            provider_id,
            model_id,
            running_automations,
        )
    }

    fn touch(&self, chat_id: &str) {
        if let Ok(guard) = self.sessions.lock() {
            if let Some(session) = guard.get(chat_id) {
                session.bump_access();
            }
        }
    }

    fn evict_if_needed(&self) {
        let mut evicted: Vec<String> = Vec::new();
        {
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
                    evicted.push(id);
                } else {
                    break;
                }
            }
        }
        // Reject write approvals outside the sessions lock (lock order:
        // write_approvals before sessions in respond_permission / close_chat).
        for id in evicted {
            reject_write_approvals_for_chat(&self.write_approvals, &id);
        }
    }

    fn spawn_session(
        &self,
        app: AppHandle,
        launch: &AcpLaunch,
        cwd: &str,
        chat_id: &str,
        stored_session_id: Option<&str>,
        _provider_id: &str,
        _model_id: Option<&str>,
        _running_automations: usize,
    ) -> Result<String, String> {
        let mut command = crate::util::hidden_command(&launch.command);
        command
            .args(&launch.args)
            .envs(launch.env.iter().cloned())
            // S37: if Swerve Build was itself launched from inside a Claude Code
            // session (a terminal, an agent run), that session's `CLAUDECODE`
            // marker is inherited — and `claude-code-acp` then refuses to start
            // with "Claude Code cannot be launched inside another Claude Code
            // session", surfacing as a permanent "Connecting…". Our agent is a
            // separate session, so drop the inherited marker; unsetting it is
            // exactly what that error message instructs.
            .env_remove("CLAUDECODE")
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
            prompt_id: Mutex::new(None),
        });
        let transport_for_reader = Arc::clone(&transport);
        let app_for_reader = app.clone();
        let chat_for_reader = chat_id.to_string();
        // The agent's fs/read|write requests are confined to this project dir.
        let cwd_for_reader = cwd.to_string();
        // Shared with ActiveSession so streaming activity keeps this session warm
        // in the LRU — a mid-turn chat must not be evicted when a 4th chat opens.
        let last_accessed = Arc::new(AtomicU64::new(now_secs()));
        let last_accessed_reader = Arc::clone(&last_accessed);
        let write_approvals_for_reader = Arc::clone(&self.write_approvals);
        let sessions_for_cleanup = Arc::clone(&self.sessions);
        let transport_for_cleanup = Arc::clone(&transport);
        let write_approvals_for_cleanup = Arc::clone(&self.write_approvals);
        let chat_for_cleanup = chat_for_reader.clone();

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
                    if is_session_update_method(method) {
                        // Streaming counts as access so the LRU keeps a mid-turn
                        // session alive even if it hasn't been sent to recently.
                        last_accessed_reader.store(now_secs(), Ordering::SeqCst);
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
                        //
                        // Writes always go through the global approval UI so an
                        // agent cannot overwrite project files without a click.
                        let reply = if method == "fs/write_text_file" {
                            match handle_write_with_approval(
                                value.get("params"),
                                &cwd_for_reader,
                                &chat_for_reader,
                                &id,
                                &app_for_reader,
                                &write_approvals_for_reader,
                            ) {
                                Ok(result) => {
                                    json!({ "jsonrpc": "2.0", "id": id, "result": result })
                                }
                                Err((code, message)) => json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": { "code": code, "message": message },
                                }),
                            }
                        } else {
                            match handle_client_request(
                                method,
                                value.get("params"),
                                &cwd_for_reader,
                            ) {
                                Ok(result) => {
                                    json!({ "jsonrpc": "2.0", "id": id, "result": result })
                                }
                                Err((code, message)) => json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": { "code": code, "message": message },
                                }),
                            }
                        };
                        if let Ok(mut writer) = transport_for_reader.stdin.lock() {
                            let _ = writeln!(writer, "{}", reply);
                            let _ = writer.flush();
                        }
                        continue;
                    }
                }

                if let Some(id) = jsonrpc_id(&value) {
                    // The reply to the in-flight session/prompt means the turn is
                    // over, and every session/update for it has already been
                    // emitted above. Announce it on the SAME event channel so the
                    // UI finalizes after the last chunk, instead of racing the
                    // command's return value and clipping the tail.
                    let is_prompt_reply = match transport_for_reader.prompt_id.lock() {
                        Ok(mut slot) if *slot == Some(id) => {
                            *slot = None;
                            true
                        }
                        _ => false,
                    };
                    if is_prompt_reply {
                        // Forward optional end-turn / context usage from the prompt
                        // result when present (S14). UI only displays used+size;
                        // incomplete usage is ignored so we never invent numbers.
                        let mut payload = json!({ "chatId": chat_for_reader });
                        if let Some(usage) =
                            usage_payload_from_prompt_response(&value)
                        {
                            payload["usage"] = usage;
                        }
                        let _ = app_for_reader.emit("chat-turn-end", payload);
                    }

                    if let Ok(mut pending) = transport_for_reader.responses.lock() {
                        if let Some(sender) = pending.remove(&id) {
                            let _ = sender.send(value);
                        }
                    }
                }
            }

            // stdout closed: grok exited (crash, kill, or clean exit). Unblock any
            // in-flight RPC — dropping the pending senders makes their recv()
            // return Err at once, so a send_prompt waiting on a now-dead agent
            // fails fast instead of hanging out the full timeout — and tell the UI
            // this chat's session is gone so it can show a reconnect hint.
            if let Ok(mut pending) = transport_for_reader.responses.lock() {
                pending.clear();
            }
            // Deny any write approvals still waiting on this chat so the reader
            // doesn't hang until WRITE_APPROVAL_TIMEOUT after the process dies.
            reject_write_approvals_for_chat(&write_approvals_for_cleanup, &chat_for_cleanup);
            // Drop the dead session from the map only if it is still *this*
            // transport (a respawn may already own the chat_id slot).
            if let Ok(mut guard) = sessions_for_cleanup.lock() {
                let stale = guard
                    .get(&chat_for_cleanup)
                    .map(|s| Arc::ptr_eq(&s.transport, &transport_for_cleanup))
                    .unwrap_or(false);
                if stale {
                    if let Some(mut active) = guard.remove(&chat_for_cleanup) {
                        let _ = active.child.kill();
                        let _ = active.child.wait();
                    }
                    // Session died — drop the local-model lease so automations
                    // are not blocked by a ghost chat holder.
                    crate::local_llm::manager()
                        .release(&crate::local_llm::chat_holder(&chat_for_cleanup));
                }
            }
            let _ = app_for_reader.emit(
                "chat-session-ended",
                json!({ "chatId": chat_for_cleanup }),
            );
        });

        // Env pack is built on send (S21) so mid-session model/media/grant
        // changes re-inject. Fingerprint starts empty → first turn always injects.
        let mut active = ActiveSession {
            session_id: String::new(),
            child,
            transport: Arc::clone(&transport),
            last_accessed,
            env_fingerprint: Mutex::new(None),
        };

        let agent_caps = active.transport.rpc(
            "initialize",
            json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
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

        // Window advertised at initialize; session/new may restate it.
        let init_window = agent_caps
            .get("_meta")
            .and_then(|m| m.get("modelState"))
            .and_then(context_window_from_models);

        let (session_id, mut context_window) = if let Some(stored) =
            stored_session_id.filter(|_| can_load)
        {
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
                // A resumed session restates the model set the same way.
                Ok(v) => (
                    stored.to_string(),
                    v.get("models").and_then(context_window_from_models),
                ),
                Err(_) => Self::create_new_session(&mut active, cwd, &mcp_servers)?,
            }
        } else {
            Self::create_new_session(&mut active, cwd, &mcp_servers)?
        };

        active.session_id = session_id.clone();
        active.bump_access();

        // Fall back to what initialize advertised if the session call omitted it.
        if context_window.is_none() {
            context_window = init_window;
        }

        // S35: hand the UI a real context window up front so the bar can show a
        // true percentage from the first turn, instead of waiting for Grok's
        // ~80%-full auto-compaction notice to reveal it.
        let _ = app.emit(
            "chat-session-ready",
            json!({
                "chatId": chat_id,
                "sessionId": session_id,
                "contextWindow": context_window,
            }),
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
    ) -> Result<(String, Option<u64>), String> {
        let result = active.transport.rpc(
            "session/new",
            json!({
                "cwd": cwd,
                "mcpServers": mcp_servers,
            }),
            CONNECT_TIMEOUT_SECS,
        )?;

        let window = result.get("models").and_then(context_window_from_models);
        let id = result
            .get("sessionId")
            .and_then(|id| id.as_str())
            .map(|id| id.to_string())
            .ok_or_else(|| "ACP session/new did not return sessionId".to_string())?;
        Ok((id, window))
    }

    pub fn respond_permission(
        &self,
        chat_id: &str,
        request_id: u64,
        option_id: &str,
    ) -> Result<(), String> {
        // Client-side write approvals are parked in write_approvals (not as an
        // agent permission RPC). Resolve those first so the reader can finish
        // the fs/write_text_file reply.
        if let Ok(mut map) = self.write_approvals.lock() {
            if let Some(tx) = map.remove(&(chat_id.to_string(), request_id)) {
                let allowed = option_id.starts_with("allow");
                let _ = tx.send(allowed);
                return Ok(());
            }
        }

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
        let active_chats = self.list_active().len().max(1);
        let (transport, session_id, env_pack) = {
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| "ACP session lock poisoned".to_string())?;
            let active = guard
                .get_mut(chat_id)
                .ok_or_else(|| "No active session for this chat. Open the chat to connect.".to_string())?;
            active.bump_access();
            // S21: re-inject when env fingerprint changes (model, media, grants, …).
            // First turn: last fingerprint is None → always inject.
            let pack = active.env_fingerprint.lock().ok().and_then(|mut last| {
                crate::env_context::pack_for_chat_if_changed(
                    chat_id,
                    active_chats,
                    0,
                    &mut last,
                )
            });
            (
                Arc::clone(&active.transport),
                active.session_id.clone(),
                pack,
            )
        };

        // Build ACP content blocks: text (+ optional env pack) and real image
        // blocks when files are readable. Path-only text is a last-resort fallback
        // so agents that only understand prose still see the attachment.
        let mut blocks: Vec<Value> = Vec::new();

        // Delivery is a user-turn prefix because Grok ACP `session/new` does not
        // expose a client system-prompt field we can rely on.
        let mut body = text.trim().to_string();
        if let Some(pack) = env_pack {
            if body.is_empty() {
                body = pack;
            } else {
                body = format!("{pack}\n\n---\n\n{body}");
            }
        }
        if !body.is_empty() {
            blocks.push(json!({ "type": "text", "text": body }));
        }

        for (index, image) in images.iter().enumerate() {
            match image_path_to_acp_block(image) {
                Ok(block) => blocks.push(block),
                Err(_) => {
                    blocks.push(json!({
                        "type": "text",
                        "text": format!("[Image #{}: {}]", index + 1, image)
                    }));
                }
            }
        }

        if blocks.is_empty() {
            return Err("Message cannot be empty".to_string());
        }

        transport.rpc(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": blocks
            }),
            PROMPT_TIMEOUT_SECS,
        )?;

        Ok(())
    }

    /// Cancel the in-flight turn for a chat. ACP `session/cancel` is a
    /// notification (no response): the agent aborts the current turn and then
    /// responds to the pending `session/prompt`, so the waiting `send_prompt`
    /// unblocks and the caller saves whatever partial reply arrived.
    pub fn cancel_prompt(&self, chat_id: &str) -> Result<(), String> {
        let (transport, session_id) = {
            let guard = self
                .sessions
                .lock()
                .map_err(|_| "ACP session lock poisoned".to_string())?;
            let active = guard
                .get(chat_id)
                .ok_or_else(|| "No active session for this chat.".to_string())?;
            (Arc::clone(&active.transport), active.session_id.clone())
        };
        transport.write_json_line(&json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": session_id }
        }))
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
        // Remember the in-flight prompt so the reader can emit chat-turn-end when
        // its reply arrives — after every session/update for that turn.
        if method == "session/prompt" {
            if let Ok(mut slot) = self.prompt_id.lock() {
                *slot = Some(id);
            }
        }
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

/// Is this a session update we should forward to the UI?
///
/// Accepts the ACP-standard `session/update` **and** vendor-namespaced variants.
///
/// **S34b — verified on the wire, do not "simplify" this.** Grok's token usage
/// does NOT ride on `session/update`. It arrives as
/// **`_x.ai/session_notification`** carrying `turn_completed`
/// (`usage.inputTokens`, `totalTokens`, `modelUsage`) and `response_completed`
/// (snake_case `input_tokens`). Grok's own on-disk `updates.jsonl` records
/// those events as `session/update`, which is misleading — the stdio wire
/// method is `session_notification`. Confirmed with a minimal ACP client
/// against `grok agent stdio`.
/// Context window (tokens) the agent advertises for the session's current model.
///
/// Shape (Grok, on `session/new` and in `initialize._meta.modelState`):
/// ```json
/// { "currentModelId": "grok-4.5",
///   "availableModels": [ { "modelId": "grok-4.5",
///                          "_meta": { "totalContextTokens": 500000 } } ] }
/// ```
///
/// **S35 — why this source and not a model catalog:** `usage.modelUsage` reports
/// `grok-4.5-build`, an id that appears in NO catalog (`~/.grok/models_cache.json`
/// only has `grok-4.5`), so matching a catalog entry would mean guessing that
/// `-build` is a suffix to strip. This value comes straight from the agent for
/// the session we are actually running, so it is authoritative, not inferred.
fn context_window_from_models(models: &Value) -> Option<u64> {
    let current = models.get("currentModelId").and_then(|v| v.as_str())?;
    let list = models.get("availableModels").and_then(|v| v.as_array())?;
    list.iter()
        .find(|m| m.get("modelId").and_then(|v| v.as_str()) == Some(current))
        .and_then(|m| m.get("_meta"))
        .and_then(|meta| meta.get("totalContextTokens"))
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
}

fn is_session_update_method(method: &str) -> bool {
    method == "session/update"
        || method.ends_with("/session/update")
        // Vendor push notifications (Grok: `_x.ai/session_notification`).
        || method == "session_notification"
        || method.ends_with("/session_notification")
}

/// Extract a usage payload suitable for the UI from a `session/prompt` JSON-RPC
/// response. Prefer objects that include both `used` and `size` (session context
/// RFD). If only a nested `usage` object exists, forward it as-is so the client
/// can decide — it must still refuse to invent a context window size.
fn usage_payload_from_prompt_response(response: &Value) -> Option<Value> {
    let result = response.get("result")?;
    if result.is_null() {
        return None;
    }
    // Full result already looks like a usage_update (used + size).
    if result_has_used_and_size(result) {
        return Some(result.clone());
    }
    if let Some(usage) = result.get("usage") {
        if usage.is_object() {
            return Some(usage.clone());
        }
    }
    // Nested context / contextWindow shapes some agents may use.
    for key in ["context", "contextWindow", "context_window"] {
        if let Some(ctx) = result.get(key) {
            if result_has_used_and_size(ctx) {
                return Some(ctx.clone());
            }
        }
    }
    None
}

fn result_has_used_and_size(v: &Value) -> bool {
    let used_ok = v
        .get("used")
        .and_then(|x| x.as_f64().or_else(|| x.as_i64().map(|n| n as f64)))
        .is_some_and(|n| n.is_finite() && n >= 0.0);
    let size_ok = v
        .get("size")
        .and_then(|x| x.as_f64().or_else(|| x.as_i64().map(|n| n as f64)))
        .is_some_and(|n| n.is_finite() && n > 0.0);
    used_ok && size_ok
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

/// Handle an agent→client JSON-RPC request (read-only path and unknown methods).
/// Writes go through [`handle_write_with_approval`] instead — never auto-approved.
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
        // Defensive: write must never land here. The reader routes writes to the
        // approval path; if this is hit, refuse rather than auto-write.
        "fs/write_text_file" => Err((
            -32002,
            "fs/write_text_file requires user approval (internal routing error)".to_string(),
        )),
        other => Err((-32601, format!("method not supported: {other}"))),
    }
}

/// Gate `fs/write_text_file` behind the global permission modal, then write only
/// if the user allows and the path is still confined (junction-safe).
fn handle_write_with_approval(
    params: Option<&Value>,
    cwd: &str,
    chat_id: &str,
    request_id: &Value,
    app: &AppHandle,
    write_approvals: &Mutex<WriteApprovalMap>,
) -> Result<Value, (i64, String)> {
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

    let id = request_id
        .as_u64()
        .or_else(|| request_id.as_i64().filter(|&n| n >= 0).map(|n| n as u64))
        .or_else(|| request_id.as_str().and_then(|s| s.parse().ok()))
        .ok_or((-32602, "invalid request id".to_string()))?;

    let (tx, rx) = mpsc::channel();
    {
        let mut map = write_approvals
            .lock()
            .map_err(|_| (-32000, "write approval lock poisoned".to_string()))?;
        map.insert((chat_id.to_string(), id), tx);
    }

    // Reuse the same event + modal as session/request_permission so background
    // chats still surface the prompt (global permissionStore).
    let display_path = target.display().to_string();
    let preview_len = content.chars().count();
    let title = if preview_len == 0 {
        format!("Write empty file: {display_path}")
    } else {
        format!("Write {preview_len} chars to: {display_path}")
    };
    let _ = app.emit(
        "chat-permission-request",
        json!({
            "chatId": chat_id,
            "requestId": id,
            "params": {
                "toolCall": {
                    "title": title,
                    "kind": "edit",
                    "toolCallId": format!("fs-write-{id}")
                },
                "options": [
                    { "optionId": "allow_once", "name": "Allow write", "kind": "allow_once" },
                    { "optionId": "reject", "name": "Deny", "kind": "reject" }
                ]
            }
        }),
    );

    let allowed = match rx.recv_timeout(Duration::from_secs(WRITE_APPROVAL_TIMEOUT_SECS)) {
        Ok(v) => v,
        Err(_) => {
            // Timed out or sender dropped — drop our slot if still present.
            if let Ok(mut map) = write_approvals.lock() {
                map.remove(&(chat_id.to_string(), id));
            }
            return Err((-32003, "write approval timed out or cancelled".to_string()));
        }
    };

    if !allowed {
        return Err((-32003, format!("write denied by user: {display_path}")));
    }

    // Re-check confinement after approval (TOCTOU + junction safety).
    let target = confine_to_cwd(path, cwd)
        .ok_or((-32001, format!("path outside project directory: {path}")))?;
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&target, content).map_err(|e| (-32000, format!("write failed: {e}")))?;
    Ok(Value::Null)
}

fn reject_write_approvals_for_chat(
    write_approvals: &Mutex<WriteApprovalMap>,
    chat_id: &str,
) {
    if let Ok(mut map) = write_approvals.lock() {
        let keys: Vec<_> = map
            .keys()
            .filter(|(c, _)| c == chat_id)
            .cloned()
            .collect();
        for key in keys {
            if let Some(tx) = map.remove(&key) {
                let _ = tx.send(false);
            }
        }
    }
}

/// Resolve `requested` (absolute or relative to `cwd`) and return it only if the
/// final path, after resolving existing junctions/symlinks, stays inside the
/// canonicalized project directory. Pure lexical `..` collapse is not enough on
/// Windows: a directory junction inside the project can point outside.
///
/// `pub(crate)` so the terminal tool shares this one junction-safe check (audit
/// A4) rather than reimplementing confinement.
pub(crate) fn confine_to_cwd(requested: &str, cwd: &str) -> Option<PathBuf> {
    let root = strip_verbatim(&fs::canonicalize(cwd).ok()?);
    let req = Path::new(requested);
    let abs = if req.is_absolute() {
        req.to_path_buf()
    } else {
        root.join(req)
    };
    let normalized = normalize_lexical(&abs);

    // Fast reject before any further filesystem work.
    if !path_inside(&normalized, &root) {
        return None;
    }

    // Existing path: canonicalize fully (resolves junctions/symlinks).
    if normalized.exists() {
        let real = strip_verbatim(&fs::canonicalize(&normalized).ok()?);
        return path_inside(&real, &root).then_some(real);
    }

    // New path: walk up to the nearest existing ancestor, canonicalize that,
    // then re-join the non-existing tail and re-check the prefix.
    let mut ancestor = normalized.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if ancestor.exists() {
            break;
        }
        let name = ancestor.file_name()?.to_os_string();
        tail.push(name);
        ancestor = ancestor.parent()?.to_path_buf();
    }
    let real_ancestor = strip_verbatim(&fs::canonicalize(&ancestor).ok()?);
    if !path_inside(&real_ancestor, &root) {
        return None;
    }
    let mut final_path = real_ancestor;
    for part in tail.into_iter().rev() {
        final_path.push(part);
    }
    path_inside(&final_path, &root).then_some(final_path)
}

fn path_inside(path: &Path, root: &Path) -> bool {
    let n = path.to_string_lossy().to_lowercase();
    let r = root.to_string_lossy().to_lowercase();
    n == r || n.starts_with(&format!("{r}\\")) || n.starts_with(&format!("{r}/"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confine_rejects_parent_escape() {
        let dir = tempfile_dir("confine-escape");
        let outside = confine_to_cwd("../secrets.txt", dir.to_str().unwrap());
        assert!(outside.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confine_allows_relative_inside() {
        let dir = tempfile_dir("confine-ok");
        let nested = dir.join("src");
        fs::create_dir_all(&nested).unwrap();
        let target = confine_to_cwd("src/main.rs", dir.to_str().unwrap()).unwrap();
        assert!(path_inside(
            &target,
            &strip_verbatim(&fs::canonicalize(&dir).unwrap())
        ));
        assert!(target.ends_with("main.rs") || target.file_name().unwrap() == "main.rs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confine_allows_new_file_under_existing_dir() {
        let dir = tempfile_dir("confine-new");
        let target = confine_to_cwd("brand-new.txt", dir.to_str().unwrap()).unwrap();
        assert_eq!(target.file_name().unwrap(), "brand-new.txt");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_client_request_never_auto_writes() {
        let dir = tempfile_dir("no-auto-write");
        let err = handle_client_request(
            "fs/write_text_file",
            Some(&json!({ "path": "x.txt", "content": "nope" })),
            dir.to_str().unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.0, -32002);
        assert!(!dir.join("x.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// S35: the window must come from the agent, for the model actually in use.
    #[test]
    fn context_window_read_from_current_model() {
        let models = json!({
            "currentModelId": "grok-4.5",
            "availableModels": [
                { "modelId": "other", "_meta": { "totalContextTokens": 111 } },
                { "modelId": "grok-4.5", "_meta": { "totalContextTokens": 500000 } }
            ]
        });
        assert_eq!(context_window_from_models(&models), Some(500000));
    }

    #[test]
    fn context_window_absent_is_none_never_guessed() {
        // Unknown current model → no window (do NOT fall back to the first entry).
        let mismatch = json!({
            "currentModelId": "grok-4.5-build",
            "availableModels": [{ "modelId": "grok-4.5", "_meta": { "totalContextTokens": 500000 } }]
        });
        assert_eq!(context_window_from_models(&mismatch), None);
        // Missing/zero/garbage shapes all yield None rather than a wrong number.
        assert_eq!(context_window_from_models(&json!({})), None);
        assert_eq!(
            context_window_from_models(&json!({
                "currentModelId": "m",
                "availableModels": [{ "modelId": "m", "_meta": { "totalContextTokens": 0 } }]
            })),
            None
        );
        assert_eq!(
            context_window_from_models(&json!({
                "currentModelId": "m",
                "availableModels": [{ "modelId": "m" }]
            })),
            None
        );
    }

    /// S34/S34b: Grok's usage arrives on `_x.ai/session_notification` — NOT on
    /// `session/update`, despite its own logs recording it that way. Verified
    /// against `grok agent stdio` with a minimal ACP client.
    #[test]
    fn session_update_matcher_accepts_vendor_namespaces() {
        assert!(is_session_update_method("session/update"));
        assert!(is_session_update_method("_x.ai/session/update"));
        assert!(is_session_update_method("acme.dev/session/update"));
        // The one that actually carries Grok's token usage on the wire.
        assert!(is_session_update_method("_x.ai/session_notification"));
        assert!(is_session_update_method("session_notification"));
        // Must not widen into other session methods or unrelated traffic.
        assert!(!is_session_update_method("session/request_permission"));
        assert!(!is_session_update_method("_x.ai/session/other"));
        assert!(!is_session_update_method("session/updated"));
        assert!(!is_session_update_method("_x.ai/fs_notify"));
        assert!(!is_session_update_method("initialize"));
    }

    #[test]
    fn usage_from_prompt_result_used_size() {
        let resp = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "stopReason": "end_turn", "used": 53000, "size": 200000 }
        });
        let usage = usage_payload_from_prompt_response(&resp).unwrap();
        assert_eq!(usage["used"], 53000);
        assert_eq!(usage["size"], 200000);
    }

    #[test]
    fn usage_from_prompt_result_nested_usage_object() {
        let resp = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "stopReason": "end_turn",
                "usage": { "totalTokens": 100, "inputTokens": 80, "outputTokens": 20 }
            }
        });
        let usage = usage_payload_from_prompt_response(&resp).unwrap();
        // Forwarded as-is; client will not treat this as context used/size.
        assert_eq!(usage["totalTokens"], 100);
        assert!(usage.get("used").is_none());
    }

    #[test]
    fn usage_from_prompt_result_absent() {
        let resp = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "stopReason": "end_turn" }
        });
        assert!(usage_payload_from_prompt_response(&resp).is_none());
    }

    #[test]
    fn image_path_to_acp_block_reads_png_bytes() {
        let dir = tempfile_dir("img-block");
        // Minimal 1x1 PNG
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x05, 0xfe,
            0xd4, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let path = dir.join("dot.png");
        fs::write(&path, png).unwrap();
        let block = image_path_to_acp_block(path.to_str().unwrap()).unwrap();
        assert_eq!(block["type"], "image");
        assert_eq!(block["mimeType"], "image/png");
        assert!(block["data"].as_str().unwrap().len() > 8);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_attachment_files_copies_images_only() {
        let dir = tempfile_dir("import-att");
        let img = dir.join("a.jpg");
        let txt = dir.join("notes.txt");
        fs::write(&img, b"fake-jpeg-bytes").unwrap();
        fs::write(&txt, b"not an image").unwrap();
        let imported = import_attachment_files(&[
            img.display().to_string(),
            txt.display().to_string(),
            dir.join("missing.png").display().to_string(),
        ])
        .unwrap();
        assert_eq!(imported.len(), 1);
        assert!(Path::new(&imported[0]).is_file());
        assert!(
            imported[0].contains("attachments")
                || Path::new(&imported[0]).extension().is_some()
        );
        let _ = fs::remove_dir_all(&dir);
        // Clean the copied attachment if under real attachments dir.
        if let Some(p) = imported.first().map(PathBuf::from) {
            let _ = fs::remove_file(p);
        }
    }

    fn tempfile_dir(label: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "swerve-acp-{}-{}-{}",
            label,
            std::process::id(),
            now_secs()
        ));
        fs::create_dir_all(&path).unwrap();
        // Touch so canonicalize works on empty dirs everywhere.
        fs::write(path.join(".keep"), b"x").unwrap();
        path
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

    let (meta, payload) = match data.split_once(',') {
        Some((head, body)) => (head, body),
        None => ("", data),
    };

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("Invalid image data: {e}"))?;

    if bytes.len() as u64 > IMAGE_ATTACH_MAX_BYTES {
        return Err(format!(
            "Image too large (max {} MB)",
            IMAGE_ATTACH_MAX_BYTES / (1024 * 1024)
        ));
    }

    let ext = ext_from_data_url_meta(meta).unwrap_or("png");
    let dir = attachments_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.join(format!("{}.{ext}", Store::new_id()));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Copy user-selected image files into the attachments dir (asset-protocol scope).
/// Skips non-images / oversize files; returns only successful paths.
pub fn import_attachment_files(paths: &[String]) -> Result<Vec<String>, String> {
    let dir = attachments_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for raw in paths {
        let src = PathBuf::from(raw);
        if !src.is_file() {
            continue;
        }
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !IMAGE_ATTACH_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let meta = fs::metadata(&src).map_err(|e| e.to_string())?;
        if meta.len() > IMAGE_ATTACH_MAX_BYTES {
            continue;
        }
        let dest = dir.join(format!("{}.{ext}", Store::new_id()));
        fs::copy(&src, &dest).map_err(|e| format!("Failed to import attachment: {e}"))?;
        out.push(dest.display().to_string());
    }
    Ok(out)
}

const IMAGE_ATTACH_MAX_BYTES: u64 = 25 * 1024 * 1024;
const IMAGE_ATTACH_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

fn ext_from_data_url_meta(meta: &str) -> Option<&'static str> {
    // data:image/png;base64
    let lower = meta.to_ascii_lowercase();
    if lower.contains("image/jpeg") || lower.contains("image/jpg") {
        Some("jpg")
    } else if lower.contains("image/png") {
        Some("png")
    } else if lower.contains("image/gif") {
        Some("gif")
    } else if lower.contains("image/webp") {
        Some("webp")
    } else if lower.contains("image/bmp") {
        Some("bmp")
    } else if lower.contains("image/svg") {
        Some("svg")
    } else {
        None
    }
}

fn mime_from_path(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ => "image/png",
    }
}

/// Read a local image file into an ACP `image` content block.
fn image_path_to_acp_block(path: &str) -> Result<Value, String> {
    use base64::Engine;

    let p = Path::new(path);
    if !p.is_file() {
        return Err("not a file".into());
    }
    let meta = fs::metadata(p).map_err(|e| e.to_string())?;
    if meta.len() > IMAGE_ATTACH_MAX_BYTES {
        return Err("image too large".into());
    }
    let bytes = fs::read(p).map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(json!({
        "type": "image",
        "mimeType": mime_from_path(path),
        "data": b64
    }))
}