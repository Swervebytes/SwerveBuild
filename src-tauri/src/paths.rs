//! App-local data under `~/.swervebuild`. Migrates from legacy `~/.swervegrok` on first use.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Once;

const DIR_NAME: &str = ".swervebuild";
const LEGACY_DIR_NAME: &str = ".swervegrok";

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn chat_count(data_path: &Path) -> usize {
    let Ok(raw) = fs::read_to_string(data_path) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return 0;
    };
    value
        .get("chats")
        .and_then(|c| c.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}

fn merge_data_file(new_path: &Path, legacy_path: &Path) {
    if !legacy_path.is_file() {
        return;
    }

    let legacy_count = chat_count(legacy_path);
    let new_count = if new_path.is_file() {
        chat_count(new_path)
    } else {
        0
    };

    if legacy_count > new_count {
        if let Some(parent) = new_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::copy(legacy_path, new_path);
    }
}

fn migrate_legacy_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let new_dir = home().join(DIR_NAME);
        let legacy = home().join(LEGACY_DIR_NAME);

        if !legacy.is_dir() {
            return;
        }

        if !new_dir.exists() {
            let _ = fs::rename(&legacy, &new_dir);
            return;
        }

        // Both dirs exist (e.g. empty ~/.swervebuild created before rename) — keep richer data.
        merge_data_file(&new_dir.join("data.json"), &legacy.join("data.json"));

        let new_providers = new_dir.join("providers.json");
        let legacy_providers = legacy.join("providers.json");
        if !new_providers.is_file() && legacy_providers.is_file() {
            let _ = fs::copy(&legacy_providers, &new_providers);
        }
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

/// Automation definitions (triggered-agent orchestration). Separate from
/// `data.json` so the background scheduler never races chat writes.
pub fn automations_file() -> PathBuf {
    data_dir().join("automations.json")
}

/// One JSON file per workflow lives here (`<workflow_id>.json`).
pub fn workflows_dir() -> PathBuf {
    data_dir().join("workflows")
}

/// Per-workflow run records + event logs (`workflow-runs/<id>/<run>.json|.jsonl`).
pub fn workflow_runs_dir() -> PathBuf {
    data_dir().join("workflow-runs")
}

/// v1 secret store for workflow `$secret()` lookups (flat name→value JSON).
pub fn secrets_file() -> PathBuf {
    data_dir().join("secrets.json")
}

/// Per-automation run records + transcripts live under `runs/<automation_id>/`.
pub fn runs_dir() -> PathBuf {
    data_dir().join("runs")
}

pub fn run_dir(automation_id: &str) -> PathBuf {
    runs_dir().join(automation_id)
}

/// Write bytes atomically: write to a sibling `<path>.tmp` then rename over the
/// target. On Windows `fs::rename` maps to `MoveFileExW` with REPLACE_EXISTING,
/// so a crash mid-write can never leave a torn primary file. Creates parents.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}

/// Rename a file that failed to parse to `<name>.corrupt-<suffix>` instead of
/// silently overwriting it on the next save. Returns the quarantine path on
/// success so the caller can log it. Best-effort — never panics.
pub fn quarantine_corrupt(path: &Path, suffix: &str) -> Option<PathBuf> {
    let mut dest = path.as_os_str().to_owned();
    dest.push(format!(".corrupt-{suffix}"));
    let dest = PathBuf::from(dest);
    fs::rename(path, &dest).ok().map(|_| dest)
}