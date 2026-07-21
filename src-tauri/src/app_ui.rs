//! App UI MCP surface (Roadmap Step 6) — grant, published state, CDP drive.
//!
//! Safety: all drive tools require the human Settings grant. CDP attaches to
//! the main WebView2 via a localhost remote-debugging port enabled at app
//! start (see [`prepare_webview_cdp`]).

use crate::app_ui_cdp;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const GRANT_FILE: &str = "app_ui_grant.json";
const STATE_FILE: &str = "app_ui_state.json";
const CDP_FILE: &str = "app_ui_cdp.json";
const ARTIFACTS_SUBDIR: &str = "app_ui_artifacts";

/// Soft cap: screenshots per rolling window (disk + token protection).
const SCREENSHOT_MAX_PER_WINDOW: u32 = 6;
const SCREENSHOT_WINDOW: Duration = Duration::from_secs(60);
/// Max PNG artifacts kept under the data dir (oldest deleted).
const ARTIFACT_KEEP: usize = 30;

/// Max characters `app_ui_type` accepts in one call.
const TYPE_TEXT_MAX: usize = 4_000;
/// `app_ui_wait` timeout bounds + poll cadence.
const WAIT_TIMEOUT_DEFAULT_MS: u64 = 5_000;
const WAIT_TIMEOUT_MAX_MS: u64 = 15_000;
const WAIT_POLL_MS: u64 = 250;

static SCREENSHOT_RATE: Mutex<ScreenshotRate> = Mutex::new(ScreenshotRate {
    window_start: None,
    count: 0,
});

struct ScreenshotRate {
    window_start: Option<Instant>,
    count: u32,
}

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

/// Localhost CDP endpoint published by the running desktop process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUiCdpEndpoint {
    pub host: String,
    pub port: u16,
    pub pid: u32,
    #[serde(default)]
    pub updated_at: String,
}

fn grant_path() -> PathBuf {
    crate::paths::data_dir().join(GRANT_FILE)
}

fn state_path() -> PathBuf {
    crate::paths::data_dir().join(STATE_FILE)
}

fn cdp_path() -> PathBuf {
    crate::paths::data_dir().join(CDP_FILE)
}

fn artifacts_dir() -> PathBuf {
    crate::paths::data_dir().join(ARTIFACTS_SUBDIR)
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

pub fn load_cdp_endpoint() -> Option<AppUiCdpEndpoint> {
    let path = cdp_path();
    if !path.is_file() {
        return None;
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn publish_cdp_endpoint(port: u16) -> Result<AppUiCdpEndpoint, String> {
    let ep = AppUiCdpEndpoint {
        host: "127.0.0.1".into(),
        port,
        pid: std::process::id(),
        updated_at: crate::store::Store::now(),
    };
    let path = cdp_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(&ep).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())?;
    Ok(ep)
}

/// Whether the published CDP endpoint answers (app is up with debugging).
pub fn drive_ready() -> bool {
    let Some(ep) = load_cdp_endpoint() else {
        return false;
    };
    app_ui_cdp::probe(&ep.host, ep.port)
}

/// Pick a free localhost port, set WebView2 browser args for remote debugging,
/// and publish the endpoint file. Call **before** the WebView is created.
///
/// On non-Windows targets this is a no-op (returns Ok(0)).
pub fn prepare_webview_cdp() -> Result<u16, String> {
    #[cfg(windows)]
    {
        let port = free_localhost_port()?;
        // Env overrides CoreWebView2EnvironmentOptions::AdditionalBrowserArguments,
        // so re-include wry's default --disable-features list.
        let args = format!(
            "--remote-debugging-port={port} --remote-allow-origins=* --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection"
        );
        // Once at process start, before any WebView threads exist.
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", &args);
        publish_cdp_endpoint(port)?;
        Ok(port)
    }
    #[cfg(not(windows))]
    {
        Ok(0)
    }
}

fn free_localhost_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("bind ephemeral port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

fn require_drive() -> Result<AppUiCdpEndpoint, String> {
    require_grant()?;
    let ep = load_cdp_endpoint().ok_or_else(|| {
        "CDP endpoint not published. Restart SwerveBuild so it enables WebView2 remote debugging."
            .to_string()
    })?;
    if !app_ui_cdp::probe(&ep.host, ep.port) {
        return Err(format!(
            "CDP not reachable at {}:{} (pid file claims {}). Is this SwerveBuild instance running with S08+ CDP?",
            ep.host, ep.port, ep.pid
        ));
    }
    Ok(ep)
}

fn connect_main_page(ep: &AppUiCdpEndpoint) -> Result<String, String> {
    let targets = app_ui_cdp::list_targets(&ep.host, ep.port)?;
    let t = app_ui_cdp::pick_main_target(&targets)?;
    Ok(t.web_socket_debugger_url.clone())
}

fn check_screenshot_rate() -> Result<(), String> {
    let mut g = SCREENSHOT_RATE
        .lock()
        .map_err(|_| "screenshot rate lock poisoned".to_string())?;
    let now = Instant::now();
    match g.window_start {
        Some(start) if now.duration_since(start) < SCREENSHOT_WINDOW => {
            if g.count >= SCREENSHOT_MAX_PER_WINDOW {
                return Err(format!(
                    "app_ui_screenshot rate limit: max {SCREENSHOT_MAX_PER_WINDOW} per {}s",
                    SCREENSHOT_WINDOW.as_secs()
                ));
            }
            g.count += 1;
        }
        _ => {
            g.window_start = Some(now);
            g.count = 1;
        }
    }
    Ok(())
}

fn prune_artifacts(dir: &PathBuf) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("png"))
                .unwrap_or(false)
        })
        .collect();
    if files.len() <= ARTIFACT_KEEP {
        return;
    }
    files.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .ok()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    let drop_n = files.len() - ARTIFACT_KEEP;
    for e in files.into_iter().take(drop_n) {
        let _ = fs::remove_file(e.path());
    }
}

/// Capture the main WebView as PNG. Returns artifact id + absolute path.
pub fn screenshot() -> Result<Value, String> {
    check_screenshot_rate()?;
    let ep = require_drive()?;
    let ws = connect_main_page(&ep)?;
    let png = app_ui_cdp::capture_screenshot_png(&ws)?;
    if png.is_empty() {
        return Err("CDP screenshot returned empty PNG".into());
    }

    let dir = artifacts_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let path = dir.join(format!("{id}.png"));
    fs::write(&path, &png).map_err(|e| format!("write screenshot: {e}"))?;
    prune_artifacts(&dir);

    Ok(json!({
        "id": id,
        "path": path.to_string_lossy(),
        "bytes": png.len(),
        "format": "png",
        "via": "cdp",
        "cdpPort": ep.port,
    }))
}

/// Encode a Rust string as a JS string literal (JSON escaping is valid JS).
fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Shared selector normalization: bare test ids become `[data-testid="…"]`.
/// No denylist — read-only waits may target anything.
fn normalize_selector_base(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("selector required".into());
    }
    if s.len() > 200 {
        return Err("selector too long (max 200)".into());
    }
    let lower = s.to_ascii_lowercase();
    if lower.contains("javascript:") || s.contains("</") || s.contains("{{") {
        return Err("selector rejected (unsafe pattern)".into());
    }

    // Bare token / testid → data-testid attribute selector.
    let is_bare = !s.contains('[')
        && !s.contains('#')
        && !s.contains('.')
        && !s.contains(' ')
        && !s.contains('>')
        && !s.contains(':');
    if is_bare {
        // Escape double quotes in attribute value.
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        return Ok(format!("[data-testid=\"{escaped}\"]"));
    }
    Ok(s.to_string())
}

/// Normalize an interaction target (click/type): base rules + soft denylist
/// for destructive / live-safety controls.
pub fn normalize_click_selector(raw: &str) -> Result<String, String> {
    let s = normalize_selector_base(raw)?;
    let lower = s.to_ascii_lowercase();
    for deny in [
        "go-live",
        "broadcast",
        "stream-start",
        "approve-all",
        "permission-allow-always",
    ] {
        if lower.contains(deny) {
            return Err(format!(
                "selector denied by app_ui denylist ({deny}). Click that control manually."
            ));
        }
    }
    Ok(s)
}

/// Click a control via CSS selector (or bare `data-testid` token).
pub fn click(selector: &str) -> Result<Value, String> {
    let sel = normalize_click_selector(selector)?;
    let ep = require_drive()?;
    let ws = connect_main_page(&ep)?;

    // Escape for embedding in a single-quoted JS string.
    let js_sel = sel.replace('\\', "\\\\").replace('\'', "\\'");
    let expression = format!(
        r#"(function() {{
  var el = document.querySelector('{js_sel}');
  if (!el) return {{ ok: false, error: 'not_found', selector: '{js_sel}' }};
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  if (typeof el.click === 'function') {{ el.click(); }}
  else {{
    el.dispatchEvent(new MouseEvent('click', {{ bubbles: true, cancelable: true, view: window }}));
  }}
  return {{
    ok: true,
    selector: '{js_sel}',
    tag: el.tagName || '',
    testId: el.getAttribute('data-testid') || '',
    text: (el.innerText || el.textContent || '').trim().slice(0, 80)
  }};
}})()"#
    );

    let value = app_ui_cdp::evaluate(&ws, &expression)?;
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("click failed");
        return Err(format!("app_ui_click: {err} (selector={sel})"));
    }
    Ok(json!({
        "ok": true,
        "selector": sel,
        "result": value,
        "via": "cdp",
        "cdpPort": ep.port,
    }))
}

/// Fill an input / textarea / contenteditable via CDP `Runtime.evaluate`.
///
/// Uses the native value setter + bubbling `input`/`change` events so
/// framework bindings (Svelte `bind:value`, React controlled inputs) see the
/// change exactly like a human edit.
pub fn type_text(selector: &str, text: &str, clear: bool) -> Result<Value, String> {
    if text.chars().count() > TYPE_TEXT_MAX {
        return Err(format!(
            "app_ui_type: text too long (max {TYPE_TEXT_MAX} chars)"
        ));
    }
    let sel = normalize_click_selector(selector)?;
    let ep = require_drive()?;
    let ws = connect_main_page(&ep)?;

    let js_sel = js_string(&sel);
    let js_text = js_string(text);
    let js_clear = if clear { "true" } else { "false" };
    let expression = format!(
        r#"(function() {{
  var el = document.querySelector({js_sel});
  if (!el) return {{ ok: false, error: 'not_found', selector: {js_sel} }};
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  var text = {js_text};
  var clear = {js_clear};
  var tag = (el.tagName || '').toUpperCase();
  if (tag === 'INPUT' || tag === 'TEXTAREA') {{
    el.focus();
    var proto = tag === 'INPUT' ? window.HTMLInputElement.prototype : window.HTMLTextAreaElement.prototype;
    var desc = Object.getOwnPropertyDescriptor(proto, 'value');
    var next = clear ? text : (el.value || '') + text;
    if (desc && desc.set) {{ desc.set.call(el, next); }} else {{ el.value = next; }}
    el.dispatchEvent(new InputEvent('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
    return {{ ok: true, selector: {js_sel}, tag: tag,
             testId: el.getAttribute('data-testid') || '',
             valueLength: (el.value || '').length }};
  }}
  if (el.isContentEditable) {{
    el.focus();
    el.textContent = clear ? text : (el.textContent || '') + text;
    el.dispatchEvent(new InputEvent('input', {{ bubbles: true }}));
    return {{ ok: true, selector: {js_sel}, tag: tag, contentEditable: true,
             valueLength: (el.textContent || '').length }};
  }}
  return {{ ok: false, error: 'not_typable', selector: {js_sel}, tag: tag }};
}})()"#
    );

    let value = app_ui_cdp::evaluate(&ws, &expression)?;
    if value.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("type failed");
        return Err(format!("app_ui_type: {err} (selector={sel})"));
    }
    Ok(json!({
        "ok": true,
        "selector": sel,
        "result": value,
        "via": "cdp",
        "cdpPort": ep.port,
    }))
}

/// One parsed `app_ui_press` key: CDP `Input.dispatchKeyEvent` fields.
#[derive(Debug, Clone, PartialEq)]
pub struct KeySpec {
    pub key: String,
    pub code: String,
    pub vk: u32,
    /// Committed text for keys that insert (Enter, Space, printable chars).
    pub text: Option<String>,
    /// CDP modifier bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8.
    pub modifiers: u32,
}

/// Parse `"Enter"`, `"Ctrl+Enter"`, `"a"`, … into a [`KeySpec`].
pub fn parse_key(raw: &str) -> Result<KeySpec, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("key required".into());
    }
    if s.len() > 40 {
        return Err("key too long".into());
    }
    let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("key required".into());
    }
    let (mods, key_part) = parts.split_at(parts.len() - 1);
    let mut modifiers = 0u32;
    for m in mods {
        modifiers |= match m.to_ascii_lowercase().as_str() {
            "alt" => 1,
            "ctrl" | "control" => 2,
            "meta" | "cmd" | "win" => 4,
            "shift" => 8,
            other => return Err(format!("unknown modifier: {other} (Ctrl/Alt/Shift/Meta)")),
        };
    }

    let k = key_part[0];
    let named: Option<(&str, &str, u32, Option<&str>)> = match k.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(("Enter", "Enter", 13, Some("\r"))),
        "tab" => Some(("Tab", "Tab", 9, None)),
        "escape" | "esc" => Some(("Escape", "Escape", 27, None)),
        "backspace" => Some(("Backspace", "Backspace", 8, None)),
        "delete" | "del" => Some(("Delete", "Delete", 46, None)),
        "arrowup" | "up" => Some(("ArrowUp", "ArrowUp", 38, None)),
        "arrowdown" | "down" => Some(("ArrowDown", "ArrowDown", 40, None)),
        "arrowleft" | "left" => Some(("ArrowLeft", "ArrowLeft", 37, None)),
        "arrowright" | "right" => Some(("ArrowRight", "ArrowRight", 39, None)),
        "home" => Some(("Home", "Home", 36, None)),
        "end" => Some(("End", "End", 35, None)),
        "pageup" => Some(("PageUp", "PageUp", 33, None)),
        "pagedown" => Some(("PageDown", "PageDown", 34, None)),
        "space" | "spacebar" => Some((" ", "Space", 32, Some(" "))),
        _ => None,
    };
    if let Some((key, code, vk, text)) = named {
        return Ok(KeySpec {
            key: key.into(),
            code: code.into(),
            vk,
            text: text.map(Into::into),
            modifiers,
        });
    }

    // Single printable character.
    let chars: Vec<char> = k.chars().collect();
    if chars.len() == 1 && !chars[0].is_control() {
        let c = chars[0];
        let vk = if c.is_ascii_alphanumeric() {
            c.to_ascii_uppercase() as u32
        } else {
            0
        };
        let code = if c.is_ascii_alphabetic() {
            format!("Key{}", c.to_ascii_uppercase())
        } else if c.is_ascii_digit() {
            format!("Digit{c}")
        } else {
            String::new()
        };
        return Ok(KeySpec {
            key: c.to_string(),
            code,
            vk,
            text: Some(c.to_string()),
            modifiers,
        });
    }
    Err(format!(
        "unsupported key: {k}. Named keys: Enter, Tab, Escape, Backspace, Delete, \
         ArrowUp/Down/Left/Right, Home, End, PageUp, PageDown, Space — or one printable \
         character. Prefix modifiers with '+': Ctrl+Enter."
    ))
}

/// Send a trusted key press (keyDown + keyUp) to the focused element via CDP
/// `Input.dispatchKeyEvent`. Focus a field first with click/type.
pub fn press_key(raw_key: &str) -> Result<Value, String> {
    let spec = parse_key(raw_key)?;
    let ep = require_drive()?;
    let ws = connect_main_page(&ep)?;

    // Shortcut chords (Ctrl/Alt/Meta) must not commit text.
    let commit_text = if spec.modifiers & (1 | 2 | 4) != 0 {
        None
    } else {
        spec.text.clone()
    };
    let mut down = json!({
        "type": if commit_text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": spec.key,
        "code": spec.code,
        "windowsVirtualKeyCode": spec.vk,
        "nativeVirtualKeyCode": spec.vk,
        "modifiers": spec.modifiers,
    });
    if let Some(t) = &commit_text {
        down["text"] = json!(t);
        down["unmodifiedText"] = json!(t);
    }
    let up = json!({
        "type": "keyUp",
        "key": spec.key,
        "code": spec.code,
        "windowsVirtualKeyCode": spec.vk,
        "nativeVirtualKeyCode": spec.vk,
        "modifiers": spec.modifiers,
    });
    app_ui_cdp::cdp_call_many(
        &ws,
        &[("Input.dispatchKeyEvent", down), ("Input.dispatchKeyEvent", up)],
    )?;
    Ok(json!({
        "ok": true,
        "key": spec.key,
        "modifiers": spec.modifiers,
        "via": "cdp",
        "cdpPort": ep.port,
    }))
}

/// Parsed `app_ui_wait` condition.
#[derive(Debug, Clone, PartialEq)]
pub enum WaitCondition {
    /// CSS selector (or bare data-testid) present in the DOM.
    Selector(String),
    /// `!selector` — selector absent from the DOM.
    Gone(String),
    /// `route:/path` — SPA location.pathname equals or starts with path.
    Route(String),
    /// `text:needle` — document.body.innerText contains needle.
    Text(String),
}

/// Parse `"selector"`, `"!selector"`, `"route:/settings"`, `"text:Saved"`.
pub fn parse_wait_condition(raw: &str) -> Result<WaitCondition, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("condition required".into());
    }
    if s.len() > 300 {
        return Err("condition too long (max 300)".into());
    }
    if let Some(rest) = s.strip_prefix("route:") {
        let r = rest.trim();
        if r.is_empty() || !r.starts_with('/') {
            return Err("route condition must look like route:/settings".into());
        }
        return Ok(WaitCondition::Route(r.to_string()));
    }
    if let Some(rest) = s.strip_prefix("text:") {
        let t = rest.trim();
        if t.is_empty() {
            return Err("text condition requires a needle, e.g. text:Saved".into());
        }
        return Ok(WaitCondition::Text(t.to_string()));
    }
    if let Some(rest) = s.strip_prefix('!') {
        return Ok(WaitCondition::Gone(normalize_selector_base(rest)?));
    }
    Ok(WaitCondition::Selector(normalize_selector_base(s)?))
}

fn wait_probe_js(cond: &WaitCondition) -> String {
    match cond {
        WaitCondition::Selector(sel) => {
            format!("!!document.querySelector({})", js_string(sel))
        }
        WaitCondition::Gone(sel) => {
            format!("!document.querySelector({})", js_string(sel))
        }
        WaitCondition::Route(path) => {
            let p = js_string(path);
            format!("(location.pathname === {p} || location.pathname.startsWith({p}))")
        }
        WaitCondition::Text(needle) => {
            format!(
                "((document.body && document.body.innerText) || '').includes({})",
                js_string(needle)
            )
        }
    }
}

/// Poll a condition until it holds or the (capped) timeout elapses.
/// A timeout is a normal result (`matched: false, timedOut: true`), not an
/// error — agents branch on it.
pub fn wait_for(condition: &str, timeout_ms: Option<u64>) -> Result<Value, String> {
    let cond = parse_wait_condition(condition)?;
    let timeout = timeout_ms
        .unwrap_or(WAIT_TIMEOUT_DEFAULT_MS)
        .min(WAIT_TIMEOUT_MAX_MS);
    let ep = require_drive()?;
    let ws = connect_main_page(&ep)?;
    let expression = wait_probe_js(&cond);

    let start = Instant::now();
    loop {
        let hit = app_ui_cdp::evaluate(&ws, &expression)?.as_bool() == Some(true);
        let elapsed = start.elapsed().as_millis() as u64;
        if hit {
            return Ok(json!({
                "ok": true,
                "matched": true,
                "condition": condition,
                "elapsedMs": elapsed,
                "via": "cdp",
                "cdpPort": ep.port,
            }));
        }
        if elapsed >= timeout {
            return Ok(json!({
                "ok": false,
                "matched": false,
                "timedOut": true,
                "condition": condition,
                "elapsedMs": elapsed,
                "timeoutMs": timeout,
                "via": "cdp",
                "cdpPort": ep.port,
            }));
        }
        std::thread::sleep(Duration::from_millis(WAIT_POLL_MS));
    }
}

/// Structured payload for MCP `app_ui_state` (and snapshot text).
pub fn state_report() -> Value {
    let grant = load_grant();
    let published = load_published_state();
    let ep = load_cdp_endpoint();
    let ready = ep
        .as_ref()
        .map(|e| app_ui_cdp::probe(&e.host, e.port))
        .unwrap_or(false);

    let (shot, click_s, type_s, press_s, wait_s, drive_note) = if ready {
        (
            "available",
            "available (selector or bare data-testid)",
            "available (selector or bare data-testid + text; clear defaults true)",
            "available (Enter/Tab/Escape/arrows/… or one char; Ctrl+/Alt+/Shift+/Meta+ chords)",
            "available (selector, !selector, route:/path, text:needle)",
            "CDP attached to main WebView2 (localhost remote-debugging-port).",
        )
    } else if ep.is_some() {
        let s = "cdp_endpoint_published_but_unreachable";
        (
            s,
            s,
            s,
            s,
            s,
            "CDP endpoint file exists but port does not answer — start/restart SwerveBuild (S08+ binary).",
        )
    } else {
        let s = "not_ready";
        (
            s,
            s,
            s,
            s,
            s,
            "No CDP endpoint file — app must call prepare_webview_cdp at start.",
        )
    };

    json!({
        "granted": grant.granted,
        "grantUpdatedAt": grant.updated_at,
        "driveReady": ready,
        "driveNote": drive_note,
        "cdp": ep.as_ref().map(|e| json!({
            "host": e.host,
            "port": e.port,
            "pid": e.pid,
            "updatedAt": e.updated_at,
        })),
        "route": published.route,
        "title": published.title,
        "permissionModalOpen": published.permission_modal_open,
        "stateUpdatedAt": published.updated_at,
        "tools": {
            "app_ui_state": "available",
            "app_ui_snapshot": "available (text digest from published state)",
            "app_ui_screenshot": shot,
            "app_ui_click": click_s,
            "app_ui_type": type_s,
            "app_ui_press": press_s,
            "app_ui_wait": wait_s,
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
    fn state_report_includes_tools() {
        let _g = LOCK.lock().unwrap();
        let report = state_report();
        assert!(report.get("driveReady").and_then(|v| v.as_bool()).is_some());
        assert!(report.get("tools").is_some());
        assert!(report.get("tools").unwrap().get("app_ui_screenshot").is_some());
    }

    #[test]
    fn normalize_bare_testid() {
        let s = normalize_click_selector("app-ui-grant-on").unwrap();
        assert_eq!(s, "[data-testid=\"app-ui-grant-on\"]");
    }

    #[test]
    fn normalize_keeps_css() {
        let s = normalize_click_selector("[data-testid=\"x\"]").unwrap();
        assert_eq!(s, "[data-testid=\"x\"]");
    }

    #[test]
    fn normalize_denies_go_live() {
        assert!(normalize_click_selector("go-live-button").is_err());
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_click_selector("  ").is_err());
    }

    #[test]
    fn js_string_escapes_quotes_and_newlines() {
        assert_eq!(js_string("a\"b\nc"), r#""a\"b\nc""#);
    }

    #[test]
    fn parse_key_enter_commits_cr() {
        let k = parse_key("Enter").unwrap();
        assert_eq!(k.key, "Enter");
        assert_eq!(k.vk, 13);
        assert_eq!(k.text.as_deref(), Some("\r"));
        assert_eq!(k.modifiers, 0);
    }

    #[test]
    fn parse_key_ctrl_enter_modifier() {
        let k = parse_key("Ctrl+Enter").unwrap();
        assert_eq!(k.modifiers, 2);
        assert_eq!(k.key, "Enter");
    }

    #[test]
    fn parse_key_single_char() {
        let k = parse_key("a").unwrap();
        assert_eq!(k.key, "a");
        assert_eq!(k.code, "KeyA");
        assert_eq!(k.vk, 65);
        assert_eq!(k.text.as_deref(), Some("a"));
    }

    #[test]
    fn parse_key_rejects_unknown() {
        assert!(parse_key("F1").is_err());
        assert!(parse_key("Hyper+Enter").is_err());
        assert!(parse_key("").is_err());
    }

    #[test]
    fn parse_wait_route() {
        assert_eq!(
            parse_wait_condition("route:/settings").unwrap(),
            WaitCondition::Route("/settings".into())
        );
        assert!(parse_wait_condition("route:settings").is_err());
    }

    #[test]
    fn parse_wait_text() {
        assert_eq!(
            parse_wait_condition("text:Saved").unwrap(),
            WaitCondition::Text("Saved".into())
        );
    }

    #[test]
    fn parse_wait_gone_normalizes_testid() {
        assert_eq!(
            parse_wait_condition("!spinner").unwrap(),
            WaitCondition::Gone("[data-testid=\"spinner\"]".into())
        );
    }

    #[test]
    fn parse_wait_selector_no_denylist() {
        // Waiting (read-only) may target controls the click denylist blocks.
        assert_eq!(
            parse_wait_condition("go-live-button").unwrap(),
            WaitCondition::Selector("[data-testid=\"go-live-button\"]".into())
        );
    }

    #[test]
    fn wait_probe_js_shapes() {
        let js = wait_probe_js(&WaitCondition::Route("/settings".into()));
        assert!(js.contains("location.pathname"));
        let js = wait_probe_js(&WaitCondition::Gone("[data-testid=\"x\"]".into()));
        assert!(js.starts_with('!'));
        let js = wait_probe_js(&WaitCondition::Text("Hello \"world\"".into()));
        assert!(js.contains("innerText"));
        assert!(js.contains("\\\"world\\\""));
    }

    #[test]
    fn type_text_caps_length() {
        let long = "x".repeat(TYPE_TEXT_MAX + 1);
        let err = type_text("some-input", &long, true).unwrap_err();
        assert!(err.contains("too long"), "unexpected error: {err}");
    }
}
