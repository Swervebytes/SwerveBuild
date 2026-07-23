//! Media worker process shell (S24 / Step 7 start).
//!
//! Separate process so future capture/encode crashes never take down chat.
//! This session: **health + supervise only** — no capture, encode, or stream.
//!
//! IPC: loopback HTTP, versioned path `/v1/*`, Bearer token (same idea as
//! terminal control server). Endpoint published under data dir.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
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
    let mut child = crate::util::hidden_command(&bin)
        .arg("--token")
        .arg(&token)
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
            "Worker not running. Call media_worker_start (shell only — no capture yet)."
                .into()
        } else if healthy {
            format!(
                "Media worker up (protocol v{PROTOCOL_VERSION}). Capabilities empty until capture lands."
            )
        } else {
            "Worker process present but health check failed.".into()
        },
    }
}

/// Kill worker on app exit.
pub fn shutdown_on_exit() {
    let _ = stop();
}

// ---------------------------------------------------------------------------
// Worker process entry (used by bin/swervebuild_media.rs)
// ---------------------------------------------------------------------------

static WORKER_START: Mutex<Option<Instant>> = Mutex::new(None);

/// Run the media worker main loop (blocks). Args: `--token <uuid>`.
pub fn worker_main(args: &[String]) -> i32 {
    let mut token = None::<String>;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--token" if i + 1 < args.len() => {
                token = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                eprintln!("unknown arg: {other}");
                return 2;
            }
        }
    }
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        eprintln!("usage: swervebuild-media --token <uuid>");
        return 2;
    };

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
    });
    println!("{READY_PREFIX}{ready}");
    let _ = std::io::stdout().flush();

    let running = AtomicBool::new(true);
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let tok = token.clone();
                thread::spawn(move || handle_client(stream, &tok));
            }
            Err(e) => {
                eprintln!("accept: {e}");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    0
}

fn handle_client(mut stream: TcpStream, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok().unwrap_or(0) == 0 {
        return;
    }
    let mut auth = None::<String>;
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
            capabilities: vec![],
        };
        let body = serde_json::to_string(&health).unwrap_or_else(|_| "{}".into());
        let _ = write_http(&mut stream, 200, "application/json", &body);
        return;
    }

    if method == "POST" && path == "/v1/shutdown" {
        let body = r#"{"ok":true}"#;
        let _ = write_http(&mut stream, 200, "application/json", body);
        // Exit process shortly after reply.
        thread::spawn(|| {
            thread::sleep(Duration::from_millis(50));
            std::process::exit(0);
        });
        return;
    }

    let body = r#"{"ok":false,"error":"not_found"}"#;
    let _ = write_http(&mut stream, 404, "application/json", body);
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
}
