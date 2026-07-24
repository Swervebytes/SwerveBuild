//! Media worker process shell (S24+ / Step 7).
//!
//! Separate process so capture/encode crashes never take down chat.
//! Still PNG + short MJPEG clip (S25–S26); pinned LGPL FFmpeg (S27); **optional
//! dshow audio track on clip** (S28 — WASAPI not in the pinned LGPL build).
//!
//! IPC: loopback HTTP, versioned path `/v1/*`, Bearer token (same idea as
//! terminal control server). Endpoint published under data dir.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Wire protocol version (bump when request/response shapes break).
pub const PROTOCOL_VERSION: u32 = 1;
const ENDPOINT_FILE: &str = "media_worker.json";
const READY_PREFIX: &str = "SWERVE_MEDIA_READY ";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerEndpoint {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub protocol: u32,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthBody {
    pub ok: bool,
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
    pub uptime_ms: u64,
    pub role: String,
    /// Future: capture, encode, etc. Empty in S24 shell.
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorStatus {
    pub running: bool,
    pub healthy: bool,
    pub endpoint: Option<WorkerEndpoint>,
    pub last_error: Option<String>,
    pub note: String,
}

struct LiveWorker {
    child: Child,
    endpoint: WorkerEndpoint,
}

static LIVE: Mutex<Option<LiveWorker>> = Mutex::new(None);
static LAST_ERR: Mutex<Option<String>> = Mutex::new(None);

fn endpoint_path() -> PathBuf {
    crate::paths::data_dir().join(ENDPOINT_FILE)
}

fn publish_endpoint(ep: &WorkerEndpoint) -> Result<(), String> {
    let path = endpoint_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(ep).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())
}

fn clear_endpoint_file() {
    let _ = std::fs::remove_file(endpoint_path());
}

fn load_endpoint_file() -> Option<WorkerEndpoint> {
    let path = endpoint_path();
    if !path.is_file() {
        return None;
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

/// Prefer this process's worker, then a healthy endpoint published by the app
/// (or another supervisor), else spawn our own. Lets MCP drive media without
/// fighting a second worker when the desktop app is already up.
fn resolve_endpoint() -> Result<WorkerEndpoint, String> {
    if let Ok(g) = LIVE.lock() {
        if let Some(live) = g.as_ref() {
            if http_get_health(&live.endpoint).is_ok() {
                return Ok(live.endpoint.clone());
            }
        }
    }
    if let Some(ep) = load_endpoint_file() {
        if http_get_health(&ep).is_ok() {
            return Ok(ep);
        }
    }
    let st = ensure_running()?;
    if !st.healthy {
        return Err(st
            .last_error
            .unwrap_or_else(|| "media worker not healthy".into()));
    }
    let g = LIVE.lock().map_err(|_| "media worker lock poisoned".to_string())?;
    g.as_ref()
        .map(|l| l.endpoint.clone())
        .ok_or_else(|| "media worker not running after ensure".into())
}

/// Copy a media file into `<project>/swerve-media/` (hero-proof project artifact).
pub fn copy_to_project(src: &str, project_id: &str) -> Result<String, String> {
    let id = project_id.trim();
    if id.is_empty() {
        return Err("project_id empty".into());
    }
    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return Err("invalid project_id".into());
    }
    let store = crate::store::Store::load();
    let project = store
        .projects
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("project not found: {id}"))?;
    let root = PathBuf::from(&project.path);
    if !root.is_dir() {
        return Err(format!("project path missing: {}", root.display()));
    }
    let dest_dir = root.join("swerve-media");
    std::fs::create_dir_all(&dest_dir).map_err(|e| format!("create swerve-media: {e}"))?;
    let src_path = PathBuf::from(src);
    if !src_path.is_file() {
        return Err(format!("source missing: {src}"));
    }
    let name = src_path
        .file_name()
        .ok_or_else(|| "source has no file name".to_string())?;
    let dest = dest_dir.join(name);
    std::fs::copy(&src_path, &dest).map_err(|e| format!("copy to project: {e}"))?;
    Ok(dest.display().to_string())
}

fn set_err(msg: impl Into<String>) {
    if let Ok(mut g) = LAST_ERR.lock() {
        *g = Some(msg.into());
    }
}

fn clear_err() {
    if let Ok(mut g) = LAST_ERR.lock() {
        *g = None;
    }
}

/// Resolve the worker binary next to the app / build tree (same idea as MCP).
pub fn resolve_worker_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current_exe has no parent".to_string())?;
    let candidates = [
        dir.join("swervebuild-media.exe"),
        dir.join("swervebuild-media"),
        dir.join("swervebuild_media.exe"),
        // Dev: cargo run from target/debug
        PathBuf::from("target/debug/swervebuild-media.exe"),
        PathBuf::from("target/debug/swervebuild-media"),
        PathBuf::from("src-tauri/target/debug/swervebuild-media.exe"),
        PathBuf::from("src-tauri/target/release/swervebuild-media.exe"),
        dir.join("../swervebuild-media.exe"),
    ];
    for c in candidates {
        if c.is_file() {
            return Ok(c);
        }
    }
    // Same package binary name as Cargo [[bin]] name with hyphens.
    let alt = dir.join(format!(
        "swervebuild-media{}",
        std::env::consts::EXE_SUFFIX
    ));
    if alt.is_file() {
        return Ok(alt);
    }
    Err(
        "swervebuild-media binary not found. Rebuild the project (cargo build -p swerve-build --bin swervebuild-media)."
            .into(),
    )
}

fn http_get_health(ep: &WorkerEndpoint) -> Result<HealthBody, String> {
    let url = format!("http://{}:{}/v1/health", ep.host, ep.port);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(500))
        .timeout(Duration::from_secs(2))
        .build();
    let resp = agent
        .get(&url)
        .set("Authorization", &format!("Bearer {}", ep.token))
        .call()
        .map_err(|e| format!("health GET: {e}"))?;
    resp.into_json::<HealthBody>()
        .map_err(|e| format!("health JSON: {e}"))
}

/// Start the worker if not already running and healthy.
pub fn ensure_running() -> Result<SupervisorStatus, String> {
    {
        let mut g = LIVE.lock().map_err(|_| "media worker lock poisoned".to_string())?;
        if let Some(live) = g.as_mut() {
            // Reap dead children.
            match live.child.try_wait() {
                Ok(Some(status)) => {
                    set_err(format!("worker exited: {status}"));
                    *g = None;
                    clear_endpoint_file();
                }
                Ok(None) => {
                    if http_get_health(&live.endpoint).is_ok() {
                        clear_err();
                        return Ok(status_locked(g.as_ref(), true));
                    }
                }
                Err(e) => {
                    set_err(format!("try_wait: {e}"));
                    let _ = live.child.kill();
                    *g = None;
                    clear_endpoint_file();
                }
            }
        }
    }

    let bin = resolve_worker_binary()?;
    let token = uuid::Uuid::new_v4().to_string();
    let out_dir = crate::paths::attachments_dir();
    let _ = std::fs::create_dir_all(&out_dir);
    let mut child = crate::util::hidden_command(&bin)
        .arg("--token")
        .arg(&token)
        .arg("--out-dir")
        .arg(out_dir.as_os_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn media worker: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "worker stdout missing".to_string())?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    // Wait for READY line (max ~5s).
    let deadline = Instant::now() + Duration::from_secs(5);
    let port = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("media worker did not publish READY in time".into());
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = child.kill();
                return Err("media worker exited before READY".into());
            }
            Ok(_) => {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix(READY_PREFIX) {
                    #[derive(Deserialize)]
                    struct Ready {
                        port: u16,
                        protocol: u32,
                    }
                    let ready: Ready = serde_json::from_str(rest)
                        .map_err(|e| format!("parse READY: {e} ({rest})"))?;
                    if ready.protocol != PROTOCOL_VERSION {
                        let _ = child.kill();
                        return Err(format!(
                            "worker protocol {} != app {}",
                            ready.protocol, PROTOCOL_VERSION
                        ));
                    }
                    break ready.port;
                }
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("read worker stdout: {e}"));
            }
        }
    };

    // Drain remaining stdout/stderr so the pipe never fills.
    thread::spawn(move || {
        let mut sink = String::new();
        let _ = reader.read_to_string(&mut sink);
    });
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            let mut r = BufReader::new(stderr);
            let mut buf = String::new();
            while r.read_line(&mut buf).ok().filter(|n| *n > 0).is_some() {
                eprintln!("[swerve-media] {}", buf.trim_end());
                buf.clear();
            }
        });
    }

    let pid = child.id();
    let ep = WorkerEndpoint {
        host: "127.0.0.1".into(),
        port,
        token,
        pid,
        protocol: PROTOCOL_VERSION,
        updated_at: crate::store::Store::now(),
    };
    publish_endpoint(&ep)?;

    // Confirm health once.
    for _ in 0..20 {
        if http_get_health(&ep).is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if let Err(e) = http_get_health(&ep) {
        let _ = child.kill();
        clear_endpoint_file();
        return Err(format!("worker started but health failed: {e}"));
    }

    let mut g = LIVE.lock().map_err(|_| "media worker lock poisoned".to_string())?;
    *g = Some(LiveWorker {
        child,
        endpoint: ep,
    });
    clear_err();
    Ok(status_locked(g.as_ref(), true))
}

/// Stop the worker if running.
pub fn stop() -> Result<SupervisorStatus, String> {
    let mut g = LIVE.lock().map_err(|_| "media worker lock poisoned".to_string())?;
    if let Some(mut live) = g.take() {
        let _ = live.child.kill();
        let _ = live.child.wait();
    }
    clear_endpoint_file();
    clear_err();
    Ok(status_locked(None, false))
}

pub fn status() -> SupervisorStatus {
    let g = match LIVE.lock() {
        Ok(g) => g,
        Err(_) => {
            return SupervisorStatus {
                running: false,
                healthy: false,
                endpoint: None,
                last_error: Some("lock poisoned".into()),
                note: "media worker supervisor unavailable".into(),
            };
        }
    };
    // Drop dead process.
    // (Need mut — re-lock)
    drop(g);
    let mut g = LIVE.lock().unwrap();
    if let Some(live) = g.as_mut() {
        if let Ok(Some(status)) = live.child.try_wait() {
            set_err(format!("worker exited: {status}"));
            *g = None;
            clear_endpoint_file();
        }
    }
    let healthy = g
        .as_ref()
        .map(|l| http_get_health(&l.endpoint).is_ok())
        .unwrap_or(false);
    status_locked(g.as_ref(), healthy)
}

fn status_locked(live: Option<&LiveWorker>, healthy: bool) -> SupervisorStatus {
    let err = LAST_ERR.lock().ok().and_then(|g| g.clone());
    SupervisorStatus {
        running: live.is_some(),
        healthy,
        endpoint: live.map(|l| {
            let mut e = l.endpoint.clone();
            // Never echo full token to UI in future — for S24 status we redact.
            e.token = if e.token.len() > 8 {
                format!("{}…", &e.token[..8])
            } else {
                "(set)".into()
            };
            e
        }),
        last_error: err,
        note: if live.is_none() {
            "Worker not running. Call media_worker_start (S25: still capture available when up)."
                .into()
        } else if healthy {
            format!(
                "Media worker up (protocol v{PROTOCOL_VERSION}). Capabilities: still_png, clip_mjpeg, clip_audio."
            )
        } else {
            "Worker process present but health check failed.".into()
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStillResult {
    pub ok: bool,
    pub path: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// Ensure worker is up, capture primary display to attachments as PNG.
pub fn capture_still() -> Result<CaptureStillResult, String> {
    capture_still_opts(None)
}

/// Capture still; optional `project_id` also copies PNG into `<project>/swerve-media/`.
pub fn capture_still_opts(project_id: Option<&str>) -> Result<CaptureStillResult, String> {
    let ep = resolve_endpoint()?;
    let url = format!("http://{}:{}/v1/capture/still", ep.host, ep.port);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(Duration::from_secs(30))
        .build();
    let resp = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {}", ep.token))
        .set("Content-Type", "application/json")
        .send_string("{}")
        .map_err(|e| format!("capture still: {e}"))?;
    let mut body: CaptureStillResult = resp
        .into_json()
        .map_err(|e| format!("capture still JSON: {e}"))?;
    if !body.ok {
        return Err("capture still returned ok=false".into());
    }
    if let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) {
        body.path = copy_to_project(&body.path, pid)?;
    }
    // Budget enforcement after large binary write.
    crate::artifacts::maybe_enforce_after_write();
    let _ = crate::db::upsert_artifact(
        &uuid::Uuid::new_v4().to_string(),
        "attachment",
        &body.path,
        body.bytes,
        &crate::store::Store::now(),
        None,
        Some("media_worker_still"),
    );
    Ok(body)
}

/// Kill worker on app exit.
pub fn shutdown_on_exit() {
    let _ = stop();
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodeClipResult {
    pub ok: bool,
    pub path: String,
    pub bytes: u64,
    pub duration_secs: f64,
    pub still_path: String,
    pub codec: String,
    /// True when an audio track was muxed into the clip.
    #[serde(default)]
    pub has_audio: bool,
    /// dshow device name used, if any.
    #[serde(default)]
    pub audio_device: Option<String>,
    /// `dshow` | `silent` (more sources later if the pin gains wasapi).
    #[serde(default = "default_audio_mode_label")]
    pub audio_mode: String,
}

fn default_audio_mode_label() -> String {
    "silent".into()
}

/// Capture a still (if needed) and encode a short clip via worker + FFmpeg.
/// Audio: auto dshow mic when available; silent fallback (S28).
pub fn encode_clip(still_path: Option<String>) -> Result<EncodeClipResult, String> {
    encode_clip_opts(still_path, 2.0, "auto", None)
}

/// Encode options for MCP / hero path (S29).
/// `audio`: `auto` | `none` | `dshow`. Optional `project_id` copies clip to project.
pub fn encode_clip_opts(
    still_path: Option<String>,
    duration_secs: f64,
    audio: &str,
    project_id: Option<&str>,
) -> Result<EncodeClipResult, String> {
    let duration = duration_secs.clamp(0.5, 10.0);
    let audio = if audio.trim().is_empty() {
        "auto"
    } else {
        audio.trim()
    };

    let still = match still_path {
        Some(p) if !p.trim().is_empty() => {
            let path = PathBuf::from(p.trim());
            if !path.is_file() {
                return Err(format!("still not found: {}", path.display()));
            }
            CaptureStillResult {
                ok: true,
                path: path.display().to_string(),
                bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                width: 0,
                height: 0,
            }
        }
        _ => capture_still()?,
    };

    let ep = resolve_endpoint()?;
    let url = format!("http://{}:{}/v1/encode/clip", ep.host, ep.port);
    let body_in = serde_json::json!({
        "stillPath": still.path,
        "durationSecs": duration,
        "audio": audio,
    });
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(Duration::from_secs(120))
        .build();
    let resp = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {}", ep.token))
        .set("Content-Type", "application/json")
        .send_string(&body_in.to_string())
        .map_err(|e| format!("encode clip: {e}"))?;
    let mut body: EncodeClipResult = resp
        .into_json()
        .map_err(|e| format!("encode clip JSON: {e}"))?;
    if !body.ok {
        return Err("encode clip returned ok=false".into());
    }
    if let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) {
        body.path = copy_to_project(&body.path, pid)?;
    }
    crate::artifacts::maybe_enforce_after_write();
    let _ = crate::db::upsert_artifact(
        &uuid::Uuid::new_v4().to_string(),
        "attachment",
        &body.path,
        body.bytes,
        &crate::store::Store::now(),
        None,
        Some("media_worker_clip"),
    );
    Ok(body)
}

/// Combined status for agents (worker + FFmpeg).
pub fn agent_status_report() -> serde_json::Value {
    let worker = status();
    let ff = ffmpeg_status();
    serde_json::json!({
        "workerRunning": worker.running,
        "workerHealthy": worker.healthy,
        "workerNote": worker.note,
        "workerError": worker.last_error,
        "capabilities": ["still_png", "clip_mjpeg", "clip_audio"],
        "ffmpeg": {
            "tag": ff.tag,
            "installed": ff.installed,
            "path": ff.path,
            "resolveSource": ff.resolve_source,
        },
        "attachmentsDir": crate::paths::attachments_dir().display().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Pinned FFmpeg (S27) — LGPL win64 static, tag + SHA-256 (same ritual as llama engine)
// ---------------------------------------------------------------------------

/// Pinned BtbN LGPL win64 static build (n7.1 line). Upgrade: bump all four together.
pub const FFMPEG_TAG: &str = "n7.1-lgpl-win64";
const FFMPEG_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-win64-lgpl-7.1.zip";
const FFMPEG_SHA256: &str = "6c943e93c59653eb5e39b498f89f073f29874598c0c7b3dd828a21a1665d096a";
const FFMPEG_SIZE: u64 = 139_446_530;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegStatus {
    pub tag: String,
    pub installed: bool,
    pub path: Option<String>,
    pub source: String,
    /// Where resolve would get FFmpeg today: pin | flat | path | missing
    pub resolve_source: String,
}

pub fn ffmpeg_install_dir() -> PathBuf {
    crate::paths::data_dir().join("ffmpeg").join(FFMPEG_TAG)
}

pub fn ffmpeg_pinned_exe() -> PathBuf {
    // Prefer bin/ layout after unpack; fall back to flat under tag dir.
    let bin = ffmpeg_install_dir().join("bin").join("ffmpeg.exe");
    if bin.is_file() {
        return bin;
    }
    ffmpeg_install_dir().join("ffmpeg.exe")
}

pub fn ffmpeg_pinned_installed() -> bool {
    ffmpeg_pinned_exe().is_file()
}

/// Status for Settings / agent diagnostics.
pub fn ffmpeg_status() -> FfmpegStatus {
    let (path, source) = match resolve_ffmpeg_no_install() {
        Ok((p, s)) => (Some(p.display().to_string()), s),
        Err(_) => (None, "missing".to_string()),
    };
    FfmpegStatus {
        tag: FFMPEG_TAG.to_string(),
        installed: ffmpeg_pinned_installed(),
        path: path.clone(),
        source: source.clone(),
        resolve_source: source,
    }
}

/// Download + verify + unpack the pinned LGPL FFmpeg build. Blocking.
pub fn install_ffmpeg() -> Result<String, String> {
    if ffmpeg_pinned_installed() {
        return Ok(format!(
            "FFmpeg {FFMPEG_TAG} already installed at {}",
            ffmpeg_pinned_exe().display()
        ));
    }
    let dir = ffmpeg_install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create ffmpeg dir: {e}"))?;
    let zip = dir.join("ffmpeg-pin.zip");

    let status = crate::util::hidden_command("curl.exe")
        .args([
            "-L",
            "--fail",
            "--retry",
            "3",
            "-C",
            "-",
            "-o",
            &zip.display().to_string(),
            FFMPEG_URL,
        ])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            return Err(format!(
                "FFmpeg download failed (curl exit {:?})",
                s.code()
            ))
        }
        Err(e) => return Err(format!("could not run curl.exe: {e}")),
    }

    let got = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
    if got == 0 {
        let _ = std::fs::remove_file(&zip);
        return Err("FFmpeg download empty".into());
    }
    // Soft size check (exact size may drift if upstream re-packs; SHA is authoritative).
    if FFMPEG_SIZE > 0 && (got as i64 - FFMPEG_SIZE as i64).unsigned_abs() > FFMPEG_SIZE / 5 {
        // more than 20% off — still allow if SHA matches
    }

    let hash = file_sha256_ps(&zip)?;
    if !hash.eq_ignore_ascii_case(FFMPEG_SHA256) {
        let _ = std::fs::remove_file(&zip);
        return Err(format!(
            "FFmpeg checksum mismatch (got {hash}); download removed — bump pin or retry"
        ));
    }

    // Windows 10+ tar handles zip; more reliable than Expand-Archive under CREATE_NO_WINDOW.
    let unzip = crate::util::hidden_command("tar.exe")
        .args([
            "-xf",
            &zip.display().to_string(),
            "-C",
            &dir.display().to_string(),
        ])
        .status();
    match unzip {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(format!("FFmpeg unzip failed (tar exit {:?})", s.code())),
        Err(e) => return Err(format!("FFmpeg unzip failed (tar: {e})")),
    }
    let _ = std::fs::remove_file(&zip);

    // BtbN packs as ffmpeg-n7.1-.../bin/ffmpeg.exe — flatten into our tag dir.
    if !ffmpeg_pinned_installed() {
        if let Some(found) = find_named_file(&dir, "ffmpeg.exe") {
            let dest_bin = dir.join("bin");
            let _ = std::fs::create_dir_all(&dest_bin);
            let dest = dest_bin.join("ffmpeg.exe");
            if found != dest {
                let _ = std::fs::copy(&found, &dest);
            }
            // Also copy sibling tools if present (optional).
            if let Some(parent) = found.parent() {
                for name in ["ffprobe.exe", "ffplay.exe"] {
                    let src = parent.join(name);
                    if src.is_file() {
                        let _ = std::fs::copy(&src, dest_bin.join(name));
                    }
                }
            }
        }
    }
    if !ffmpeg_pinned_installed() {
        return Err(
            "FFmpeg unpacked but ffmpeg.exe was not found — archive layout unexpected".into(),
        );
    }
    Ok(format!(
        "FFmpeg {FFMPEG_TAG} installed ({}).",
        ffmpeg_pinned_exe().display()
    ))
}

/// Ensure pinned FFmpeg exists (download if needed), then resolve.
pub fn ensure_ffmpeg() -> Result<PathBuf, String> {
    if !ffmpeg_pinned_installed() {
        // Prefer pin for shipping; still allow PATH/flat without download if present.
        if resolve_ffmpeg_no_install().is_ok() {
            return resolve_ffmpeg_no_install().map(|(p, _)| p);
        }
        install_ffmpeg()?;
    }
    resolve_ffmpeg()
}

/// Locate FFmpeg: pinned install → flat `~/.swervebuild/ffmpeg/ffmpeg.exe` → PATH.
pub fn resolve_ffmpeg() -> Result<PathBuf, String> {
    resolve_ffmpeg_no_install().map(|(p, _)| p)
}

fn resolve_ffmpeg_no_install() -> Result<(PathBuf, String), String> {
    let pin = ffmpeg_pinned_exe();
    if pin.is_file() {
        return Ok((pin, "pin".into()));
    }
    let flat = crate::paths::data_dir().join("ffmpeg").join("ffmpeg.exe");
    if flat.is_file() {
        return Ok((flat, "flat".into()));
    }
    if let Ok(p) = which_ffmpeg_on_path() {
        return Ok((p, "path".into()));
    }
    Err(
        "FFmpeg not found. Call media_worker_ensure_ffmpeg to download the pinned LGPL build, install FFmpeg on PATH, or place ffmpeg.exe under ~/.swervebuild/ffmpeg/."
            .into(),
    )
}

fn which_ffmpeg_on_path() -> Result<PathBuf, String> {
    let out = crate::util::hidden_command("where.exe")
        .arg("ffmpeg")
        .output()
        .map_err(|e| format!("where ffmpeg: {e}"))?;
    if !out.status.success() {
        return Err("ffmpeg not on PATH".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && Path::new(l).is_file())
        .ok_or_else(|| "ffmpeg not on PATH".to_string())?;
    Ok(PathBuf::from(line))
}

fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn file_sha256_ps(path: &Path) -> Result<String, String> {
    // Prefer certutil (ships with Windows; quieter than PowerShell under CREATE_NO_WINDOW).
    let path_s = path.display().to_string();
    let cert = crate::util::hidden_command("certutil")
        .args(["-hashfile", &path_s, "SHA256"])
        .output()
        .map_err(|e| format!("certutil hash: {e}"))?;
    if cert.status.success() {
        // Output: "SHA256 hash of <file>:\n<hex>\nCertUtil: ..."
        let text = String::from_utf8_lossy(&cert.stdout);
        for line in text.lines() {
            let t = line.trim().replace(' ', "");
            if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(t);
            }
        }
    }
    let output = crate::util::hidden_command("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash",
                ps_quote(&path_s)
            ),
        ])
        .output()
        .map_err(|e| format!("hash: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "hashing the FFmpeg download failed (exit {:?}): stderr={err} stdout={out}",
            output.status.code()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn find_named_file(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(p);
        }
        if p.is_dir() {
            if let Some(found) = find_named_file(&p, name) {
                return Some(found);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Worker process entry (used by bin/swervebuild_media.rs)
// ---------------------------------------------------------------------------

static WORKER_START: Mutex<Option<Instant>> = Mutex::new(None);

/// Run the media worker main loop (blocks).
/// Args: `--token <uuid> --out-dir <path>`
pub fn worker_main(args: &[String]) -> i32 {
    let mut token = None::<String>;
    let mut out_dir = None::<PathBuf>;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--token" if i + 1 < args.len() => {
                token = Some(args[i + 1].clone());
                i += 2;
            }
            "--out-dir" if i + 1 < args.len() => {
                out_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other => {
                eprintln!("unknown arg: {other}");
                return 2;
            }
        }
    }
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        eprintln!("usage: swervebuild-media --token <uuid> --out-dir <path>");
        return 2;
    };
    let out_dir = out_dir.unwrap_or_else(|| std::env::temp_dir().join("swerve-media-out"));
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("out-dir: {e}");
        return 1;
    }

    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("bind: {e}");
            return 1;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            eprintln!("local_addr: {e}");
            return 1;
        }
    };

    *WORKER_START.lock().unwrap() = Some(Instant::now());

    let ready = serde_json::json!({
        "port": port,
        "protocol": PROTOCOL_VERSION,
        "pid": std::process::id(),
        "capabilities": ["still_png", "clip_mjpeg", "clip_audio"],
    });
    println!("{READY_PREFIX}{ready}");
    let _ = std::io::stdout().flush();

    let out_dir = std::sync::Arc::new(out_dir);
    let running = AtomicBool::new(true);
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let tok = token.clone();
                let dir = std::sync::Arc::clone(&out_dir);
                thread::spawn(move || handle_client(stream, &tok, &dir));
            }
            Err(e) => {
                eprintln!("accept: {e}");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    0
}

fn handle_client(mut stream: TcpStream, token: &str, out_dir: &PathBuf) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok().unwrap_or(0) == 0 {
        return;
    }
    let mut auth = None::<String>;
    let mut content_length = 0usize;
    let mut headers = String::new();
    loop {
        headers.clear();
        if reader.read_line(&mut headers).ok().unwrap_or(0) == 0 {
            break;
        }
        if headers == "\r\n" || headers == "\n" {
            break;
        }
        if let Some(rest) = headers
            .strip_prefix("Authorization:")
            .or_else(|| headers.strip_prefix("authorization:"))
        {
            auth = Some(rest.trim().to_string());
        }
        if let Some(rest) = headers
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
        {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    // Drain body if any (POST may send JSON).
    let mut body_bytes = Vec::new();
    if content_length > 0 && content_length < 1_000_000 {
        body_bytes.resize(content_length, 0);
        let _ = reader.read_exact(&mut body_bytes);
    }

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("");
    let path = parts.get(1).copied().unwrap_or("/");

    let authorized = auth
        .as_deref()
        .and_then(|a| a.strip_prefix("Bearer "))
        .map(|t| t == token)
        .unwrap_or(false);

    if !authorized {
        let body = r#"{"ok":false,"error":"unauthorized"}"#;
        let _ = write_http(&mut stream, 401, "application/json", body);
        return;
    }

    if method == "GET" && path == "/v1/health" {
        let uptime = WORKER_START
            .lock()
            .ok()
            .and_then(|g| g.map(|t| t.elapsed().as_millis() as u64))
            .unwrap_or(0);
        let health = HealthBody {
            ok: true,
            protocol: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            uptime_ms: uptime,
            role: "media-worker".into(),
            capabilities: vec![
                "still_png".into(),
                "clip_mjpeg".into(),
                "clip_audio".into(),
            ],
        };
        let body = serde_json::to_string(&health).unwrap_or_else(|_| "{}".into());
        let _ = write_http(&mut stream, 200, "application/json", &body);
        return;
    }

    if method == "POST" && path == "/v1/encode/clip" {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct EncodeReq {
            still_path: Option<String>,
            duration_secs: Option<f64>,
            /// `auto` (default) | `none` | `dshow`
            audio: Option<String>,
            /// Optional exact DirectShow audio device name.
            audio_device: Option<String>,
        }
        let req: EncodeReq = serde_json::from_slice(&body_bytes).unwrap_or(EncodeReq {
            still_path: None,
            duration_secs: Some(2.0),
            audio: Some("auto".into()),
            audio_device: None,
        });
        let duration = req.duration_secs.unwrap_or(2.0).clamp(0.5, 10.0);
        let audio_pref = req
            .audio
            .as_deref()
            .unwrap_or("auto")
            .trim()
            .to_ascii_lowercase();
        let still = match req.still_path.filter(|p| !p.trim().is_empty()) {
            Some(p) => PathBuf::from(p),
            None => match capture_primary_still(out_dir) {
                Ok((p, _, _, _)) => p,
                Err(e) => {
                    let body = serde_json::json!({"ok": false, "error": e});
                    let _ = write_http(&mut stream, 500, "application/json", &body.to_string());
                    return;
                }
            },
        };
        match encode_still_mjpeg_clip(
            &still,
            out_dir,
            duration,
            &audio_pref,
            req.audio_device.as_deref(),
        ) {
            Ok(result) => {
                let s = serde_json::to_string(&result).unwrap_or_else(|_| "{}".into());
                let _ = write_http(&mut stream, 200, "application/json", &s);
            }
            Err(e) => {
                let body = serde_json::json!({"ok": false, "error": e});
                let _ = write_http(&mut stream, 500, "application/json", &body.to_string());
            }
        }
        return;
    }

    if method == "POST" && path == "/v1/capture/still" {
        match capture_primary_still(out_dir) {
            Ok((path, width, height, bytes)) => {
                let body = CaptureStillResult {
                    ok: true,
                    path: path.display().to_string(),
                    bytes,
                    width,
                    height,
                };
                let s = serde_json::to_string(&body).unwrap_or_else(|_| "{}".into());
                let _ = write_http(&mut stream, 200, "application/json", &s);
            }
            Err(e) => {
                let body = serde_json::json!({"ok": false, "error": e});
                let s = body.to_string();
                let _ = write_http(&mut stream, 500, "application/json", &s);
            }
        }
        return;
    }

    if method == "POST" && path == "/v1/shutdown" {
        let body = r#"{"ok":true}"#;
        let _ = write_http(&mut stream, 200, "application/json", body);
        thread::spawn(|| {
            thread::sleep(Duration::from_millis(50));
            std::process::exit(0);
        });
        return;
    }

    let body = r#"{"ok":false,"error":"not_found"}"#;
    let _ = write_http(&mut stream, 404, "application/json", body);
}

/// Capture the primary monitor into `out_dir` as PNG (S25).
fn capture_primary_still(out_dir: &Path) -> Result<(PathBuf, u32, u32, u64), String> {
    use xcap::Monitor;
    let monitors = Monitor::all().map_err(|e| format!("list monitors: {e}"))?;
    let mon = monitors
        .into_iter()
        .next()
        .ok_or_else(|| "no monitors found".to_string())?;
    let img = mon
        .capture_image()
        .map_err(|e| format!("capture_image: {e}"))?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let name = format!("still-{}.png", uuid::Uuid::new_v4());
    let path = out_dir.join(&name);
    img.save(&path)
        .map_err(|e| format!("save png: {e}"))?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    Ok((path, img.width(), img.height(), meta.len()))
}

/// Short clip from a still via FFmpeg MJPEG (S26) + optional dshow audio (S28).
/// Avoids GPL x264; pinned build has dshow but not wasapi.
fn encode_still_mjpeg_clip(
    still: &Path,
    out_dir: &Path,
    duration_secs: f64,
    audio_pref: &str,
    audio_device: Option<&str>,
) -> Result<EncodeClipResult, String> {
    if !still.is_file() {
        return Err(format!("still missing: {}", still.display()));
    }
    // S27: ensure pinned LGPL FFmpeg (or existing PATH/flat) before encode.
    let ffmpeg = ensure_ffmpeg()?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let out = out_dir.join(format!("clip-{}.avi", uuid::Uuid::new_v4()));
    let dur = format!("{duration_secs:.2}");

    let want_audio = !matches!(audio_pref, "none" | "off" | "silent" | "an");
    let device = if want_audio {
        resolve_dshow_audio_device(&ffmpeg, audio_device)
    } else {
        None
    };

    if let Some(ref dev) = device {
        match run_ffmpeg_clip(&ffmpeg, still, &out, &dur, Some(dev.as_str())) {
            Ok(()) => {
                let meta = std::fs::metadata(&out).map_err(|e| e.to_string())?;
                return Ok(EncodeClipResult {
                    ok: true,
                    path: out.display().to_string(),
                    bytes: meta.len(),
                    duration_secs,
                    still_path: still.display().to_string(),
                    codec: "mjpeg+pcm_s16le".into(),
                    has_audio: true,
                    audio_device: Some(dev.clone()),
                    audio_mode: "dshow".into(),
                });
            }
            Err(e) => {
                // Device busy / open failed → silent path (acceptance: silent still works).
                eprintln!("media clip audio encode failed ({e}); falling back to silent");
                let _ = std::fs::remove_file(&out);
            }
        }
    }

    run_ffmpeg_clip(&ffmpeg, still, &out, &dur, None)?;
    let meta = std::fs::metadata(&out).map_err(|e| e.to_string())?;
    Ok(EncodeClipResult {
        ok: true,
        path: out.display().to_string(),
        bytes: meta.len(),
        duration_secs,
        still_path: still.display().to_string(),
        codec: "mjpeg".into(),
        has_audio: false,
        audio_device: None,
        audio_mode: "silent".into(),
    })
}

fn run_ffmpeg_clip(
    ffmpeg: &Path,
    still: &Path,
    out: &Path,
    duration: &str,
    dshow_audio: Option<&str>,
) -> Result<(), String> {
    let mut cmd = crate::util::hidden_command(ffmpeg);
    cmd.args(["-y", "-loop", "1", "-framerate", "2", "-i"])
        .arg(still.as_os_str());
    if let Some(dev) = dshow_audio {
        // DirectShow audio input (pinned LGPL has dshow; no wasapi demuxer).
        let input = format!("audio={dev}");
        cmd.args(["-f", "dshow", "-i", &input]);
        cmd.args([
            "-t",
            duration,
            "-c:v",
            "mjpeg",
            "-q:v",
            "5",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ]);
    } else {
        cmd.args(["-t", duration, "-c:v", "mjpeg", "-q:v", "5", "-an"]);
    }
    cmd.arg(out.as_os_str());
    let status = cmd
        .status()
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg failed with {status}"));
    }
    Ok(())
}

/// Pick a DirectShow audio capture device (explicit name or auto).
fn resolve_dshow_audio_device(ffmpeg: &Path, explicit: Option<&str>) -> Option<String> {
    if let Some(name) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(name.to_string());
    }
    let devices = list_dshow_audio_devices(ffmpeg).unwrap_or_default();
    pick_dshow_audio_device(&devices)
}

/// Parse `ffmpeg -list_devices true -f dshow -i dummy` stderr for `(audio)` lines.
pub fn parse_dshow_audio_devices(ffmpeg_stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in ffmpeg_stderr.lines() {
        // Example: [dshow @ ...] "Microphone (Realtek)" (audio)
        let Some(q0) = line.find('"') else {
            continue;
        };
        let rest = &line[q0 + 1..];
        let Some(q1) = rest.find('"') else {
            continue;
        };
        let name = &rest[..q1];
        let after = rest[q1 + 1..].to_ascii_lowercase();
        if after.contains("(audio)") {
            out.push(name.to_string());
        }
    }
    out
}

fn list_dshow_audio_devices(ffmpeg: &Path) -> Result<Vec<String>, String> {
    let output = crate::util::hidden_command(ffmpeg)
        .args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            "dshow",
            "-i",
            "dummy",
        ])
        .output()
        .map_err(|e| format!("list dshow devices: {e}"))?;
    // dshow always "fails" opening dummy; names are on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(parse_dshow_audio_devices(&stderr))
}

/// Prefer desktop-loopback-ish names, then real mics, then first device.
/// Deprioritize virtual/cable noise sources when better options exist.
pub fn pick_dshow_audio_device(devices: &[String]) -> Option<String> {
    if devices.is_empty() {
        return None;
    }
    let score = |name: &str| -> i32 {
        let n = name.to_ascii_lowercase();
        let mut s = 0i32;
        if n.contains("stereo mix")
            || n.contains("what u hear")
            || n.contains("wave out mix")
            || n.contains("loopback")
            || n.contains("desktop")
        {
            s += 100;
        }
        if n.contains("microphone") || n.contains("mic ") || n.starts_with("mic") {
            s += 50;
        }
        if n.contains("line ") || n.contains("line (") || n.contains("audiobox") {
            s += 40;
        }
        if n.contains("headset") || n.contains("headphone") {
            s += 30;
        }
        if n.contains("virtual") || n.contains("voicemod") || n.contains("cable") {
            s -= 20;
        }
        s
    };
    devices
        .iter()
        .max_by_key(|d| score(d))
        .cloned()
}

fn write_http(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn ffmpeg_pin_constants_are_set() {
        assert!(!FFMPEG_TAG.is_empty());
        assert!(FFMPEG_URL.contains("lgpl"));
        assert_eq!(FFMPEG_SHA256.len(), 64);
        assert!(FFMPEG_SIZE > 1_000_000);
    }

    /// Live: download pinned LGPL FFmpeg if missing. Run with:
    /// `cargo test -p swerve-build --lib media_worker::tests::live_install_ffmpeg_pin -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_install_ffmpeg_pin() {
        let msg = install_ffmpeg().expect("install_ffmpeg");
        eprintln!("{msg}");
        assert!(ffmpeg_pinned_installed(), "pin missing after install");
        let (path, source) = resolve_ffmpeg_no_install().expect("resolve after install");
        assert_eq!(source, "pin");
        assert!(path.is_file());
        let st = crate::util::hidden_command(&path)
            .args(["-version"])
            .output()
            .expect("ffmpeg -version");
        assert!(st.status.success());
        let ver = String::from_utf8_lossy(&st.stdout);
        eprintln!("ffmpeg -version head: {}", ver.lines().next().unwrap_or(""));
        assert!(ver.to_lowercase().contains("ffmpeg"), "unexpected -version output");
    }

    #[test]
    fn health_body_serializes() {
        let h = HealthBody {
            ok: true,
            protocol: 1,
            version: "0.0.0".into(),
            pid: 1,
            uptime_ms: 10,
            role: "media-worker".into(),
            capabilities: vec![],
        };
        let s = serde_json::to_string(&h).unwrap();
        assert!(s.contains("media-worker"));
        assert!(s.contains("\"ok\":true"));
    }

    #[test]
    fn ready_prefix_stable() {
        assert!(READY_PREFIX.starts_with("SWERVE_MEDIA_READY"));
    }

    #[test]
    fn parse_dshow_audio_devices_extracts_audio_only() {
        let sample = r#"
[dshow @ 0] "Camera (NVIDIA Broadcast)" (video)
[dshow @ 0] "Microphone (Realtek(R) Audio)" (audio)
[dshow @ 0] "Line (AudioBox Go)" (audio)
[dshow @ 0] "Meld Studio Virtual Camera" (none)
"#;
        let devs = parse_dshow_audio_devices(sample);
        assert_eq!(
            devs,
            vec![
                "Microphone (Realtek(R) Audio)".to_string(),
                "Line (AudioBox Go)".to_string(),
            ]
        );
    }

    #[test]
    fn pick_dshow_prefers_loopback_then_mic_over_virtual() {
        let devices = vec![
            "CABLE Output (VB-Audio Virtual Cable)".into(),
            "Microphone (Voicemod Virtual Audio Device (WDM))".into(),
            "Microphone (Realtek(R) Audio)".into(),
        ];
        let pick = pick_dshow_audio_device(&devices).unwrap();
        assert_eq!(pick, "Microphone (Realtek(R) Audio)");

        let with_mix = vec![
            "Microphone (Realtek(R) Audio)".into(),
            "Stereo Mix (Realtek(R) Audio)".into(),
        ];
        assert_eq!(
            pick_dshow_audio_device(&with_mix).unwrap(),
            "Stereo Mix (Realtek(R) Audio)"
        );

        assert!(pick_dshow_audio_device(&[]).is_none());
    }

    #[test]
    fn encode_clip_result_defaults_audio_fields() {
        let raw = r#"{"ok":true,"path":"x.avi","bytes":1,"durationSecs":2.0,"stillPath":"s.png","codec":"mjpeg"}"#;
        let r: EncodeClipResult = serde_json::from_str(raw).unwrap();
        assert!(!r.has_audio);
        assert!(r.audio_device.is_none());
        assert_eq!(r.audio_mode, "silent");
    }

    #[test]
    fn copy_to_project_rejects_traversal_ids() {
        assert!(copy_to_project("x.png", "../evil").is_err());
        assert!(copy_to_project("x.png", "a/b").is_err());
        assert!(copy_to_project("x.png", "").is_err());
    }

    #[test]
    fn agent_status_report_has_capabilities() {
        let v = agent_status_report();
        let caps = v.get("capabilities").and_then(|c| c.as_array()).unwrap();
        assert!(caps.iter().any(|c| c.as_str() == Some("clip_audio")));
        assert!(v.get("ffmpeg").is_some());
    }

    /// Live: still + auto dshow audio (or silent fallback). Run with:
    /// `cargo test -p swerve-build --lib media_worker::tests::live_encode_clip_audio -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_encode_clip_audio() {
        let ffmpeg = ensure_ffmpeg().expect("ffmpeg");
        let dir = std::env::temp_dir().join("swerve_s28_audio_smoke");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let still = dir.join("still.png");
        let st = crate::util::hidden_command(&ffmpeg)
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=320x240:d=1",
                "-frames:v",
                "1",
            ])
            .arg(&still)
            .status()
            .expect("make still");
        assert!(st.success());

        let with_audio =
            encode_still_mjpeg_clip(&still, &dir, 2.0, "auto", None).expect("auto encode");
        eprintln!(
            "auto: has_audio={} mode={} device={:?} path={} bytes={}",
            with_audio.has_audio,
            with_audio.audio_mode,
            with_audio.audio_device,
            with_audio.path,
            with_audio.bytes
        );
        assert!(PathBuf::from(&with_audio.path).is_file());

        let silent =
            encode_still_mjpeg_clip(&still, &dir, 2.0, "none", None).expect("silent encode");
        assert!(!silent.has_audio);
        assert_eq!(silent.audio_mode, "silent");
        assert!(PathBuf::from(&silent.path).is_file());
        eprintln!("silent ok bytes={}", silent.bytes);
    }
}
