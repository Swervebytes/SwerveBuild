//! Swerve Build self-update (P3.1).
//!
//! The minisign private key never enters this crate. The public key and the
//! GitHub Releases endpoint live in `tauri.conf.json`. `prepare-release.ps1`
//! loads the owner's key from outside the repo only while it bundles.
//!
//! The webview calls the two commands below. The updater plugin's own IPC
//! stays off `capabilities/default.json`, so a script in the page cannot
//! call `download` / `install` except through these commands. There is no
//! MCP tool for this: an agent must not replace the running app.

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

const FEED_MISSING: &str = "No update feed is published yet. Swerve Build will check GitHub Releases again after the next release.";

#[derive(Debug, Clone, Serialize)]
pub struct AppUpdateReport {
    pub ok: bool,
    pub current_version: String,
    pub available: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub message: String,
}

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Honest copy for a failed check or install. A missing feed is not a crash.
pub fn explain_update_error(err: &tauri_plugin_updater::Error) -> String {
    match err {
        tauri_plugin_updater::Error::ReleaseNotFound => FEED_MISSING.to_string(),
        tauri_plugin_updater::Error::Network(detail) => {
            let lower = detail.to_ascii_lowercase();
            if lower.contains("404") || lower.contains("not found") {
                FEED_MISSING.to_string()
            } else {
                format!("Could not reach the update feed. {detail}")
            }
        }
        tauri_plugin_updater::Error::Minisign(_)
        | tauri_plugin_updater::Error::SignatureUtf8(_)
        | tauri_plugin_updater::Error::Base64(_) => {
            "The update signature did not verify. Nothing was installed.".to_string()
        }
        tauri_plugin_updater::Error::InsecureTransportProtocol => {
            "The update feed must use https. Nothing was installed.".to_string()
        }
        tauri_plugin_updater::Error::SignedVersionMismatch { signed, announced } => {
            format!(
                "The signed installer is {signed}, but the feed announced {announced}. Nothing was installed."
            )
        }
        other => format!("Could not check for updates. {other}"),
    }
}

fn report_from_update(
    current: String,
    version: String,
    notes: Option<String>,
) -> AppUpdateReport {
    let message = format!("Update available: {version}.");
    AppUpdateReport {
        ok: true,
        current_version: current,
        available: true,
        version: Some(version),
        notes,
        message,
    }
}

fn up_to_date(current: String) -> AppUpdateReport {
    AppUpdateReport {
        ok: true,
        current_version: current.clone(),
        available: false,
        version: None,
        notes: None,
        message: format!("Swerve Build {current} is up to date."),
    }
}

fn missing_feed(current: String) -> AppUpdateReport {
    AppUpdateReport {
        ok: true,
        current_version: current,
        available: false,
        version: None,
        notes: None,
        message: FEED_MISSING.to_string(),
    }
}

fn failed(current: String, message: String) -> AppUpdateReport {
    AppUpdateReport {
        ok: false,
        current_version: current,
        available: false,
        version: None,
        notes: None,
        message,
    }
}

fn short_notes(body: Option<String>) -> Option<String> {
    let text = body?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let mut chars = text.chars();
    let clipped: String = chars.by_ref().take(500).collect();
    if chars.next().is_some() {
        Some(format!("{clipped}…"))
    } else {
        Some(clipped)
    }
}

#[tauri::command]
pub async fn check_app_update(app: AppHandle) -> AppUpdateReport {
    let current = current_version(&app);
    let updater = match app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(updater) => updater,
        Err(error) => return failed(current, explain_update_error(&error)),
    };
    match updater.check().await {
        Ok(Some(update)) => {
            report_from_update(current, update.version.clone(), short_notes(update.body.clone()))
        }
        Ok(None) => up_to_date(current),
        Err(error) => {
            if matches!(
                error,
                tauri_plugin_updater::Error::ReleaseNotFound
            ) || explain_update_error(&error) == FEED_MISSING
            {
                missing_feed(current)
            } else {
                failed(current, explain_update_error(&error))
            }
        }
    }
}

#[tauri::command]
pub async fn install_app_update(app: AppHandle) -> Result<String, String> {
    let updater = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|error| explain_update_error(&error))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|error| explain_update_error(&error))?
    else {
        return Err("Swerve Build is already up to date.".into());
    };
    let version = update.version.clone();
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|error| explain_update_error(&error))?;
    // Windows exits the process during install. This line runs only if it did not.
    Ok(format!(
        "Swerve Build {version} is installed. Restart the app to finish."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn missing_feed_is_not_a_failure() {
        let message = explain_update_error(&tauri_plugin_updater::Error::ReleaseNotFound);
        assert!(message.contains("No update feed"));
        assert!(!message.to_ascii_lowercase().contains("panic"));
    }

    #[test]
    fn not_found_network_matches_the_missing_feed() {
        let message = explain_update_error(&tauri_plugin_updater::Error::Network(
            "404 Not Found".into(),
        ));
        assert_eq!(message, FEED_MISSING);
    }

    #[test]
    fn other_network_errors_stay_failures() {
        let message = explain_update_error(&tauri_plugin_updater::Error::Network(
            "connection refused".into(),
        ));
        assert!(message.starts_with("Could not reach the update feed."));
    }

    #[test]
    fn insecure_feed_is_refused() {
        let message = explain_update_error(&tauri_plugin_updater::Error::InsecureTransportProtocol);
        assert!(message.contains("https"));
    }

    #[test]
    fn config_pins_the_existing_public_key_and_github_feed() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let pubkey = conf["plugins"]["updater"]["pubkey"]
            .as_str()
            .expect("pubkey")
            .trim();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(pubkey)
            .expect("pubkey is base64");
        let text = String::from_utf8(raw).expect("pubkey text");
        assert!(text.contains("minisign public key"));
        let endpoints = conf["plugins"]["updater"]["endpoints"]
            .as_array()
            .expect("endpoints");
        assert_eq!(endpoints.len(), 1);
        let endpoint = endpoints[0].as_str().expect("endpoint");
        assert_eq!(
            endpoint,
            "https://github.com/Swervebytes/SwerveBuild/releases/latest/download/latest.json"
        );
        assert!(endpoint.starts_with("https://"));
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
        assert_eq!(
            conf["plugins"]["updater"]["windows"]["installMode"],
            "passive"
        );
    }
}
