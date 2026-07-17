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
    /// Model this chat runs on, passed to the agent via `-m` at spawn.
    /// None => the agent's own default model.
    #[serde(default)]
    pub model_id: Option<String>,
}

fn default_store_version() -> u32 {
    1
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppStore {
    /// Schema version. Absent in pre-1.0 files (serde default = 1). Bump + add a
    /// migration branch in `load()` when the shape changes.
    #[serde(default = "default_store_version")]
    pub version: u32,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub chats: Vec<Chat>,
}

impl Default for AppStore {
    fn default() -> Self {
        AppStore {
            version: default_store_version(),
            projects: Vec::new(),
            chats: Vec::new(),
        }
    }
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

        let Ok(raw) = fs::read_to_string(&path) else {
            return AppStore::default();
        };

        match serde_json::from_str(&raw) {
            Ok(store) => store,
            Err(err) => {
                // A corrupt/truncated file must NOT be silently replaced with an
                // empty store on the next save. Quarantine it so the data is
                // recoverable, and surface the reason.
                if let Some(dest) = crate::paths::quarantine_corrupt(&path, &Self::now()) {
                    eprintln!(
                        "data.json failed to parse ({err}); quarantined to {}",
                        dest.display()
                    );
                }
                AppStore::default()
            }
        }
    }

    pub fn save(store: &AppStore) -> Result<(), String> {
        let path = Self::data_path();
        let raw = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
        crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_json_predating_model_id_deserializes() {
        // Chats saved before per-chat models existed must load unchanged.
        let chat: Chat = serde_json::from_str(
            r#"{
                "id": "c1", "project_id": "p1", "title": "Old chat",
                "created_at": "1", "updated_at": "2", "messages": []
            }"#,
        )
        .unwrap();
        assert_eq!(chat.model_id, None);
        assert_eq!(chat.provider_id, None);
        assert_eq!(chat.grok_session_id, None);
    }
}