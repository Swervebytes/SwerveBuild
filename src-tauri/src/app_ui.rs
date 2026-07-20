//! App UI MCP surface (Roadmap Step 6) — grant + published UI state.
//!
//! Full CDP drive (click/type/screenshot of the live WebView) is a follow-up.
//! This module ships the safety gate and a read-only state channel the MCP
//! sidecar can consume without attaching to the process.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const GRANT_FILE: &str = "app_ui_grant.json";
const STATE_FILE: &str = "app_ui_state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUiGrant {
    pub granted: bool,
    /// ISO-ish timestamp string from the app (display / audit only).
    #[serde(default)]
    pub updated_at: String,
}

impl Default for AppUiGrant {
    fn default() -> Self {
        Self {
            granted: false,
            updated_at: String::new(),
        }
    }
}

/// Frontend-published snapshot of what the human currently sees.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppUiPublishedState {
    #[serde(default)]
    pub route: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub permission_modal_open: bool,
    #[serde(default)]
    pub updated_at: String,
}

fn grant_path() -> PathBuf {
    crate::paths::data_dir().join(GRANT_FILE)
}

fn state_path() -> PathBuf {
    crate::paths::data_dir().join(STATE_FILE)
}

fn read_json_file<T: for<'de> Deserialize<'de> + Default>(path: &PathBuf) -> T {
    if !path.is_file() {
        return T::default();
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn load_grant() -> AppUiGrant {
    read_json_file(&grant_path())
}

pub fn is_granted() -> bool {
    load_grant().granted
}

pub fn set_granted(granted: bool) -> Result<AppUiGrant, String> {
    let grant = AppUiGrant {
        granted,
        updated_at: crate::store::Store::now(),
    };
    let path = grant_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(&grant).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())?;
    Ok(grant)
}

pub fn load_published_state() -> AppUiPublishedState {
    read_json_file(&state_path())
}

pub fn publish_state(state: AppUiPublishedState) -> Result<AppUiPublishedState, String> {
    let mut state = state;
    if state.updated_at.is_empty() {
        state.updated_at = crate::store::Store::now();
    }
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())?;
    Ok(state)
}

/// Structured payload for MCP `app_ui_state` (and snapshot text).
pub fn state_report() -> serde_json::Value {
    let grant = load_grant();
    let published = load_published_state();
    serde_json::json!({
        "granted": grant.granted,
        "grantUpdatedAt": grant.updated_at,
        "driveReady": false,
        "driveNote": "CDP attach not shipped yet — read-only route/title only. Interaction tools return not_implemented until S08+.",
        "route": published.route,
        "title": published.title,
        "permissionModalOpen": published.permission_modal_open,
        "stateUpdatedAt": published.updated_at,
        "tools": {
            "app_ui_state": "available",
            "app_ui_snapshot": "available (text digest from published state)",
            "app_ui_screenshot": "not_implemented",
            "app_ui_click": "not_implemented",
            "app_ui_type": "not_implemented",
            "app_ui_press": "not_implemented",
            "app_ui_wait": "not_implemented",
        }
    })
}

pub fn require_grant() -> Result<(), String> {
    if is_granted() {
        Ok(())
    } else {
        Err(
            "app_ui not granted. Human must enable \"Allow agent to control SwerveBuild UI\" in Settings → Agent UI control."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that touch the real data dir paths via env override is hard;
    // unit-test pure defaults + require_grant against default (usually false).
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_grant_is_denied() {
        let _g = LOCK.lock().unwrap();
        // Do not assert filesystem — just the Default and JSON shape.
        let g = AppUiGrant::default();
        assert!(!g.granted);
        let raw = serde_json::to_string(&g).unwrap();
        assert!(raw.contains("\"granted\":false") || raw.contains("\"granted\": false"));
    }

    #[test]
    fn published_state_deserializes_partial() {
        let v: AppUiPublishedState =
            serde_json::from_str(r#"{"route":"/settings"}"#).expect("parse");
        assert_eq!(v.route, "/settings");
        assert!(v.title.is_empty());
        assert!(!v.permission_modal_open);
    }

    #[test]
    fn state_report_includes_drive_not_ready() {
        let _g = LOCK.lock().unwrap();
        let report = state_report();
        assert_eq!(report.get("driveReady").and_then(|v| v.as_bool()), Some(false));
        assert!(report.get("tools").is_some());
    }
}
