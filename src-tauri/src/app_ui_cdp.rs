//! Minimal Chrome DevTools Protocol client for the main WebView2.
//!
//! Connects to the localhost remote-debugging port published by the running
//! SwerveBuild process. Used by MCP `app_ui_*` drive tools (out-of-process).

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_secs(8);

/// One entry from `GET /json/list` (WebView2 / Chromium debugger).
#[derive(Debug, Clone)]
pub struct CdpTarget {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub title: String,
    pub url: String,
    pub web_socket_debugger_url: String,
    pub target_type: String,
}

/// HTTP GET against the CDP discovery HTTP server (not the page WebSocket).
///
/// WebView2's debugger often keeps the socket open after the response (ignores
/// `Connection: close`). Do **not** `read_to_end` — parse `Content-Length` and
/// stop once the body is complete (otherwise probes hang until read timeout).
pub fn http_get(host: &str, port: u16, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect((host, port)).map_err(|e| {
        format!("CDP HTTP connect {host}:{port} failed: {e}. Is SwerveBuild running with CDP enabled?")
    })?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("CDP HTTP write: {e}"))?;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => {
                // Peer closed — accept whatever body we have after headers.
                return take_http_body(&buf, true).ok_or_else(|| {
                    format!("CDP HTTP: incomplete response ({} bytes)", buf.len())
                });
            }
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(body) = take_http_body(&buf, false) {
                    return Ok(body);
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Timed out — only succeed if Content-Length body is already complete.
                if let Some(body) = take_http_body(&buf, false) {
                    return Ok(body);
                }
                return Err(format!("CDP HTTP read: {e}"));
            }
            Err(e) => return Err(format!("CDP HTTP read: {e}")),
        }
        if buf.len() > 8 * 1024 * 1024 {
            return Err("CDP HTTP response too large".into());
        }
    }
}

/// If `buf` contains a full HTTP response, return the body as a String.
/// When `peer_closed` is false, only complete `Content-Length` responses return.
fn take_http_body(buf: &[u8], peer_closed: bool) -> Option<String> {
    let raw = String::from_utf8_lossy(buf);
    let header_end = raw
        .find("\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| raw.find("\n\n").map(|i| i + 2))?;
    let headers = &raw[..header_end];
    let body_bytes = &buf[header_end..];
    let content_len = headers.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        lower
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    if let Some(len) = content_len {
        if body_bytes.len() >= len {
            return Some(String::from_utf8_lossy(&body_bytes[..len]).into_owned());
        }
        return None;
    }
    if peer_closed {
        return Some(String::from_utf8_lossy(body_bytes).into_owned());
    }
    None
}

/// List debugger targets. Prefers page-type targets with a websocket URL.
pub fn list_targets(host: &str, port: u16) -> Result<Vec<CdpTarget>, String> {
    // WebView2 accepts both /json and /json/list.
    let body = http_get(host, port, "/json/list")
        .or_else(|_| http_get(host, port, "/json"))?;
    let arr: Value = serde_json::from_str(&body)
        .map_err(|e| format!("CDP /json parse: {e}; body starts: {}", trunc(&body, 120)))?;
    let list = arr
        .as_array()
        .ok_or_else(|| "CDP /json: expected array".to_string())?;
    let mut out = Vec::new();
    for item in list {
        let ws = item
            .get("webSocketDebuggerUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if ws.is_empty() {
            continue;
        }
        out.push(CdpTarget {
            id: item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            title: item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            url: item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            web_socket_debugger_url: ws,
            target_type: item
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(out)
}

/// URL prefixes the SwerveBuild shell can load at (prod WebView2 origin forms +
/// the vite dev server). Used to prefer the MAIN window now that the process
/// hosts a second webview (the S12 debug pane).
const SHELL_ORIGINS: [&str; 4] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    "http://localhost:1420",
];

/// Pick the main app page target. With the S12 debug pane, the process exposes
/// TWO page targets, and /json list order is unspecified — so prefer a known
/// shell origin, and always exclude the debug pane (by its cached target id and
/// its marker creation URL) so `app_ui_*` can never drive the pane.
pub fn pick_main_target<'a>(
    targets: &'a [CdpTarget],
    exclude_id: Option<&str>,
) -> Result<&'a CdpTarget, String> {
    let usable = |t: &&CdpTarget| {
        !t.web_socket_debugger_url.is_empty()
            && !t.url.starts_with("devtools://")
            && Some(t.id.as_str()) != exclude_id
            && !t.url.starts_with(crate::browser_debug::DEBUG_PANE_INITIAL_URL)
    };
    let pages = || targets.iter().filter(|t| t.target_type == "page").filter(usable);
    pages()
        .find(|t| SHELL_ORIGINS.iter().any(|o| t.url.starts_with(o)))
        .or_else(|| pages().next())
        .or_else(|| targets.iter().find(usable))
        .ok_or_else(|| {
            "CDP: no page target found. Wait for the main window to finish loading.".into()
        })
}

type CdpSocket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

fn connect_ws(ws_url: &str) -> Result<CdpSocket, String> {
    let (mut socket, _resp) = tungstenite::connect(ws_url)
        .map_err(|e| format!("CDP WebSocket connect failed: {e}"))?;

    // Best-effort timeouts on the underlying TCP stream (plain localhost).
    {
        use tungstenite::stream::MaybeTlsStream;
        if let MaybeTlsStream::Plain(tcp) = socket.get_mut() {
            let _ = tcp.set_read_timeout(Some(IO_TIMEOUT));
            let _ = tcp.set_write_timeout(Some(IO_TIMEOUT));
        }
    }
    Ok(socket)
}

/// Send one CDP command and wait for the matching id response.
pub fn cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value, String> {
    let mut socket = connect_ws(ws_url)?;
    send_and_wait(&mut socket, 1, method, params)
}

/// Send several CDP commands over ONE WebSocket connection, awaiting each
/// response in order. Required for paired events (keyDown + keyUp).
pub fn cdp_call_many(ws_url: &str, calls: &[(&str, Value)]) -> Result<Vec<Value>, String> {
    let mut socket = connect_ws(ws_url)?;
    let mut out = Vec::with_capacity(calls.len());
    for (i, (method, params)) in calls.iter().enumerate() {
        out.push(send_and_wait(
            &mut socket,
            (i + 1) as u64,
            method,
            params.clone(),
        )?);
    }
    Ok(out)
}

fn send_and_wait(
    socket: &mut CdpSocket,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let msg = json!({ "id": id, "method": method, "params": params });
    let text = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
    socket
        .send(tungstenite::Message::Text(text.into()))
        .map_err(|e| format!("CDP send {method}: {e}"))?;

    let deadline = std::time::Instant::now() + IO_TIMEOUT;
    loop {
        if std::time::Instant::now() > deadline {
            return Err(format!("CDP timeout waiting for {method} response"));
        }
        let msg = socket
            .read()
            .map_err(|e| format!("CDP read {method}: {e}"))?;
        match msg {
            tungstenite::Message::Text(t) => {
                let v: Value = serde_json::from_str(&t)
                    .map_err(|e| format!("CDP response JSON: {e}"))?;
                if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                    if let Some(err) = v.get("error") {
                        return Err(format!(
                            "CDP {method} error: {}",
                            err.get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or(&err.to_string())
                        ));
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
                // Event — ignore and continue.
            }
            tungstenite::Message::Ping(p) => {
                let _ = socket.send(tungstenite::Message::Pong(p));
            }
            tungstenite::Message::Close(_) => {
                return Err(format!("CDP WebSocket closed while waiting for {method}"));
            }
            _ => {}
        }
    }
}

/// Capture a PNG of the page via `Page.captureScreenshot`.
pub fn capture_screenshot_png(ws_url: &str) -> Result<Vec<u8>, String> {
    let result = cdp_call(
        ws_url,
        "Page.captureScreenshot",
        json!({ "format": "png", "fromSurface": true }),
    )?;
    let b64 = result
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "CDP Page.captureScreenshot: missing data".to_string())?;
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("CDP screenshot base64 decode: {e}"))
}

fn unwrap_evaluate(result: Value) -> Result<Value, String> {
    if result.get("exceptionDetails").is_some() {
        let text = result
            .pointer("/exceptionDetails/text")
            .and_then(|v| v.as_str())
            .unwrap_or("JS exception");
        let desc = result
            .pointer("/exceptionDetails/exception/description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(format!("CDP evaluate exception: {text} {desc}").trim().into());
    }
    Ok(result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}

/// Run a JS expression and return the remote object value (Runtime.evaluate).
pub fn evaluate(ws_url: &str, expression: &str) -> Result<Value, String> {
    let result = cdp_call(
        ws_url,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )?;
    unwrap_evaluate(result)
}

/// A HELD DevTools session (one WebSocket kept open across calls).
///
/// Needed when session-scoped state must survive between commands:
/// `Page.addScriptToEvaluateOnNewDocument` registrations are removed when the
/// registering session detaches, so register-then-navigate must happen on ONE
/// session — per-call sockets silently lose the script before navigation.
pub struct CdpSession {
    socket: CdpSocket,
    next_id: u64,
}

impl CdpSession {
    pub fn connect(ws_url: &str) -> Result<Self, String> {
        Ok(CdpSession { socket: connect_ws(ws_url)?, next_id: 0 })
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        send_and_wait(&mut self.socket, self.next_id, method, params)
    }

    pub fn evaluate(&mut self, expression: &str) -> Result<Value, String> {
        let result = self.call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )?;
        unwrap_evaluate(result)
    }
}

/// Probe whether the CDP HTTP endpoint answers (drive ready).
pub fn probe(host: &str, port: u16) -> bool {
    http_get(host, port, "/json/version")
        .or_else(|_| http_get(host, port, "/json/list"))
        .or_else(|_| http_get(host, port, "/json"))
        .is_ok()
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_http_body_content_length_complete() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]";
        assert_eq!(take_http_body(raw, false).as_deref(), Some("[]"));
    }

    #[test]
    fn take_http_body_waits_for_full_content_length() {
        // Body shorter than Content-Length is incomplete — even on close
        // (a truncated response must error, not silently return a prefix).
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n[]";
        assert_eq!(take_http_body(raw, false), None);
        assert_eq!(take_http_body(raw, true), None);
    }

    #[test]
    fn take_http_body_no_content_length_needs_close() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\n{\"a\":1}";
        assert_eq!(take_http_body(raw, false), None);
        assert_eq!(take_http_body(raw, true).as_deref(), Some("{\"a\":1}"));
    }

    fn target(id: &str, url: &str) -> CdpTarget {
        CdpTarget {
            id: id.into(),
            title: String::new(),
            url: url.into(),
            web_socket_debugger_url: format!("ws://127.0.0.1/{id}"),
            target_type: "page".into(),
        }
    }

    #[test]
    fn pick_main_prefers_page() {
        let mut devtools = target("1", "devtools://x");
        devtools.title = "DevTools".into();
        let targets = vec![devtools, target("2", "http://localhost:1420/")];
        let t = pick_main_target(&targets, None).unwrap();
        assert_eq!(t.id, "2");
    }

    #[test]
    fn pick_main_prefers_shell_origin_over_debug_pane_listed_first() {
        // The pane navigated to some arbitrary localhost app AND is listed
        // first — the shell origin must still win.
        let targets = vec![
            target("pane", "http://localhost:5500/app/"),
            target("main", "tauri://localhost/settings"),
        ];
        let t = pick_main_target(&targets, None).unwrap();
        assert_eq!(t.id, "main");
    }

    #[test]
    fn pick_main_excludes_debug_pane_by_id_and_marker_url() {
        // By cached id, even when no shell-origin candidate exists (dev
        // edge: main not loaded yet).
        let targets = vec![
            target("pane", "http://localhost:5500/app/"),
            target("other", "http://localhost:9000/x"),
        ];
        let t = pick_main_target(&targets, Some("pane")).unwrap();
        assert_eq!(t.id, "other");
        // By the creation-marker URL with no cache at all.
        let targets = vec![
            target("pane", "about:blank#swerve-debug-pane"),
            target("main", "http://localhost:1420/"),
        ];
        let t = pick_main_target(&targets, None).unwrap();
        assert_eq!(t.id, "main");
    }

    #[test]
    fn pick_main_errors_when_only_the_pane_exists() {
        let targets = vec![target("pane", "about:blank#swerve-debug-pane")];
        assert!(pick_main_target(&targets, Some("pane")).is_err());
    }
}
