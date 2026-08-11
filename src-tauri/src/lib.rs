mod acp;
pub mod app_ui;
mod app_ui_cdp;
mod artifacts;
pub mod browser_debug;
mod chat_media;
mod db;
mod env_context;
mod grok_config;
mod jobs;
pub mod live;
mod local_llm;
pub mod local_image;
pub mod media_worker;
mod media_providers;
mod model_catalog;
pub mod paths;
mod providers;
pub mod provider_auth;
pub mod secrets;
mod store;
pub mod terminal;
mod util;
pub mod workflows_tauri;

use acp::AcpManager;
use jobs::JobManager;
use providers::{ProviderStatus, ProviderView};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use store::{AppStore, Chat, ChatMessage, MessagePart, Project, Store};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

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

    prefer_executable(&String::from_utf8_lossy(&output.stdout))
}

/// Pick the spawnable path out of `where` output.
///
/// S37: npm installs BOTH an extensionless Unix shell script and a Windows shim
/// (`.cmd`), and `where` lists the script FIRST. Spawning that script fails with
/// "%1 is not a valid Win32 application" (os error 193) — exactly how the
/// freshly-installed claude-code-acp / gemini providers died while the UI still
/// said "Available". So on Windows, prefer a genuinely executable extension.
///
/// Falls back to the first non-empty line, so a command with no recognised
/// extension still resolves rather than vanishing.
///
/// S38b: this is now the **only** definition of that rule. `which_on_path` used
/// to carry an inlined duplicate of it, which meant the S37 regression test was
/// exercising this copy while production ran the other one — editing the inlined
/// branch would have left the test green and reintroduced os error 193. Keep the
/// delegation; do not re-inline.
pub(crate) fn prefer_executable(lines: &str) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = lines
        .lines()
        .map(|l| PathBuf::from(l.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .collect();

    #[cfg(windows)]
    {
        if let Some(exe) = candidates.iter().find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| matches!(e.to_ascii_lowercase().as_str(), "exe" | "cmd" | "bat" | "com"))
                .unwrap_or(false)
        }) {
            return Some(exe.clone());
        }
    }

    candidates.into_iter().next()
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

// ---------------------------------------------------------------------------
// Pinned Grok Build CLI install (A5)
//
// Never pipe a remote script to iex. Download the Windows binary at a fixed
// version, SHA-256 verify, then place it under ~/.grok/bin. Bump all four
// constants together via the DEPENDENCIES.md upgrade ritual.
// Recorded 2026-07-20 from https://x.ai/cli/stable → 0.2.106.
// ---------------------------------------------------------------------------
/// Pinned Grok CLI release tag (semver without `v`).
pub const GROK_CLI_VERSION: &str = "0.2.106";
const GROK_CLI_URL: &str = "https://x.ai/cli/grok-0.2.106-windows-x86_64.exe";
/// Fallback when the Cloudflare-fronted x.ai host is unreachable (same artifact
/// layout as the official install.ps1).
const GROK_CLI_URL_FALLBACK: &str =
    "https://storage.googleapis.com/grok-build-public-artifacts/cli/grok-0.2.106-windows-x86_64.exe";
const GROK_CLI_SHA256: &str = "A6A25D55DAADCA0C2458A5ACEB4C1873EB7C76964EF307647D079E344C53969A";
const GROK_CLI_SIZE: u64 = 130_120_520;

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
async fn install_grok() -> CommandResult {
    // Download + hash can take minutes; never block the UI thread.
    tauri::async_runtime::spawn_blocking(install_grok_pinned)
        .await
        .unwrap_or_else(|e| CommandResult {
            success: false,
            message: format!("Install task failed: {e}"),
        })
}

/// Download the pinned Grok CLI binary, verify SHA-256, install to `~/.grok/bin`.
/// Blocking — call only from a worker thread (`spawn_blocking`).
fn install_grok_pinned() -> CommandResult {
    match install_grok_pinned_inner() {
        Ok(message) => CommandResult {
            success: true,
            message,
        },
        Err(message) => CommandResult {
            success: false,
            message,
        },
    }
}

fn install_grok_pinned_inner() -> Result<String, String> {
    // Already on the pin? Skip the ~124 MB download.
    if let Some(path) = resolve_grok_executable() {
        if let Some(ver) = grok_version_at(&path) {
            if version_matches_pin(&ver, GROK_CLI_VERSION) {
                return Ok(format!(
                    "Grok Build v{GROK_CLI_VERSION} is already installed at {}.",
                    path.display()
                ));
            }
        }
    }

    let home = grok_home();
    let download_dir = home.join("downloads");
    let bin_dir = home.join("bin");
    fs::create_dir_all(&download_dir).map_err(|e| format!("create downloads dir: {e}"))?;
    fs::create_dir_all(&bin_dir).map_err(|e| format!("create bin dir: {e}"))?;

    let staged = download_dir.join(format!("grok-{GROK_CLI_VERSION}-windows-x86_64.exe"));
    // Fresh download each attempt so a partial/corrupt file can't pass if size
    // happens to match later (checksum is the real gate either way).
    let _ = fs::remove_file(&staged);

    download_with_curl(GROK_CLI_URL, &staged)
        .or_else(|primary_err| {
            download_with_curl(GROK_CLI_URL_FALLBACK, &staged).map_err(|fallback_err| {
                format!(
                    "download failed (primary: {primary_err}; fallback: {fallback_err})"
                )
            })
        })?;

    let hash = file_sha256(&staged)?;
    if !hash.eq_ignore_ascii_case(GROK_CLI_SHA256) {
        let _ = fs::remove_file(&staged);
        return Err(format!(
            "Grok CLI checksum mismatch (got {hash}, expected {GROK_CLI_SHA256}); download removed — try again or re-pin via DEPENDENCIES ritual"
        ));
    }

    // Official installer places both names; keep parity so either works.
    for name in ["grok.exe", "agent.exe"] {
        install_binary_locked(&staged, &bin_dir.join(name))?;
    }

    // Best-effort User PATH so shells outside SwerveBuild find the binary.
    let _ = ensure_user_path_has(&bin_dir);

    let installed = bin_dir.join("grok.exe");
    let reported = grok_version_at(&installed).unwrap_or_else(|| GROK_CLI_VERSION.to_string());
    Ok(format!(
        "Grok Build {reported} installed to {} (pinned v{GROK_CLI_VERSION}).",
        installed.display()
    ))
}

fn version_matches_pin(reported: &str, pin: &str) -> bool {
    // `grok --version` may print "grok 0.2.106", "v0.2.106", or just "0.2.106".
    // Tokenize so "0.2.1060" does not match pin "0.2.106".
    reported
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|tok| tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-'))
        .map(|tok| tok.trim_start_matches('v'))
        .any(|tok| tok == pin)
}

fn download_with_curl(url: &str, dest: &Path) -> Result<(), String> {
    let status = util::hidden_command("curl.exe")
        .args([
            "-L",
            "--fail",
            "--retry",
            "3",
            "-o",
            &dest.display().to_string(),
            url,
        ])
        .status()
        .map_err(|e| format!("could not run curl.exe: {e}"))?;
    if !status.success() {
        let _ = fs::remove_file(dest);
        return Err(format!("curl exit {:?}", status.code()));
    }
    let size = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = fs::remove_file(dest);
        return Err("download was empty".into());
    }
    // Soft size check — warn via error if wildly wrong (checksum still authoritative).
    if size != GROK_CLI_SIZE && !(size > GROK_CLI_SIZE / 2 && size < GROK_CLI_SIZE * 2) {
        // still allow if hash matches later; only reject absurd sizes
        if size < 1_000_000 {
            let _ = fs::remove_file(dest);
            return Err(format!("download too small ({size} bytes)"));
        }
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let output = util::hidden_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash",
                path.display().to_string().replace('\'', "''")
            ),
        ])
        .output()
        .map_err(|e| format!("hash: {e}"))?;
    if !output.status.success() {
        return Err("hashing the download failed".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Copy `src` onto `dest`, renaming a locked existing file to `.old` when needed
/// (mirrors official install.ps1 locked-file handling).
fn install_binary_locked(src: &Path, dest: &Path) -> Result<(), String> {
    let old = dest.with_extension("exe.old");
    let _ = fs::remove_file(&old);
    match fs::copy(src, dest) {
        Ok(_) => Ok(()),
        Err(_) => {
            if dest.exists() {
                let _ = fs::rename(dest, &old);
            }
            fs::copy(src, dest).map_err(|e| format!("install {}: {e}", dest.display()))?;
            Ok(())
        }
    }
}

/// Append `dir` to the user's PATH if missing. Best-effort — install still
/// succeeds if PATH update fails (we resolve `~/.grok/bin` directly).
fn ensure_user_path_has(dir: &Path) -> Result<(), String> {
    let dir_s = dir.display().to_string();
    let output = util::hidden_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "$d = '{}'; $p = [Environment]::GetEnvironmentVariable('Path','User'); if (-not $p) {{ $p = '' }}; $parts = $p -split ';' | Where-Object {{ $_ -ne '' }}; if ($parts -notcontains $d) {{ $new = (@($d) + $parts) -join ';'; [Environment]::SetEnvironmentVariable('Path', $new, 'User') }}",
                dir_s.replace('\'', "''")
            ),
        ])
        .status()
        .map_err(|e| format!("PATH update: {e}"))?;
    if !output.success() {
        return Err("PATH update failed".into());
    }
    Ok(())
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
    videos: Option<Vec<String>>,
    parts: Option<Vec<MessagePart>>,
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
        videos: videos.unwrap_or_default(),
        parts: parts.unwrap_or_default(),
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
    let path = acp::save_image_base64(&data_url)?;
    artifacts::maybe_enforce_after_write();
    Ok(path)
}

/// S23: artifact budget / per-kind usage (attachments + UI captures).
#[tauri::command]
fn artifact_store_status() -> artifacts::ArtifactStoreStatus {
    artifacts::status()
}

/// S23: resync on-disk files into SQLite artifact registry.
#[tauri::command]
fn artifact_store_resync() -> Result<u32, String> {
    db::init()?;
    artifacts::resync_registry()
}

/// S23: prune oldest managed artifacts until under budget (`dry_run` = report only).
#[tauri::command]
fn artifact_store_prune(dry_run: Option<bool>) -> Result<artifacts::PruneResult, String> {
    let _ = db::init();
    artifacts::prune(dry_run.unwrap_or(true))
}

/// S24: media worker supervisor status (separate process shell).
#[tauri::command]
fn media_worker_status() -> media_worker::SupervisorStatus {
    media_worker::status()
}

#[tauri::command]
fn media_worker_start() -> Result<media_worker::SupervisorStatus, String> {
    media_worker::ensure_running()
}

#[tauri::command]
fn media_worker_stop() -> Result<media_worker::SupervisorStatus, String> {
    media_worker::stop()
}

/// S25: primary-display still PNG via media worker → attachments dir.
#[tauri::command]
fn media_worker_capture_still() -> Result<media_worker::CaptureStillResult, String> {
    media_worker::capture_still()
}

/// S26/S28: short MJPEG clip (from still or new capture) via FFmpeg in worker.
/// Audio: auto dshow when a device is available; silent fallback.
#[tauri::command]
fn media_worker_encode_clip(still_path: Option<String>) -> Result<media_worker::EncodeClipResult, String> {
    media_worker::encode_clip(still_path)
}

/// S27: pinned LGPL FFmpeg status (tag, path, resolve source).
#[tauri::command]
fn media_worker_ffmpeg_status() -> media_worker::FfmpegStatus {
    media_worker::ffmpeg_status()
}

/// S27: download + verify + unpack pinned LGPL FFmpeg if missing (blocking task).
#[tauri::command]
async fn media_worker_ensure_ffmpeg() -> Result<media_worker::FfmpegStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        media_worker::ensure_ffmpeg()?;
        Ok(media_worker::ffmpeg_status())
    })
    .await
    .map_err(|e| format!("ffmpeg ensure task failed: {e}"))?
}

/// `refresh: true` forces a fresh Comfy probe (Probe button). Default uses cache.
/// `prefs_only: true` skips network entirely (header summary on chat paint).
/// Runs on a worker thread so Comfy timeouts never freeze the WebView.
#[tauri::command]
async fn list_media_providers(
    refresh: Option<bool>,
    prefs_only: Option<bool>,
) -> media_providers::MediaProvidersView {
    let refresh = refresh.unwrap_or(false);
    let prefs_only = prefs_only.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        if prefs_only {
            media_providers::view_prefs_only()
        } else if refresh {
            media_providers::view_refresh()
        } else {
            media_providers::view()
        }
    })
    .await
    .unwrap_or_else(|_| media_providers::view_prefs_only())
}

#[tauri::command]
fn set_image_provider(id: String) -> Result<media_providers::MediaProvidersView, String> {
    media_providers::set_image_provider(&id)
}

#[tauri::command]
fn set_video_provider(id: String) -> Result<media_providers::MediaProvidersView, String> {
    media_providers::set_video_provider(&id)
}

#[tauri::command]
fn set_comfy_base_url(url: String) -> Result<media_providers::MediaProvidersView, String> {
    media_providers::set_comfy_base_url(&url)
}

#[tauri::command]
async fn probe_local_image() -> local_image::LocalImageStatus {
    tauri::async_runtime::spawn_blocking(local_image::probe_fresh)
        .await
        .unwrap_or_else(|_| local_image::LocalImageStatus {
            reachable: false,
            base_url: local_image::DEFAULT_COMFY_URL.into(),
            note: "Probe worker failed".into(),
            checkpoints: vec![],
        })
}

/// Generate via ComfyUI when local provider is selected / available. Returns
/// attachment path under ~/.swervebuild/attachments.
#[tauri::command]
async fn generate_local_image(
    prompt: String,
    negative: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        local_image::generate(
            &prompt,
            negative.as_deref(),
            width.unwrap_or(512),
            height.unwrap_or(512),
        )
    })
    .await
    .map_err(|e| format!("generate task failed: {e}"))?
}

/// Copy picked image files into `~/.swervebuild/attachments` so the webview
/// asset protocol can render them (S15). Non-images / oversize files are skipped.
#[tauri::command]
fn import_attachment_files(paths: Vec<String>) -> Result<Vec<String>, String> {
    let out = acp::import_attachment_files(&paths)?;
    artifacts::maybe_enforce_after_write();
    Ok(out)
}

/// Scan an agent turn's text for image/video artifact paths (chat_media.rs),
/// verify them on disk (relative → the chat's project folder), copy survivors
/// into the attachments dir, and return the stored paths for persistence.
#[tauri::command]
fn detect_chat_media(chat_id: String, text: String) -> Result<serde_json::Value, String> {
    let store = Store::load();
    let chat = store
        .chats
        .iter()
        .find(|c| c.id == chat_id)
        .ok_or_else(|| "Chat not found".to_string())?;
    let cwd = store
        .projects
        .iter()
        .find(|p| p.id == chat.project_id)
        .map(|p| std::path::PathBuf::from(&p.path))
        .filter(|p| p.is_dir())
        // No/gone project folder: relative candidates simply won't resolve;
        // absolute paths still verify.
        .unwrap_or_else(std::env::temp_dir);
    let (images, videos) = chat_media::detect_for_turn(&text, &cwd);
    Ok(serde_json::json!({ "images": images, "videos": videos }))
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
    let provider_id = provider.id.clone();
    let model_id = chat.model_id.clone();
    let acp = acp.inner().clone();
    let acp_for_task = Arc::clone(&acp);
    // Local models need the app's llama-server up before grok spawns. Done in
    // the blocking task — first load of a big GGUF can take minutes.
    let local_model = chat
        .model_id
        .clone()
        .filter(|m| provider.id == "grok" && m.starts_with(grok_config::LOCAL_PREFIX));
    let running_automations = app
        .try_state::<Arc<JobManager>>()
        .map(|jm| jm.running_count())
        .unwrap_or(0);

    let session_id = tauri::async_runtime::spawn_blocking(move || {
        if let Some(model) = local_model.as_deref() {
            // Lease this chat on the local model so automations / other chats
            // cannot swap VRAM out from under a live session.
            local_llm::manager().acquire(
                &app,
                &local_llm::chat_holder(&chat_id_for_task),
                model,
            )?;
        }
        acp_for_task.ensure_session(
            app,
            &launch,
            &project_path,
            &chat_id_for_task,
            stored_session.as_deref(),
            &provider_id,
            model_id.as_deref(),
            running_automations,
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
        local_llm::manager().release(&local_llm::chat_holder(&id));
    } else {
        acp.close_all();
        local_llm::manager().release_prefix("chat:");
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
            // P1.2: existence check spans keystore + not-yet-migrated plaintext.
            has_api_key: providers::endpoint_api_key().is_some(),
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

/// May shell out to `grok models` — never on the UI thread.
#[tauri::command]
async fn list_models() -> Vec<providers::ModelInfo> {
    tauri::async_runtime::spawn_blocking(providers::list_models)
        .await
        .unwrap_or_default()
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
    /// S17 resource plane — honest nulls when nvidia-smi is missing.
    vram_total_mb: Option<u32>,
    vram_used_mb: Option<u32>,
    /// `"nvidia-smi"` or `"unavailable"`.
    vram_source: String,
    /// Catalog estimate for the loaded model, if known.
    model_est_vram_gb: Option<f32>,
    parallel_slots: u32,
    ctx_tokens: u32,
}

impl LocalState {
    fn current() -> Self {
        let server = local_llm::manager().status();
        let usage = model_catalog::detect_vram_usage();
        let (vram_used_mb, vram_total_mb, vram_source) = match usage {
            Some(u) => (Some(u.used_mb), Some(u.total_mb), "nvidia-smi".to_string()),
            None => (None, None, "unavailable".to_string()),
        };
        let model_est_vram_gb = server
            .model_id
            .as_deref()
            .and_then(model_catalog::estimate_vram_gb_for_model);
        LocalState {
            engine_installed: local_llm::engine_installed(),
            engine_version: local_llm::ENGINE_TAG.to_string(),
            server,
            models: providers::ProviderStore::load().local.models,
            vram_total_mb,
            vram_used_mb,
            vram_source,
            model_est_vram_gb,
            parallel_slots: local_llm::PARALLEL_SLOTS,
            ctx_tokens: local_llm::CTX_TOKENS,
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
    // If the server is currently serving this model, stop only when idle.
    let status = local_llm::manager().status();
    if status.model_id.as_deref() == Some(id.as_str()) {
        local_llm::manager().stop_if_idle(&app)?;
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
fn stop_local_server(app: tauri::AppHandle) -> Result<LocalState, String> {
    local_llm::manager().stop_if_idle(&app)?;
    Ok(LocalState::current())
}

// -------------------------------------------------------- model catalog (P3)

#[tauri::command]
fn get_model_catalog() -> model_catalog::CatalogState {
    model_catalog::catalog_state()
}

#[tauri::command]
fn set_models_dir(path: String) -> Result<model_catalog::CatalogState, String> {
    model_catalog::set_models_dir(path)?;
    Ok(model_catalog::catalog_state())
}

/// Download a curated catalog GGUF (resumable) and register it for pickers.
#[tauri::command]
async fn download_catalog_model(
    app: tauri::AppHandle,
    catalog_id: String,
) -> Result<CommandResult, String> {
    let message = tauri::async_runtime::spawn_blocking(move || {
        model_catalog::download_catalog_model(&app, &catalog_id)
    })
    .await
    .map_err(|e| format!("download task failed: {e}"))??;
    Ok(CommandResult {
        success: true,
        message,
    })
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

#[tauri::command]
fn get_app_ui_grant() -> app_ui::AppUiGrant {
    app_ui::load_grant()
}

#[tauri::command]
fn set_app_ui_grant(granted: bool) -> Result<app_ui::AppUiGrant, String> {
    app_ui::set_granted(granted)
}

#[tauri::command]
fn get_term_grant() -> terminal::TermGrant {
    terminal::load_grant()
}

#[tauri::command]
fn set_term_grant(granted: bool) -> Result<terminal::TermGrant, String> {
    terminal::set_granted(granted)
}

// ---- provider CLI install (S37) ----------------------------------------------

/// Is npm on PATH, and where? `npm` is a `.cmd` shim on Windows, so it must be
/// invoked through the shell rather than spawned directly.
fn npm_version() -> Option<String> {
    let out = if cfg!(windows) {
        crate::util::hidden_command("cmd")
            .args(["/C", "npm --version"])
            .output()
            .ok()?
    } else {
        crate::util::hidden_command("npm").arg("--version").output().ok()?
    };
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// What the Providers UI needs to render install controls honestly.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInstallInfo {
    /// False for Grok (self-managed) and the HTTP rows.
    installable: bool,
    package: Option<String>,
    version: Option<String>,
    docs: Option<String>,
    /// Shown verbatim so what the user copies is what we would have run.
    install_command: Option<String>,
    uninstall_command: Option<String>,
    /// None when npm is missing — the UI disables the button and explains.
    npm_version: Option<String>,
    installed: bool,
}

#[tauri::command]
fn provider_install_info(id: String) -> ProviderInstallInfo {
    let cli = providers::installable_for(&id);
    let installed = providers::provider_status(&id).installed;
    match cli {
        Some(c) => ProviderInstallInfo {
            installable: true,
            package: Some(c.package.to_string()),
            version: Some(c.version.to_string()),
            docs: Some(c.docs.to_string()),
            install_command: Some(c.install_command()),
            uninstall_command: Some(c.uninstall_command()),
            npm_version: npm_version(),
            installed,
        },
        None => ProviderInstallInfo {
            installable: false,
            package: None,
            version: None,
            docs: None,
            install_command: None,
            uninstall_command: None,
            npm_version: npm_version(),
            installed,
        },
    }
}

fn run_npm(command: &str) -> Result<String, String> {
    let out = if cfg!(windows) {
        crate::util::hidden_command("cmd")
            .args(["/C", command])
            .output()
            .map_err(|e| format!("spawn npm: {e}"))?
    } else {
        crate::util::hidden_command("sh")
            .args(["-c", command])
            .output()
            .map_err(|e| format!("spawn npm: {e}"))?
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else {
        // npm puts the useful part on stderr.
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

/// Install a provider's CLI at the pinned version.
#[tauri::command]
async fn install_provider_cli(id: String) -> CommandResult {
    let Some(cli) = providers::installable_for(&id) else {
        return CommandResult {
            success: false,
            message: format!("{id} has no managed install."),
        };
    };
    if npm_version().is_none() {
        return CommandResult {
            success: false,
            message: format!(
                "npm not found on PATH. Install Node.js, then run:\n{}",
                cli.install_command()
            ),
        };
    }
    let cmd = cli.install_command();
    match tauri::async_runtime::spawn_blocking(move || run_npm(&cmd)).await {
        Ok(Ok(_)) => {
            let ok = providers::provider_status(&id).installed;
            CommandResult {
                success: ok,
                message: if ok {
                    format!("{}@{} installed.", cli.package, cli.version)
                } else {
                    // npm succeeded but the binary is not resolvable — usually a
                    // PATH that the app process has not picked up yet.
                    format!(
                        "{}@{} installed, but its command is not on this app's PATH yet — restart Swerve Build.",
                        cli.package, cli.version
                    )
                },
            }
        }
        Ok(Err(e)) => CommandResult {
            success: false,
            message: format!("npm install failed: {e}"),
        },
        Err(e) => CommandResult {
            success: false,
            message: format!("install task failed: {e}"),
        },
    }
}

/// Remove a provider's CLI. The UI confirms (naming the exact command) first.
#[tauri::command]
async fn uninstall_provider_cli(id: String) -> CommandResult {
    let Some(cli) = providers::installable_for(&id) else {
        return CommandResult {
            success: false,
            message: format!("{id} has no managed install."),
        };
    };
    if npm_version().is_none() {
        return CommandResult {
            success: false,
            message: format!(
                "npm not found on PATH. To remove it by hand:\n{}",
                cli.uninstall_command()
            ),
        };
    }
    let cmd = cli.uninstall_command();
    match tauri::async_runtime::spawn_blocking(move || run_npm(&cmd)).await {
        Ok(Ok(_)) => CommandResult {
            success: true,
            message: format!("{} removed.", cli.package),
        },
        Ok(Err(e)) => CommandResult {
            success: false,
            message: format!("npm uninstall failed: {e}"),
        },
        Err(e) => CommandResult {
            success: false,
            message: format!("uninstall task failed: {e}"),
        },
    }
}

// ---- secrets (S36): write-and-forget only ------------------------------------
//
// There is deliberately NO `secret_get` command. Every registered command is
// callable by any JS running in the main webview, so exposing a read would let
// a single injected script exfiltrate a stream key — a live-broadcast takeover,
// which `design/live-safety.md` treats as irreversible. Rust-side consumers
// (the future RTMP sink) call `secrets::get` directly.

/// Store a secret in the OS keystore (Windows Credential Manager).
#[tauri::command]
fn secret_set(name: String, value: String) -> Result<secrets::SecretStatus, String> {
    secrets::set(&name, &value)?;
    secrets::status(&name)
}

/// Does a secret exist? Never returns the value.
#[tauri::command]
fn secret_status(name: String) -> Result<secrets::SecretStatus, String> {
    secrets::status(&name)
}

/// Remove a secret. Idempotent, so a "clear" button always succeeds.
#[tauri::command]
fn secret_delete(name: String) -> Result<secrets::SecretStatus, String> {
    secrets::delete(&name)?;
    secrets::status(&name)
}

/// Namespaced name for a stream destination's key, e.g. `stream.twitch`.
#[tauri::command]
fn stream_key_name(target: String) -> Result<String, String> {
    secrets::stream_key_name(&target)
}

// ---- provider sign-in (P1.1 / S-AUTH) ----------------------------------------
//
// Human-only surface: reachable from Settings, never exposed as an MCP tool.
// Both are blocking (they spawn and drive an agent process), so they run on a
// worker thread like the install commands above.

#[tauri::command]
async fn provider_auth_probe(provider_id: String) -> Result<provider_auth::AuthProbe, String> {
    tauri::async_runtime::spawn_blocking(move || provider_auth::probe(&provider_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn provider_sign_in(
    provider_id: String,
    method_id: String,
) -> Result<provider_auth::SignInOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || provider_auth::sign_in(&provider_id, &method_id))
        .await
        .map_err(|e| e.to_string())?
}

// ---- go-live approval tier (S38) ---------------------------------------------
//
// Every command here passes `Caller::Human` because the webview *is* the human
// surface. That is a claim, not proof — S21b showed injected JS can invoke any
// command — so `live.rs` backs it with a physical-presence check the webview
// cannot fake. Deliberately absent: any command that starts a stream in one
// shot, and any MCP tool at all (see `bin/swervebuild_mcp.rs`).

#[tauri::command]
fn live_status() -> live::LiveStatus {
    live::status()
}

#[tauri::command]
fn get_live_grant() -> live::LiveGrant {
    live::load_grant()
}

#[tauri::command]
fn set_live_grant(granted: bool) -> Result<live::LiveGrant, String> {
    live::set_granted(live::Caller::Human, granted)
}

/// Step 1 of arming: returns a single-use nonce the UI echoes back on confirm.
#[tauri::command]
fn live_request_arm() -> Result<String, String> {
    live::request_arm(live::Caller::Human)
}

/// Step 2: confirm while physically holding the chord.
#[tauri::command]
fn live_confirm_arm(nonce: String) -> Result<live::LiveStatus, String> {
    live::confirm_arm(live::Caller::Human, &nonce)
}

/// Step 3: on air. Re-runs the whole gate; arming is a second factor, not a
/// bypass. The RTMP sink (S39) starts only after this returns `Ok`.
#[tauri::command]
fn live_go_live() -> Result<live::LiveStatus, String> {
    live::go_live(live::Caller::Human)
}

/// Always allowed, cannot fail — no grant, no arming, no presence proof.
#[tauri::command]
fn live_stop(reason: Option<String>) -> live::LiveStatus {
    live::stop(
        live::Caller::Human,
        reason.as_deref().unwrap_or("stopped from the UI"),
    )
}

/// Panic button. Same fail-safe rules as `live_stop`.
#[tauri::command]
fn live_panic_cut() -> live::LiveStatus {
    live::panic_cut(live::Caller::Human)
}

#[tauri::command]
fn live_clear_privacy_cut() -> Result<live::LiveStatus, String> {
    live::clear_privacy_cut(live::Caller::Human)
}

/// Per-stream opt-in for agent-driven scene changes (rules 2 and 3).
#[tauri::command]
fn live_set_agent_scene_control(allowed: bool) -> Result<live::LiveStatus, String> {
    live::set_agent_scene_control(live::Caller::Human, allowed)
}

#[tauri::command]
fn live_recent_events(limit: Option<usize>) -> Vec<serde_json::Value> {
    live::recent_events(limit.unwrap_or(50))
}

#[tauri::command]
fn get_browser_debug_grant() -> browser_debug::BrowserDebugGrant {
    browser_debug::load_grant()
}

#[tauri::command]
fn set_browser_debug_grant(granted: bool) -> Result<browser_debug::BrowserDebugGrant, String> {
    browser_debug::set_granted(granted)
}

/// S13e: toggle whether the browser may open PUBLIC (non-loopback) URLs. Off by
/// default; the SSRF guard (private/link-local/metadata) holds even when on.
#[tauri::command]
fn set_browser_public(allow: bool) -> Result<browser_debug::BrowserDebugGrant, String> {
    browser_debug::set_allow_public(allow)
}

/// Offscreen Y (logical px) where the docked debug pane is parked when the
/// human dock is closed. Far below any real window height so the child webview
/// is clipped out of view, yet still alive — the agent's browser_* tools and
/// CDP screenshots keep working headlessly.
const PANE_PARK_Y: f64 = 30_000.0;

/// The docked debug-pane child webview (label `swerve-debug`) living inside the
/// main window. `None` until `add_child` has run at startup.
fn debug_pane_webview(app: &AppHandle) -> Option<tauri::Webview<tauri::Wry>> {
    app.get_webview(browser_debug::DEBUG_PANE_LABEL)
}

/// Position + size the docked pane over the frontend's reserved area. Bounds
/// are **physical** pixels (CSS px × devicePixelRatio) relative to the window
/// client area: a child webview is positioned in device pixels, so the frontend
/// pre-multiplies its `getBoundingClientRect` by the current DPR (this window
/// runs at fractional scale, e.g. 1.38). Called on dock open and on resize.
#[tauri::command]
fn browser_pane_set_bounds(
    app: AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let pane = debug_pane_webview(&app).ok_or("debug pane missing — restart SwerveBuild")?;
    pane.set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    pane.set_size(tauri::PhysicalSize::new(width.max(1.0), height.max(1.0)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Park the docked pane offscreen — dock closed, or temporarily hidden while a
/// modal is up (a native child webview always paints above the HTML). The
/// webview stays alive so agent tools keep working.
#[tauri::command]
fn browser_pane_park(app: AppHandle) -> Result<(), String> {
    let pane = debug_pane_webview(&app).ok_or("debug pane missing — restart SwerveBuild")?;
    pane.set_position(tauri::PhysicalPosition::new(0.0, PANE_PARK_Y))
        .map_err(|e| e.to_string())
}

/// Human toolbar: navigate the docked pane to a LOCAL url. Loopback-only (same
/// policy as the agent tool) but no agent grant — the human is driving.
#[tauri::command]
fn browser_pane_open(url: String) -> Result<serde_json::Value, String> {
    browser_debug::human_open(&url)
}

/// Human toolbar: back | forward | reload in the docked pane.
#[tauri::command]
fn browser_pane_nav(action: String) -> Result<serde_json::Value, String> {
    browser_debug::human_navigate(&action)
}

/// Frontend publishes the visible route/title so MCP `app_ui_state` can report it
/// without CDP. Does not require the human grant (publish is local telemetry).
#[tauri::command]
fn publish_app_ui_state(state: app_ui::AppUiPublishedState) -> Result<app_ui::AppUiPublishedState, String> {
    app_ui::publish_state(state)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebView2 remote debugging only when allowed (S21b): not unconditional in
    // release. Must run before any webview is created.
    match app_ui::prepare_webview_cdp() {
        Ok(0) => { /* CDP off — expected in release without grant / opt-in */ }
        Ok(port) => eprintln!("[swervebuild] CDP remote-debugging on 127.0.0.1:{port}"),
        Err(e) => eprintln!("[swervebuild] prepare_webview_cdp: {e}"),
    }

    let acp = Arc::new(AcpManager::default());
    let acp_exit = acp.clone();
    let job_mgr = Arc::new(JobManager::default());
    let jobs_exit = job_mgr.clone();
    let jobs_sched = job_mgr.clone();
    let wf_mgr = Arc::new(workflows_tauri::WorkflowManager::default());
    let wf_exit = wf_mgr.clone();
    let wf_sched = wf_mgr.clone();
    // Persistent terminal sessions live in this (app) process; the sidecar
    // proxies to them over a loopback control server started in setup().
    let term_mgr = Arc::new(terminal::SessionManager::default());
    let term_exit = term_mgr.clone();
    let term_serve = term_mgr.clone();

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
        .manage(wf_mgr)
        .manage(term_mgr)
        .setup(move |app| {
            // S23: SQLite skeleton + optional artifact registry (non-fatal).
            db::init_best_effort();
            jobs::spawn_scheduler(app.handle().clone(), jobs_sched);
            workflows_tauri::spawn_scheduler(app.handle().clone(), wf_sched);
            if let Err(e) = terminal::serve(term_serve) {
                eprintln!("[swervebuild] terminal control server: {e}");
            }
            // S13d debug pane: a second webview docked as a CHILD of the main
            // window (was a separate hidden window through S13c) so it is visible
            // IN the app — a Claude-style browser pane. Still the same marker URL
            // and label, so the CDP supervisor + grant-gated browser_* MCP tools
            // find and drive it exactly as before. It is created parked offscreen
            // (see PANE_PARK_Y): alive and agent-drivable at all times, brought
            // on-screen only when the human opens the dock (browser_pane_*).
            //
            // Created on a background thread: Window::add_child dispatches the
            // build to the main thread and BLOCKS until it runs — calling it
            // inline here (also the main thread, before the event loop pumps)
            // would deadlock.
            let pane_app = app.handle().clone();
            std::thread::spawn(move || {
                let url = match browser_debug::DEBUG_PANE_INITIAL_URL.parse::<tauri::Url>() {
                    Ok(u) => u,
                    Err(e) => {
                        eprintln!("[swervebuild] debug pane url: {e}");
                        return;
                    }
                };
                let Some(window) = pane_app.get_window("main") else {
                    eprintln!("[swervebuild] debug pane: main window missing");
                    return;
                };
                let builder = tauri::webview::WebviewBuilder::new(
                    browser_debug::DEBUG_PANE_LABEL,
                    tauri::WebviewUrl::External(url),
                );
                // Born offscreen at a real size so it renders (CDP screenshots
                // work) but is clipped out of view until the dock opens. Physical
                // px (see browser_pane_set_bounds): PANE_PARK_Y is far offscreen.
                if let Err(e) = window.add_child(
                    builder,
                    tauri::PhysicalPosition::new(0.0, PANE_PARK_Y),
                    tauri::PhysicalSize::new(800.0, 600.0),
                ) {
                    eprintln!("[swervebuild] debug pane add_child: {e}");
                }
            });
            // S13c: hold a persistent CDP session to the pane so the console/
            // network capture hooks survive every navigation (human or agent).
            browser_debug::spawn_pane_supervisor(app.handle().clone());
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
            import_attachment_files,
            artifact_store_status,
            artifact_store_resync,
            artifact_store_prune,
            media_worker_status,
            media_worker_start,
            media_worker_stop,
            media_worker_capture_still,
            media_worker_encode_clip,
            media_worker_ffmpeg_status,
            media_worker_ensure_ffmpeg,
            detect_chat_media,
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
            list_media_providers,
            set_image_provider,
            set_video_provider,
            set_comfy_base_url,
            probe_local_image,
            generate_local_image,
            get_local_state,
            install_local_engine,
            add_local_model,
            remove_local_model,
            start_local_server,
            stop_local_server,
            get_model_catalog,
            set_models_dir,
            download_catalog_model,
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
            automation_failure_count,
            get_app_ui_grant,
            set_app_ui_grant,
            get_term_grant,
            set_term_grant,
            get_browser_debug_grant,
            secret_set,
            secret_status,
            secret_delete,
            stream_key_name,
            live_status,
            get_live_grant,
            set_live_grant,
            live_request_arm,
            live_confirm_arm,
            live_go_live,
            live_stop,
            live_panic_cut,
            live_clear_privacy_cut,
            live_set_agent_scene_control,
            live_recent_events,
            provider_install_info,
            install_provider_cli,
            uninstall_provider_cli,
            provider_auth_probe,
            provider_sign_in,
            set_browser_debug_grant,
            set_browser_public,
            browser_pane_set_bounds,
            browser_pane_park,
            browser_pane_open,
            browser_pane_nav,
            publish_app_ui_state,
            workflows_tauri::workflows_list,
            workflows_tauri::workflow_get,
            workflows_tauri::workflow_save,
            workflows_tauri::workflow_delete,
            workflows_tauri::workflow_validate,
            workflows_tauri::workflow_node_catalog,
            workflows_tauri::workflow_run_now,
            workflows_tauri::workflow_cancel_run,
            workflows_tauri::workflow_runs,
            workflows_tauri::workflow_run_detail,
            workflows_tauri::workflows_set_paused,
            workflows_tauri::workflows_get_paused,
            workflows_tauri::workflow_secret_names,
            workflows_tauri::workflow_secret_set,
            workflows_tauri::workflow_secret_delete
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(move |_app, event| {
            if let tauri::RunEvent::Exit = event {
                acp_exit.close_all();
                jobs_exit.cancel_all();
                wf_exit.cancel_all();
                term_exit.kill_all();
                media_worker::shutdown_on_exit();
                local_llm::manager().shutdown();
            }
        });
}

#[cfg(test)]
mod grok_install_tests {
    use super::*;

    /// S37 regression: npm ships an extensionless Unix script alongside the
    /// Windows `.cmd`, and `where` lists the script first. Spawning it gives
    /// "%1 is not a valid Win32 application" (os error 193) — the exact failure
    /// that made freshly-installed claude-code-acp / gemini unusable while the
    /// UI still said "Available".
    ///
    /// S38b: this now guards the shipping path. `which_on_path` delegates to
    /// `prefer_executable`, so there is one implementation and this test cannot
    /// drift away from it — until S38b it was asserting against a duplicate.
    #[cfg(windows)]
    #[test]
    fn prefers_windows_shim_over_npm_shell_script() {
        let npm_style = "C:\\Users\\x\\AppData\\Roaming\\npm\\claude-code-acp\n\
             C:\\Users\\x\\AppData\\Roaming\\npm\\claude-code-acp.cmd\n\
             C:\\Users\\x\\AppData\\Roaming\\npm\\claude-code-acp.ps1\n";
        let picked = prefer_executable(npm_style).unwrap();
        assert_eq!(
            picked.extension().and_then(|e| e.to_str()),
            Some("cmd"),
            "must pick the .cmd shim, got {picked:?}"
        );

        // A plain .exe on PATH is still chosen.
        let exe_only = "C:\\tools\\grok.exe\n";
        assert_eq!(
            prefer_executable(exe_only).unwrap().extension().and_then(|e| e.to_str()),
            Some("exe")
        );

        // No executable extension anywhere → fall back to the first line
        // rather than returning nothing.
        let none = "C:\\tools\\weird-thing\n";
        assert_eq!(prefer_executable(none).unwrap(), PathBuf::from("C:\\tools\\weird-thing"));
        assert!(prefer_executable("").is_none());
    }

    /// The same guarantee against the REAL PATH, through the real entry point.
    /// On this box `where claude-code-acp` still lists the extensionless Unix
    /// script FIRST, so a resolver that took line 1 would hand os error 193 to
    /// the ACP layer — which is precisely the S37 failure. Spawn-and-kill is
    /// enough: 193 is raised at spawn, and not waiting avoids hanging on an ACP
    /// server that would sit waiting for stdio.
    ///
    /// `cargo test -p swerve-build --lib grok_install_tests::live_npm_cli_resolves_spawnable -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_npm_cli_resolves_spawnable() {
        let mut checked = 0;
        for cli in ["claude-code-acp", "gemini"] {
            let Some(path) = which_on_path(cli) else {
                eprintln!("{cli}: not on PATH — skipped");
                continue;
            };
            checked += 1;
            eprintln!("{cli} -> {}", path.display());
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            assert!(
                matches!(ext.as_deref(), Some("exe" | "cmd" | "bat" | "com")),
                "{cli} resolved to {path:?}, which Windows cannot spawn"
            );
            let mut child = util::hidden_command(&path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap_or_else(|e| panic!("{cli} at {path:?} failed to spawn: {e}"));
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("{cli}: spawned OK (no os error 193)");
        }
        assert!(checked > 0, "no provider CLI on PATH — nothing was proven");
    }

    #[test]
    fn pin_constants_agree() {
        assert!(
            GROK_CLI_URL.contains(GROK_CLI_VERSION),
            "primary URL must embed the pin version"
        );
        assert!(
            GROK_CLI_URL_FALLBACK.contains(GROK_CLI_VERSION),
            "fallback URL must embed the pin version"
        );
        assert_eq!(
            GROK_CLI_SHA256.len(),
            64,
            "SHA-256 hex should be 64 chars"
        );
        assert!(GROK_CLI_SIZE > 10_000_000, "binary should be multi-MB");
        // A5 regression: never ship the unpinned remote-script pipe.
        assert!(
            !GROK_CLI_URL.contains("install.ps1") && !GROK_CLI_URL.contains("install.sh"),
            "must pin the binary artifact, not the install script"
        );
    }

    #[test]
    fn version_match_accepts_common_grok_output() {
        assert!(version_matches_pin("0.2.106", "0.2.106"));
        assert!(version_matches_pin("grok 0.2.106", "0.2.106"));
        assert!(version_matches_pin("grok-cli v0.2.106", "0.2.106"));
        assert!(!version_matches_pin("0.2.93", "0.2.106"));
        assert!(!version_matches_pin("0.2.1060", "0.2.106"));
    }
}