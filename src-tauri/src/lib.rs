mod acp;
mod grok_config;
mod jobs;
mod local_llm;
pub mod paths;
mod providers;
mod store;
mod util;

use acp::AcpManager;
use jobs::JobManager;
use providers::{ProviderStatus, ProviderView};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use store::{AppStore, Chat, ChatMessage, Project, Store};
use std::sync::Arc;
use tauri::{Manager, State};

#[derive(Serialize)]
pub struct GrokStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub authenticated: bool,
}

#[derive(Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub path: String,
    pub description: String,
    pub source: String,
}

#[derive(Serialize)]
pub struct CommandResult {
    pub success: bool,
    pub message: String,
}

pub(crate) fn grok_home() -> PathBuf {
    // Match grok's own resolution: `$GROK_HOME` overrides the default `~/.grok`.
    if let Some(dir) = std::env::var_os("GROK_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
}

fn grok_bin() -> PathBuf {
    grok_home().join("bin").join("grok.exe")
}

pub fn resolve_grok_executable() -> Option<PathBuf> {
    if let Some(path) = which_on_path("grok") {
        return Some(path);
    }

    let bundled = grok_bin();
    if bundled.is_file() {
        return Some(bundled);
    }

    None
}

pub(crate) fn which_on_path(command: &str) -> Option<PathBuf> {
    let output = util::hidden_command("where").arg(command).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .next()
        .map(|line| PathBuf::from(line.trim()))
        .filter(|path| !path.as_os_str().is_empty())
}

fn grok_version_at(path: &Path) -> Option<String> {
    let output = util::hidden_command(path).arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|line| line.trim().to_string())
}

fn is_authenticated() -> bool {
    grok_home().join("auth.json").is_file()
}

#[tauri::command]
fn get_grok_status() -> GrokStatus {
    let path = resolve_grok_executable();

    if path.is_none() {
        return GrokStatus {
            installed: false,
            version: None,
            path: None,
            authenticated: is_authenticated(),
        };
    }

    let path = path.unwrap();
    let version = grok_version_at(&path);

    GrokStatus {
        installed: true,
        version,
        path: Some(path.display().to_string()),
        authenticated: is_authenticated(),
    }
}

#[tauri::command]
fn install_grok() -> CommandResult {
    let script = "irm https://x.ai/cli/install.ps1 | iex";
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .status();

    match status {
        Ok(result) if result.success() => CommandResult {
            success: true,
            message: "Grok Build installed successfully.".into(),
        },
        Ok(result) => CommandResult {
            success: false,
            message: format!("Installer exited with code {:?}", result.code()),
        },
        Err(error) => CommandResult {
            success: false,
            message: format!("Failed to run installer: {error}"),
        },
    }
}

fn spawn_hidden_grok_login(grok: &Path) -> std::io::Result<()> {
    util::hidden_command(grok)
        .arg("login")
        .arg("--oauth")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[tauri::command]
fn open_grok_login() -> CommandResult {
    let grok = match resolve_grok_executable() {
        Some(path) => path,
        None => {
            return CommandResult {
                success: false,
                message: "Grok Build is not installed.".into(),
            };
        }
    };

    match spawn_hidden_grok_login(&grok) {
        Ok(()) => CommandResult {
            success: true,
            message: "Browser sign-in started. Complete login in your browser.".into(),
        },
        Err(error) => CommandResult {
            success: false,
            message: format!("Failed to start sign-in: {error}"),
        },
    }
}

#[tauri::command]
fn check_grok_updates() -> CommandResult {
    // Read grok's own version state from ~/.grok/version.json, which its
    // background auto-updater keeps fresh. Spawning `grok update --check` returns
    // "program not found" when launched from this app's process context (it works
    // from a shell — an environment quirk we couldn't reproduce or pin down), so
    // we read the authoritative file instead: instant, and no fragile subprocess.
    if resolve_grok_executable().is_none() {
        return CommandResult {
            success: false,
            message: "Grok Build is not installed.".into(),
        };
    }

    let path = grok_home().join("version.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => {
            // No cached file yet — report the installed version directly.
            let installed = resolve_grok_executable()
                .as_deref()
                .and_then(grok_version_at)
                .unwrap_or_else(|| "unknown version".to_string());
            return CommandResult {
                success: true,
                message: format!("Installed: {installed}. Grok manages its own updates."),
            };
        }
    };

    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return CommandResult {
            success: true,
            message: "Grok Build manages its own updates.".into(),
        };
    };

    let current = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
    let latest = v
        .get("stable_version")
        .and_then(|x| x.as_str())
        .unwrap_or(current);

    let message = if !latest.is_empty() && latest != "?" && latest != current {
        format!("Update available: v{current} → v{latest}. Grok auto-updates; run `grok update` to apply it now.")
    } else {
        format!("Grok Build is up to date — v{current}.")
    };

    CommandResult {
        success: true,
        message,
    }
}

fn memory_file() -> PathBuf {
    grok_home().join("memory").join("MEMORY.md")
}

#[tauri::command]
fn read_memory() -> String {
    let path = memory_file();
    if !path.exists() {
        return String::new();
    }

    fs::read_to_string(path).unwrap_or_default()
}

#[tauri::command]
fn write_memory(content: String) -> CommandResult {
    let path = memory_file();

    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return CommandResult {
                success: false,
                message: format!("Failed to create memory directory: {error}"),
            };
        }
    }

    match fs::write(&path, content) {
        Ok(()) => CommandResult {
            success: true,
            message: path.display().to_string(),
        },
        Err(error) => CommandResult {
            success: false,
            message: format!("Failed to save memory: {error}"),
        },
    }
}

fn parse_skill_description(content: &str) -> String {
    let mut lines = content.lines();

    while let Some(line) = lines.next() {
        if line.starts_with("description:") {
            return line
                .trim_start_matches("description:")
                .trim()
                .trim_matches('"')
                .to_string();
        }
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed != "---" {
            return trimmed.to_string();
        }
    }

    "No description".to_string()
}

fn collect_skills_in(root: &Path, source: &str, skills: &mut Vec<SkillInfo>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string();

        let description = fs::read_to_string(&skill_file)
            .map(|content| parse_skill_description(&content))
            .unwrap_or_else(|_| "No description".to_string());

        skills.push(SkillInfo {
            name,
            path: skill_file.display().to_string(),
            description,
            source: source.to_string(),
        });
    }
}

#[tauri::command]
fn list_skills() -> Vec<SkillInfo> {
    let home = grok_home();
    let mut skills = Vec::new();

    collect_skills_in(&home.join("skills"), "user", &mut skills);
    collect_skills_in(&home.join("bundled").join("skills"), "bundled", &mut skills);

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

#[tauri::command]
fn get_workspace() -> AppStore {
    Store::load()
}

#[tauri::command]
fn add_project(path: String) -> Result<Project, String> {
    let _guard = Store::lock();
    let mut store = Store::load();
    let now = Store::now();

    if let Some(existing_id) = store
        .projects
        .iter()
        .find(|p| p.path == path)
        .map(|p| p.id.clone())
    {
        Store::touch_project(&mut store, &existing_id);
        let existing = store
            .projects
            .iter()
            .find(|p| p.id == existing_id)
            .cloned()
            .ok_or_else(|| "Project not found".to_string())?;
        Store::save(&store)?;
        return Ok(existing);
    }

    let project = Project {
        id: Store::new_id(),
        name: Store::project_name_from_path(&path),
        path,
        created_at: now.clone(),
        last_opened_at: now,
    };

    store.projects.push(project.clone());
    store.projects.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
    Store::save(&store)?;
    Ok(project)
}

#[tauri::command]
fn remove_project(project_id: String) -> Result<(), String> {
    let _guard = Store::lock();
    let mut store = Store::load();
    store.projects.retain(|p| p.id != project_id);
    store.chats.retain(|c| c.project_id != project_id);
    Store::save(&store)
}

#[tauri::command]
fn create_chat(project_id: String, title: Option<String>) -> Result<Chat, String> {
    let _guard = Store::lock();
    let mut store = Store::load();
    let resolved_project_id = store
        .projects
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.id.clone())
        .ok_or_else(|| "Project not found".to_string())?;

    let now = Store::now();
    let chat = Chat {
        id: Store::new_id(),
        project_id: resolved_project_id.clone(),
        title: title.unwrap_or_else(|| "New chat".to_string()),
        created_at: now.clone(),
        updated_at: now,
        messages: Vec::new(),
        grok_session_id: None,
        provider_id: None,
        model_id: None,
    };

    Store::touch_project(&mut store, &resolved_project_id);
    store.chats.push(chat.clone());
    Store::save(&store)?;
    Ok(chat)
}

#[tauri::command]
fn remove_chat(chat_id: String, acp: State<'_, Arc<AcpManager>>) -> Result<(), String> {
    acp.close_chat(&chat_id);
    let _guard = Store::lock();
    let mut store = Store::load();
    store.chats.retain(|c| c.id != chat_id);
    Store::save(&store)
}

#[tauri::command]
fn rename_chat(chat_id: String, title: String) -> Result<Chat, String> {
    let _guard = Store::lock();
    let mut store = Store::load();
    let chat = store
        .chats
        .iter_mut()
        .find(|c| c.id == chat_id)
        .ok_or_else(|| "Chat not found".to_string())?;

    chat.title = title.trim().to_string();
    chat.updated_at = Store::now();
    let updated = chat.clone();
    Store::save(&store)?;
    Ok(updated)
}

#[tauri::command]
fn get_chat(chat_id: String) -> Result<Chat, String> {
    let store = Store::load();
    store
        .chats
        .iter()
        .find(|c| c.id == chat_id)
        .cloned()
        .ok_or_else(|| "Chat not found".to_string())
}

#[tauri::command]
fn append_chat_message(
    chat_id: String,
    role: String,
    content: String,
    images: Vec<String>,
) -> Result<ChatMessage, String> {
    let _guard = Store::lock();
    let mut store = Store::load();
    let chat = store
        .chats
        .iter_mut()
        .find(|c| c.id == chat_id)
        .ok_or_else(|| "Chat not found".to_string())?;

    let message = ChatMessage {
        id: Store::new_id(),
        role,
        content,
        images,
        created_at: Store::now(),
    };

    if chat.title == "New chat" && message.role == "user" && !message.content.trim().is_empty() {
        chat.title = Store::chat_title_from_message(&message.content);
    }

    chat.messages.push(message.clone());
    chat.updated_at = Store::now();
    Store::save(&store)?;
    Ok(message)
}

#[tauri::command]
fn save_pasted_image(data_url: String) -> Result<String, String> {
    acp::save_image_base64(&data_url)
}

#[tauri::command]
async fn start_chat_session(
    app: tauri::AppHandle,
    acp: State<'_, Arc<AcpManager>>,
    chat_id: String,
) -> Result<CommandResult, String> {
    let store = Store::load();
    let chat = store
        .chats
        .iter()
        .find(|c| c.id == chat_id)
        .ok_or_else(|| "Chat not found".to_string())?
        .clone();
    let project = store
        .projects
        .iter()
        .find(|p| p.id == chat.project_id)
        .ok_or_else(|| "Project not found".to_string())?;

    // Tight lock scope: touch updated_at and release BEFORE the session spawn,
    // which later calls save_grok_session_id (also a store writer that locks).
    {
        let _guard = Store::lock();
        let mut fresh = Store::load();
        if let Some(entry) = fresh.chats.iter_mut().find(|c| c.id == chat_id) {
            entry.updated_at = Store::now();
            Store::save(&fresh)?;
        }
    }

    let provider = chat
        .provider_id
        .clone()
        .and_then(|id| providers::get_provider(&id))
        .or_else(|| providers::get_provider(&providers::active_id()))
        .ok_or_else(|| "No provider configured".to_string())?;
    let launch = providers::resolve_launch(&provider, chat.model_id.as_deref())?;
    let project_path = project.path.clone();
    let stored_session = chat.grok_session_id.clone();
    let chat_id_for_task = chat_id.clone();
    let acp = acp.inner().clone();
    let acp_for_task = Arc::clone(&acp);
    // Local models need the app's llama-server up before grok spawns. Done in
    // the blocking task — first load of a big GGUF can take minutes.
    let local_model = chat
        .model_id
        .clone()
        .filter(|m| provider.id == "grok" && m.starts_with(grok_config::LOCAL_PREFIX));

    let session_id = tauri::async_runtime::spawn_blocking(move || {
        if let Some(model) = local_model.as_deref() {
            if let Some(other) = local_model_conflict(&acp_for_task, model) {
                return Err(format!(
                    "One local model runs at a time — \"{other}\" is connected on another local model. Close that chat first."
                ));
            }
            local_llm::manager().ensure_for_model(&app, model)?;
        }
        acp_for_task.ensure_session(
            app,
            &launch,
            &project_path,
            &chat_id_for_task,
            stored_session.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("Failed to start chat session: {e}"))??;

    acp::save_grok_session_id(&chat_id, &session_id)?;

    let active = acp.list_active();
    Ok(CommandResult {
        success: true,
        message: format!(
            "{} · {} active session(s)",
            project.path,
            active.len()
        ),
    })
}

#[tauri::command]
fn list_active_chat_sessions(acp: State<'_, Arc<AcpManager>>) -> Vec<String> {
    acp.list_active()
}

#[tauri::command]
fn close_chat_session(
    acp: State<'_, Arc<AcpManager>>,
    chat_id: Option<String>,
) -> Result<(), String> {
    if let Some(id) = chat_id {
        acp.close_chat(&id);
    } else {
        acp.close_all();
    }
    Ok(())
}

#[tauri::command]
fn respond_chat_permission(
    acp: State<'_, Arc<AcpManager>>,
    chat_id: String,
    request_id: u64,
    option_id: String,
) -> Result<(), String> {
    acp.respond_permission(&chat_id, request_id, &option_id)
}

#[tauri::command]
async fn send_chat_message(
    acp: State<'_, Arc<AcpManager>>,
    chat_id: String,
    text: String,
    images: Vec<String>,
) -> Result<(), String> {
    let acp = acp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || acp.send_prompt(&chat_id, &text, &images))
        .await
        .map_err(|e| format!("Failed to send message: {e}"))??;
    Ok(())
}

#[tauri::command]
fn cancel_chat_prompt(
    acp: State<'_, Arc<AcpManager>>,
    chat_id: String,
) -> Result<(), String> {
    acp.cancel_prompt(&chat_id)
}

#[tauri::command]
fn list_providers() -> Vec<ProviderView> {
    providers::views()
}

#[tauri::command]
fn get_active_provider() -> Result<ProviderView, String> {
    let views = providers::views();
    views
        .iter()
        .find(|v| v.active)
        .cloned()
        .or_else(|| views.first().cloned())
        .ok_or_else(|| "No providers available".to_string())
}

#[tauri::command]
fn set_active_provider(id: String, model: Option<String>) -> Result<ProviderView, String> {
    let provider =
        providers::get_provider(&id).ok_or_else(|| format!("Unknown provider: {id}"))?;
    if matches!(provider.kind, providers::ProviderKind::Http) {
        return Err(format!(
            "{} isn't available for chat sessions yet.",
            provider.label
        ));
    }
    if !providers::is_available(&provider) {
        return Err(format!(
            "{} is not available — its CLI wasn't found on your PATH.",
            provider.label
        ));
    }

    let mut store = providers::ProviderStore::load();
    store.active = Some(id.clone());
    store.model = model;
    store.save()?;

    providers::views()
        .into_iter()
        .find(|v| v.provider.id == id)
        .ok_or_else(|| "Provider not found after save".to_string())
}

#[tauri::command]
fn get_provider_status(id: String) -> ProviderStatus {
    providers::provider_status(&id)
}

#[tauri::command]
fn test_provider(id: String) -> CommandResult {
    let (success, message) = providers::test(&id);
    CommandResult { success, message }
}

// ---------------------------------------------------- custom Grok endpoint

/// What the Settings UI receives. The API key is never sent back — only whether
/// one is stored — so the secret doesn't round-trip into the frontend.
#[derive(Serialize)]
struct GrokEndpointView {
    enabled: bool,
    base_url: String,
    model: String,
    api_backend: String,
    context_window: Option<u32>,
    has_api_key: bool,
    config_path: String,
}

impl GrokEndpointView {
    fn current() -> Self {
        let endpoint = providers::get_endpoint();
        GrokEndpointView {
            enabled: endpoint.enabled,
            base_url: endpoint.base_url,
            model: endpoint.model,
            api_backend: endpoint.api_backend,
            context_window: endpoint.context_window,
            has_api_key: !endpoint.api_key.is_empty(),
            config_path: grok_config::config_file_display(),
        }
    }
}

#[derive(serde::Deserialize)]
struct GrokEndpointInput {
    enabled: bool,
    base_url: String,
    model: String,
    #[serde(default)]
    api_backend: String,
    #[serde(default)]
    context_window: Option<u32>,
    /// `None` keeps the stored key; `Some("")` clears it; `Some(k)` replaces it.
    #[serde(default)]
    api_key: Option<String>,
}

#[tauri::command]
fn get_grok_endpoint() -> GrokEndpointView {
    GrokEndpointView::current()
}

#[tauri::command]
fn set_grok_endpoint(input: GrokEndpointInput) -> Result<GrokEndpointView, String> {
    if input.enabled {
        if input.base_url.trim().is_empty() {
            return Err("Enter a Base URL before turning routing on.".into());
        }
        if input.model.trim().is_empty() {
            return Err("Enter a Model id before turning routing on.".into());
        }
    }

    let endpoint = providers::GrokEndpoint {
        enabled: input.enabled,
        base_url: input.base_url,
        model: input.model,
        api_key: String::new(), // resolved inside save_endpoint from `new_key`
        api_backend: input.api_backend,
        context_window: input.context_window,
        previous_default: None, // managed inside save_endpoint
    };
    providers::save_endpoint(endpoint, input.api_key)?;
    Ok(GrokEndpointView::current())
}

#[tauri::command]
fn test_grok_endpoint() -> CommandResult {
    let (success, message) = grok_config::verify();
    CommandResult { success, message }
}

// ------------------------------------------------------------ model registry

#[tauri::command]
fn list_models() -> Vec<providers::ModelInfo> {
    providers::list_models()
}

/// Pin a chat to a model (None/blank clears back to the agent default). The
/// switch takes effect on the next session spawn — the frontend closes and
/// restarts the session, and `session/load` restores the conversation.
#[tauri::command]
fn set_chat_model(chat_id: String, model_id: Option<String>) -> Result<(), String> {
    let _guard = Store::lock();
    let mut store = Store::load();
    let chat = store
        .chats
        .iter_mut()
        .find(|c| c.id == chat_id)
        .ok_or_else(|| "Chat not found".to_string())?;
    chat.model_id = model_id.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    chat.updated_at = Store::now();
    Store::save(&store)
}

#[tauri::command]
fn set_custom_model_ids(ids: Vec<String>) -> Result<Vec<providers::ModelInfo>, String> {
    let mut store = providers::ProviderStore::load();
    store.custom_model_ids = ids
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    store.save()?;
    Ok(providers::list_models())
}

// ------------------------------------------------------------- local models

#[derive(Serialize)]
struct LocalState {
    engine_installed: bool,
    engine_version: String,
    server: local_llm::ServerStatus,
    models: Vec<providers::LocalModel>,
}

impl LocalState {
    fn current() -> Self {
        LocalState {
            engine_installed: local_llm::engine_installed(),
            engine_version: local_llm::ENGINE_TAG.to_string(),
            server: local_llm::manager().status(),
            models: providers::ProviderStore::load().local.models,
        }
    }
}

#[tauri::command]
fn get_local_state() -> LocalState {
    LocalState::current()
}

#[tauri::command]
async fn install_local_engine(app: tauri::AppHandle) -> Result<CommandResult, String> {
    let message = tauri::async_runtime::spawn_blocking(move || local_llm::install_engine(&app))
        .await
        .map_err(|e| format!("install task failed: {e}"))??;
    Ok(CommandResult { success: true, message })
}

#[tauri::command]
fn add_local_model(path: String) -> Result<LocalState, String> {
    providers::add_local_model(path)?;
    Ok(LocalState::current())
}

#[tauri::command]
fn remove_local_model(app: tauri::AppHandle, id: String) -> Result<LocalState, String> {
    // If the server is currently serving this model, stop it first.
    let status = local_llm::manager().status();
    if status.model_id.as_deref() == Some(id.as_str()) {
        local_llm::manager().stop(&app);
    }
    providers::remove_local_model(&id)?;
    Ok(LocalState::current())
}

/// Preload a local model (start the server) without opening a chat.
#[tauri::command]
async fn start_local_server(app: tauri::AppHandle, model_id: String) -> Result<LocalState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        local_llm::manager().ensure_for_model(&app, &model_id)
    })
    .await
    .map_err(|e| format!("start task failed: {e}"))??;
    Ok(LocalState::current())
}

#[tauri::command]
fn stop_local_server(app: tauri::AppHandle) -> LocalState {
    local_llm::manager().stop(&app);
    LocalState::current()
}

/// One local model runs at a time; block a swap while another ACTIVE chat is
/// pinned to a different local model (its next message would hit the wrong
/// server). Idle chats are fine — they respawn on their own model later.
fn local_model_conflict(acp: &AcpManager, want: &str) -> Option<String> {
    let active = acp.list_active();
    let store = Store::load();
    store
        .chats
        .iter()
        .filter(|c| active.contains(&c.id))
        .find(|c| {
            c.model_id
                .as_deref()
                .map(|m| m.starts_with(grok_config::LOCAL_PREFIX) && m != want)
                .unwrap_or(false)
        })
        .map(|c| c.title.clone())
}

// -------------------------------------------------------------- automations

#[tauri::command]
fn list_automations() -> Vec<jobs::Automation> {
    jobs::list_automations()
}

#[tauri::command]
fn save_automation(
    job_mgr: State<'_, Arc<JobManager>>,
    automation: jobs::Automation,
) -> Result<jobs::Automation, String> {
    let saved = jobs::save_automation(automation)?;
    job_mgr.wake();
    Ok(saved)
}

#[tauri::command]
fn delete_automation(job_mgr: State<'_, Arc<JobManager>>, id: String) -> Result<(), String> {
    jobs::delete_automation(&id)?;
    job_mgr.wake();
    Ok(())
}

#[tauri::command]
async fn run_automation_now(
    app: tauri::AppHandle,
    job_mgr: State<'_, Arc<JobManager>>,
    automation_id: String,
) -> Result<String, String> {
    let automation =
        jobs::get_automation(&automation_id).ok_or_else(|| "Automation not found".to_string())?;
    let jm = job_mgr.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        jm.start_run(app, automation, "manual".to_string(), 0)
    })
    .await
    .map_err(|e| format!("Failed to start run: {e}"))?
}

#[tauri::command]
fn cancel_run(job_mgr: State<'_, Arc<JobManager>>, run_id: String) -> Result<(), String> {
    job_mgr.cancel_run(&run_id)
}

#[tauri::command]
fn set_automations_paused(
    job_mgr: State<'_, Arc<JobManager>>,
    paused: bool,
) -> Result<(), String> {
    job_mgr.set_paused(paused);
    job_mgr.wake();
    Ok(())
}

#[tauri::command]
fn get_automations_paused() -> bool {
    jobs::is_paused()
}

#[tauri::command]
fn list_automation_runs(automation_id: String) -> Vec<jobs::RunRecord> {
    jobs::list_runs(&automation_id)
}

#[tauri::command]
fn read_run_log(automation_id: String, run_id: String) -> Result<String, String> {
    jobs::read_run_log(&automation_id, &run_id)
}

#[tauri::command]
fn mark_runs_seen(automation_id: String, run_ids: Vec<String>) -> Result<(), String> {
    jobs::mark_runs_seen(&automation_id, run_ids)
}

#[tauri::command]
fn automation_failure_count() -> usize {
    jobs::unseen_failure_count()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let acp = Arc::new(AcpManager::default());
    let acp_exit = acp.clone();
    let job_mgr = Arc::new(JobManager::default());
    let jobs_exit = job_mgr.clone();
    let jobs_sched = job_mgr.clone();

    tauri::Builder::default()
        // Must be the FIRST plugin: a second launch focuses the existing window
        // instead of starting a second app — which would run a second scheduler
        // and a second data.json writer, defeating the in-process store lock.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(acp)
        .manage(job_mgr)
        .setup(move |app| {
            jobs::spawn_scheduler(app.handle().clone(), jobs_sched);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_grok_status,
            install_grok,
            open_grok_login,
            check_grok_updates,
            read_memory,
            write_memory,
            list_skills,
            get_workspace,
            add_project,
            remove_project,
            create_chat,
            remove_chat,
            rename_chat,
            get_chat,
            append_chat_message,
            save_pasted_image,
            start_chat_session,
            list_active_chat_sessions,
            close_chat_session,
            respond_chat_permission,
            send_chat_message,
            cancel_chat_prompt,
            list_providers,
            get_active_provider,
            set_active_provider,
            get_provider_status,
            test_provider,
            get_grok_endpoint,
            set_grok_endpoint,
            test_grok_endpoint,
            list_models,
            set_chat_model,
            set_custom_model_ids,
            get_local_state,
            install_local_engine,
            add_local_model,
            remove_local_model,
            start_local_server,
            stop_local_server,
            list_automations,
            save_automation,
            delete_automation,
            run_automation_now,
            cancel_run,
            set_automations_paused,
            get_automations_paused,
            list_automation_runs,
            read_run_log,
            mark_runs_seen,
            automation_failure_count
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(move |_app, event| {
            if let tauri::RunEvent::Exit = event {
                acp_exit.close_all();
                jobs_exit.cancel_all();
                local_llm::manager().shutdown();
            }
        });
}