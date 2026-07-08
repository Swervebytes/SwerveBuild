//! App-local data under `~/.swervebuild`. Migrates from legacy `~/.swervegrok` on first use.

use std::fs;
use std::path::PathBuf;
use std::sync::Once;

const DIR_NAME: &str = ".swervebuild";
const LEGACY_DIR_NAME: &str = ".swervegrok";

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn migrate_legacy_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let new_dir = home().join(DIR_NAME);
        let legacy = home().join(LEGACY_DIR_NAME);
        if new_dir.exists() || !legacy.is_dir() {
            return;
        }
        let _ = fs::rename(&legacy, &new_dir);
    });
}

/// Root directory for Swerve Build app data (projects, chats, providers, attachments).
pub fn data_dir() -> PathBuf {
    migrate_legacy_once();
    home().join(DIR_NAME)
}

pub fn data_file() -> PathBuf {
    data_dir().join("data.json")
}

pub fn providers_file() -> PathBuf {
    data_dir().join("providers.json")
}

pub fn attachments_dir() -> PathBuf {
    data_dir().join("attachments")
}