use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
    pub last_opened_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub images: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<ChatMessage>,
    /// The ACP session id for resume. Named `grok_session_id` for back-compat with
    /// existing data.json files; it is really a generic (provider-agnostic) id.
    #[serde(default)]
    pub grok_session_id: Option<String>,
    /// Which provider this chat is bound to. None => use the global active provider.
    #[serde(default)]
    pub provider_id: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppStore {
    pub projects: Vec<Project>,
    pub chats: Vec<Chat>,
}

pub struct Store;

impl Store {
    fn data_path() -> PathBuf {
        crate::paths::data_file()
    }

    pub fn load() -> AppStore {
        let path = Self::data_path();
        if !path.exists() {
            return AppStore::default();
        }

        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(store: &AppStore) -> Result<(), String> {
        let path = Self::data_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let raw = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
        fs::write(path, raw).map_err(|e| e.to_string())
    }

    pub fn now() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        secs.to_string()
    }

    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    pub fn project_name_from_path(path: &str) -> String {
        PathBuf::from(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Project")
            .to_string()
    }

    pub fn touch_project(store: &mut AppStore, project_id: &str) {
        let now = Self::now();
        if let Some(project) = store.projects.iter_mut().find(|p| p.id == project_id) {
            project.last_opened_at = now;
        }
    }

    pub fn chat_title_from_message(text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return "New chat".to_string();
        }

        let title: String = trimmed.chars().take(48).collect();
        if trimmed.chars().count() > 48 {
            format!("{title}…")
        } else {
            title
        }
    }
}