mod acp;
pub mod paths;
mod providers;
mod store;

use acp::AcpManager;
use providers::{ProviderStatus, ProviderView};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use store::{AppStore, Chat, ChatMessage, Project, Store};
use std::sync::Arc;
use tauri::State;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

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

fn grok_home() -> PathBuf {
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
    let output = Command::new("where").arg(command).output().ok()?;

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
    let output = Command::new(path).arg("--version").output().ok()?;

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
    let mut command = Command::new(grok);
    command
        .arg("login")
        .arg("--oauth")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command.spawn().map(|_| ())
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
    let grok = match resolve_grok_executable() {
        Some(path) => path,
        None => {
            return CommandResult {
                success: false,
                message: "Grok Build is not installed.".into(),
            };
        }
    };

    let output = Command::new(grok).arg("update").arg("--check").output();

    match output {
        Ok(result) => {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            let message = if stdout.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };

            CommandResult {
                success: result.status.success(),
                message: if message.is_empty() {
                    "Update check completed.".into()
                } else {
                    message
                },
            }
        }
        Err(error) => CommandResult {
            success: false,
            message: format!("Failed to check updates: {error}"),
        },
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
    let mut store = Store::load();
    store.projects.retain(|p| p.id != project_id);
    store.chats.retain(|c| c.project_id != project_id);
    Store::save(&store)
}

#[tauri::command]
fn create_chat(project_id: String, title: Option<String>) -> Result<Chat, String> {
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
    };

    Store::touch_project(&mut store, &resolved_project_id);
    store.chats.push(chat.clone());
    Store::save(&store)?;
    Ok(chat)
}

#[tauri::command]
fn remove_chat(chat_id: String, acp: State<'_, Arc<AcpManager>>) -> Result<(), String> {
    acp.close_chat(&chat_id);
    let mut store = Store::load();
    store.chats.retain(|c| c.id != chat_id);
    Store::save(&store)
}

#[tauri::command]
fn rename_chat(chat_id: String, title: String) -> Result<Chat, String> {
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
        .ok_or_else(|| "Chat not found".to_string())?;
    let project = store
        .projects
        .iter()
        .find(|p| p.id == chat.project_id)
        .ok_or_else(|| "Project not found".to_string())?;

    let provider = chat
        .provider_id
        .clone()
        .and_then(|id| providers::get_provider(&id))
        .or_else(|| providers::get_provider(&providers::active_id()))
        .ok_or_else(|| "No provider configured".to_string())?;
    let launch = providers::resolve_launch(&provider)?;
    let project_path = project.path.clone();
    let stored_session = chat.grok_session_id.clone();
    let chat_id_for_task = chat_id.clone();
    let acp = acp.inner().clone();
    let acp_for_task = Arc::clone(&acp);

    let session_id = tauri::async_runtime::spawn_blocking(move || {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let acp = Arc::new(AcpManager::default());
    let acp_exit = acp.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(acp)
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
            list_providers,
            get_active_provider,
            set_active_provider,
            get_provider_status,
            test_provider
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(move |_app, event| {
            if let tauri::RunEvent::Exit = event {
                acp_exit.close_all();
            }
        });
}