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

/// Pick the main app page target (prefer type=page, non-devtools URL).
pub fn pick_main_target(targets: &[CdpTarget]) -> Result<&CdpTarget, String> {
    let page = targets.iter().find(|t| {
        t.target_type == "page"
            && !t.url.starts_with("devtools://")
            && !t.web_socket_debugger_url.is_empty()
    });
    if let Some(t) = page {
        return Ok(t);
    }
    targets
        .iter()
        .find(|t| !t.web_socket_debugger_url.is_empty() && !t.url.starts_with("devtools://"))
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
    if result
        .get("exceptionDetails")
        .is_some()
    {
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

    #[test]
    fn pick_main_prefers_page() {
        let targets = vec![
            CdpTarget {
                id: "1".into(),
                title: "DevTools".into(),
                url: "devtools://x".into(),
                web_socket_debugger_url: "ws://127.0.0.1/1".into(),
                target_type: "page".into(),
            },
            CdpTarget {
                id: "2".into(),
                title: "Swerve".into(),
                url: "http://localhost:1420/".into(),
                web_socket_debugger_url: "ws://127.0.0.1/2".into(),
                target_type: "page".into(),
            },
        ];
        let t = pick_main_target(&targets).unwrap();
        assert_eq!(t.id, "2");
    }
}
