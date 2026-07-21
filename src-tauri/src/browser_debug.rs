//! Browser debug pane (Roadmap Step 6, S12) — grant-gated, read-only MCP tools
//! that drive a dedicated hidden WebView2 window (`swerve-debug`) to debug
//! LOCAL web apps: navigate, read DOM/text, read console, read fetch/XHR
//! activity. Never the shell UI — that is `app_ui` (separate target, and
//! `pick_main_target` explicitly excludes this pane).
//!
//! The pane shares the app's CDP browser process, so it is just another target
//! on the endpoint published in `app_ui_cdp.json`. Console/network come from a
//! hook script registered once per target via
//! `Page.addScriptToEvaluateOnNewDocument` (runs at document-start on every
//! navigation) — the per-connection sidecar cannot hold CDP event
//! subscriptions. See `design/browser-debug.md` (S12 section) for limits.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::app_ui_cdp::{self, CdpTarget};

const GRANT_FILE: &str = "browser_debug_grant.json";
const TARGET_CACHE_FILE: &str = "browser_debug_target.json";
const NAV_LOG_FILE: &str = "browser_debug_nav.jsonl";
const NAV_LOG_KEEP: usize = 200;

/// The debug pane's creation URL — the discovery marker before any navigation.
/// `lib.rs` creates the hidden window at this URL; `pick_main_target` excludes it.
pub const DEBUG_PANE_INITIAL_URL: &str = "about:blank#swerve-debug-pane";
/// Tauri window label for the pane (used by the app-side show/hide commands).
pub const DEBUG_PANE_LABEL: &str = "swerve-debug";

/// Default / max characters returned by `browser_read_page`.
const READ_CAP_DEFAULT: usize = 8_000;
const READ_CAP_MAX: usize = 20_000;
/// Max console/network entries returned per read (hook buffers hold 300).
const ENTRIES_CAP: usize = 100;
/// Page-load wait bounds for `browser_open`.
const LOAD_TIMEOUT_MS: u64 = 10_000;
const LOAD_POLL_MS: u64 = 250;

// --------------------------------------------------------------------- grant

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDebugGrant {
    pub granted: bool,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for BrowserDebugGrant {
    fn default() -> Self {
        Self { granted: false, updated_at: String::new() }
    }
}

fn grant_path() -> PathBuf {
    crate::paths::data_dir().join(GRANT_FILE)
}

pub fn load_grant() -> BrowserDebugGrant {
    let path = grant_path();
    if !path.is_file() {
        return BrowserDebugGrant::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn set_granted(granted: bool) -> Result<BrowserDebugGrant, String> {
    let grant = BrowserDebugGrant { granted, updated_at: crate::store::Store::now() };
    let path = grant_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(&grant).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())?;
    Ok(grant)
}

/// Pure gate, unit-testable (same shape as `terminal::local_model_gate`).
fn grant_gate(granted: bool) -> Result<(), String> {
    if granted {
        Ok(())
    } else {
        Err("browser debug not granted. Human must enable \"Allow agent to use the debug browser\" in Settings → Agent browser debug.".into())
    }
}

pub fn require_grant() -> Result<(), String> {
    grant_gate(load_grant().granted)
}

// ----------------------------------------------------------------- URL policy

/// v1 navigation policy: http/https to loopback only (localhost, 127.0.0.0/8,
/// [::1]). The pane debugs LOCAL web apps; general browsing is refused.
fn validate_debug_url(raw: &str) -> Result<url::Url, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("url required".into());
    }
    if s.len() > 2000 {
        return Err("url too long (max 2000)".into());
    }
    let parsed = url::Url::parse(s).map_err(|e| format!("bad url: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("scheme {other}: only http/https are allowed")),
    }
    let loopback = match parsed.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if !loopback {
        return Err(format!(
            "host not allowed: {} — the debug browser is for LOCAL web apps (localhost / 127.0.0.1 / [::1] only)",
            parsed.host_str().unwrap_or("?")
        ));
    }
    Ok(parsed)
}

// ------------------------------------------------------------- target finding

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TargetCache {
    #[serde(default)]
    target_id: String,
    #[serde(default)]
    updated_at: String,
}

fn target_cache_path() -> PathBuf {
    crate::paths::data_dir().join(TARGET_CACHE_FILE)
}

/// The cached debug-pane target id, if any — used by `pick_main_target` to keep
/// `app_ui_*` off the pane.
pub fn cached_target_id() -> Option<String> {
    let raw = std::fs::read_to_string(target_cache_path()).ok()?;
    let cache: TargetCache = serde_json::from_str(&raw).ok()?;
    (!cache.target_id.is_empty()).then_some(cache.target_id)
}

fn save_target_id(id: &str) {
    let cache = TargetCache { target_id: id.into(), updated_at: crate::store::Store::now() };
    if let Ok(raw) = serde_json::to_string_pretty(&cache) {
        let _ = crate::paths::write_atomic(&target_cache_path(), raw.as_bytes());
    }
}

fn require_endpoint() -> Result<crate::app_ui::AppUiCdpEndpoint, String> {
    let ep = crate::app_ui::load_cdp_endpoint().ok_or_else(|| {
        "CDP endpoint not published. Restart SwerveBuild so it enables WebView2 remote debugging.".to_string()
    })?;
    if !app_ui_cdp::probe(&ep.host, ep.port) {
        return Err(format!(
            "CDP not reachable at {}:{} — is SwerveBuild running?",
            ep.host, ep.port
        ));
    }
    Ok(ep)
}

fn is_shell_url(url: &str) -> bool {
    ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost", "http://localhost:1420"]
        .iter()
        .any(|o| url.starts_with(o))
}

/// Find the debug pane target: cached id → marker initial URL → hook-marker
/// probe on non-shell page targets. Caches on success.
fn find_pane(ep: &crate::app_ui::AppUiCdpEndpoint) -> Result<CdpTarget, String> {
    let targets = app_ui_cdp::list_targets(&ep.host, ep.port)?;
    let pages: Vec<&CdpTarget> = targets
        .iter()
        .filter(|t| t.target_type == "page" && !t.url.starts_with("devtools://"))
        .collect();

    if let Some(cached) = cached_target_id() {
        if let Some(t) = pages.iter().find(|t| t.id == cached) {
            return Ok((*t).clone());
        }
    }
    if let Some(t) = pages.iter().find(|t| t.url.starts_with(DEBUG_PANE_INITIAL_URL)) {
        save_target_id(&t.id);
        return Ok((*t).clone());
    }
    // Last resort: the pane navigated away and the cache is gone — identify by
    // the injected hook marker (persists across navigations once bootstrapped).
    for t in pages.iter().filter(|t| !is_shell_url(&t.url)) {
        if app_ui_cdp::evaluate(&t.web_socket_debugger_url, "!!window.__swerveDebug")
            .map(|v| v.as_bool() == Some(true))
            .unwrap_or(false)
        {
            save_target_id(&t.id);
            return Ok((*t).clone());
        }
    }
    Err("debug pane not found — restart SwerveBuild (S12+) so it creates the hidden debug window".into())
}

// ------------------------------------------------------------------ hook code

/// Installed per target at document-start on every navigation. Wraps console,
/// error events, fetch, and XHR into capped ring buffers under
/// `window.__swerveDebug`. Idempotent per document via the guard.
const HOOK_JS: &str = r#"(function () {
  if (window.__swerveDebug) return;
  var buf = { console: [], network: [] };
  window.__swerveDebug = buf;
  function push(arr, e) { arr.push(e); if (arr.length > 300) arr.splice(0, arr.length - 300); }
  function fmt(a) {
    try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (_) { return String(a); }
  }
  ['log', 'info', 'warn', 'error', 'debug'].forEach(function (level) {
    var orig = console[level];
    console[level] = function () {
      try {
        var parts = Array.prototype.slice.call(arguments).map(fmt).join(' ');
        push(buf.console, { level: level, text: parts.slice(0, 2000), at: Date.now() });
      } catch (_) {}
      return orig.apply(this, arguments);
    };
  });
  window.addEventListener('error', function (e) {
    push(buf.console, { level: 'error', text: String(e.message) + ' @ ' + (e.filename || '') + ':' + (e.lineno || 0), at: Date.now() });
  });
  window.addEventListener('unhandledrejection', function (e) {
    push(buf.console, { level: 'error', text: ('Unhandled rejection: ' + String(e.reason)).slice(0, 2000), at: Date.now() });
  });
  var origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (input, init) {
      var u = String((input && input.url) || input).slice(0, 500);
      var m = (init && init.method) || (input && input.method) || 'GET';
      var t0 = Date.now();
      return origFetch.apply(this, arguments).then(function (r) {
        push(buf.network, { url: u, method: m, status: r.status, ms: Date.now() - t0, at: t0 });
        return r;
      }, function (err) {
        push(buf.network, { url: u, method: m, status: null, error: String(err).slice(0, 200), ms: Date.now() - t0, at: t0 });
        throw err;
      });
    };
  }
  if (window.XMLHttpRequest) {
    var XO = XMLHttpRequest.prototype.open, XS = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function (m, u) {
      this.__swerve = { method: String(m), url: String(u).slice(0, 500) };
      return XO.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function () {
      var meta = this.__swerve || {}; var t0 = Date.now(); var self = this;
      this.addEventListener('loadend', function () {
        push(buf.network, { url: meta.url || '', method: meta.method || 'GET', status: self.status || null, ms: Date.now() - t0, at: t0 });
      });
      return XS.apply(this, arguments);
    };
  }
})();"#;

// NOTE on hook lifetime: `Page.addScriptToEvaluateOnNewDocument` registrations
// are SESSION-scoped — they vanish when the registering DevTools session
// detaches. So `open()` registers and navigates on ONE held `CdpSession`; the
// navigated document runs the hook at document-start, and the buffers then live
// in the page itself (surviving session close). Consequence: page-internal
// navigations/reloads produce un-hooked documents until the next
// `browser_open`. Recorded in design/browser-debug.md.

// --------------------------------------------------------------------- tools

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn append_nav_log(url: &str, ok: bool, note: &str) {
    let path = crate::paths::data_dir().join(NAV_LOG_FILE);
    let entry = json!({ "at": crate::store::Store::now(), "url": url, "ok": ok, "note": note });
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .map(|raw| raw.lines().map(String::from).collect())
        .unwrap_or_default();
    lines.push(entry.to_string());
    let keep = lines.len().saturating_sub(NAV_LOG_KEEP);
    let body = lines[keep..].join("\n");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = crate::paths::write_atomic(&path, format!("{body}\n").as_bytes());
}

/// Navigate the pane to a LOCAL url, wait (bounded) for the document to settle,
/// and report where it landed. Load timeout is a normal `ok:false` result — the
/// agent can still read console/network to see why.
///
/// Everything runs on ONE held CDP session so the document-start hook script is
/// still registered at navigation commit (see the note above).
pub fn open(raw_url: &str) -> Result<Value, String> {
    require_grant()?;
    let url = validate_debug_url(raw_url)?;
    let ep = require_endpoint()?;
    let pane = find_pane(&ep)?;

    let mut session = app_ui_cdp::CdpSession::connect(&pane.web_socket_debugger_url)?;
    session.call("Page.enable", json!({}))?;
    session.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": HOOK_JS }))?;
    // Hook the CURRENT document too (harmless if already hooked — guarded).
    let _ = session.evaluate(HOOK_JS);
    session.call("Page.navigate", json!({ "url": url.as_str() }))?;

    let start = std::time::Instant::now();
    let mut loaded = false;
    while start.elapsed().as_millis() < u128::from(LOAD_TIMEOUT_MS) {
        // Mid-navigation evaluates can fail ("context destroyed") — that just
        // means not ready yet; keep polling within the same session.
        if let Ok(v) = session.evaluate("document.readyState") {
            if matches!(v.as_str(), Some("interactive") | Some("complete")) {
                loaded = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(LOAD_POLL_MS));
    }

    let landed = session
        .evaluate("location.href")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let title = session
        .evaluate("document.title")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    append_nav_log(url.as_str(), loaded, if loaded { "loaded" } else { "load timeout" });
    Ok(json!({
        "ok": loaded,
        "loaded": loaded,
        "url": landed,
        "title": title,
        "requested": url.as_str(),
        "via": "cdp",
    }))
}

/// Read the pane's page: full-page text digest, or one element's outerHTML.
pub fn read_page(selector: Option<&str>, max_chars: Option<u64>) -> Result<Value, String> {
    require_grant()?;
    let cap = max_chars
        .map(|c| (c as usize).clamp(500, READ_CAP_MAX))
        .unwrap_or(READ_CAP_DEFAULT);
    let ep = require_endpoint()?;
    let pane = find_pane(&ep)?;
    let ws = &pane.web_socket_debugger_url;

    let expression = match selector {
        Some(sel) => {
            let s = sel.trim();
            if s.is_empty() || s.len() > 300 {
                return Err("selector must be 1..300 chars".into());
            }
            let js_sel = js_str(s);
            format!(
                r#"(function() {{
  var el = document.querySelector({js_sel});
  if (!el) return {{ ok: false, error: 'not_found', selector: {js_sel} }};
  var html = el.outerHTML || '';
  return {{ ok: true, selector: {js_sel}, total: html.length, html: html.slice(0, {cap}) }};
}})()"#
            )
        }
        None => format!(
            r#"(function() {{
  var text = (document.body && document.body.innerText) || '';
  return {{ ok: true, url: location.href, title: document.title, total: text.length, text: text.slice(0, {cap}) }};
}})()"#
        ),
    };
    let mut out = app_ui_cdp::evaluate(ws, &expression)?;
    if let Some(obj) = out.as_object_mut() {
        let total = obj.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
        obj.insert("truncated".into(), json!(total as usize > cap));
    }
    Ok(out)
}

fn read_buffer(kind: &str, clear: bool) -> Result<Value, String> {
    require_grant()?;
    let ep = require_endpoint()?;
    let pane = find_pane(&ep)?;
    let ws = &pane.web_socket_debugger_url;
    let clear_js = if clear { "true" } else { "false" };
    let expression = format!(
        r#"(function() {{
  var d = window.__swerveDebug;
  if (!d) return {{ ok: false, error: 'hooks_not_installed', hint: 'call browser_open first' }};
  var arr = d.{kind} || [];
  var out = arr.slice(-{ENTRIES_CAP});
  var total = arr.length;
  if ({clear_js}) arr.length = 0;
  return {{ ok: true, total: total, returned: out.length, entries: out }};
}})()"#
    );
    app_ui_cdp::evaluate(ws, &expression)
}

/// Recent console entries (console.*, window errors, unhandled rejections).
pub fn console(clear: bool) -> Result<Value, String> {
    read_buffer("console", clear)
}

/// Recent fetch/XHR activity (url, method, status, duration). Resource loads
/// outside fetch/XHR are not captured — see design doc limits.
pub fn network(clear: bool) -> Result<Value, String> {
    read_buffer("network", clear)
}

/// Status for `browser_state`: grant, CDP, pane presence, current page.
pub fn state_report() -> Value {
    let grant = load_grant();
    let ep = crate::app_ui::load_cdp_endpoint();
    let ready = ep.as_ref().map(|e| app_ui_cdp::probe(&e.host, e.port)).unwrap_or(false);
    let (pane_found, url, title, hooks) = if ready {
        match require_endpoint().and_then(|ep| find_pane(&ep)) {
            Ok(pane) => {
                let ws = &pane.web_socket_debugger_url;
                let hooks = app_ui_cdp::evaluate(ws, "!!window.__swerveDebug")
                    .map(|v| v.as_bool() == Some(true))
                    .unwrap_or(false);
                (true, pane.url.clone(), pane.title.clone(), hooks)
            }
            Err(_) => (false, String::new(), String::new(), false),
        }
    } else {
        (false, String::new(), String::new(), false)
    };
    let tool_status = if !grant.granted {
        "not_granted (enable in Settings → Agent browser debug)"
    } else if !ready {
        "app_not_running (CDP endpoint unreachable)"
    } else if !pane_found {
        "pane_missing (restart SwerveBuild, S12+)"
    } else {
        "available"
    };
    json!({
        "granted": grant.granted,
        "grantUpdatedAt": grant.updated_at,
        "cdpReady": ready,
        "paneFound": pane_found,
        "paneUrl": url,
        "paneTitle": title,
        "hooksInstalled": hooks,
        "policy": "read-only; http/https to loopback hosts only; navigations logged",
        "tools": {
            "browser_state": "available",
            "browser_open": tool_status,
            "browser_read_page": tool_status,
            "browser_console": tool_status,
            "browser_network": tool_status,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grant_is_denied_and_gate_messages() {
        assert!(!BrowserDebugGrant::default().granted);
        assert!(grant_gate(true).is_ok());
        let err = grant_gate(false).unwrap_err();
        assert!(err.contains("not granted"), "got: {err}");
        assert!(err.contains("Settings"), "got: {err}");
    }

    #[test]
    fn url_policy_allows_loopback_http() {
        assert!(validate_debug_url("http://localhost:5500/index.html").is_ok());
        assert!(validate_debug_url("http://127.0.0.1:3000").is_ok());
        assert!(validate_debug_url("http://127.5.4.3:3000/x").is_ok()); // 127/8 loopback
        assert!(validate_debug_url("https://LOCALHOST:8443/app").is_ok());
        assert!(validate_debug_url("http://[::1]:8080/dev").is_ok());
    }

    #[test]
    fn url_policy_refuses_everything_else() {
        for bad in [
            "",
            "   ",
            "http://example.com",
            "https://google.com/",
            "http://192.168.1.10:8080",
            "http://10.0.0.5",
            "file:///C:/secrets.txt",
            "javascript:alert(1)",
            "ftp://localhost/x",
            "tauri://localhost",
            "not a url",
        ] {
            assert!(validate_debug_url(bad).is_err(), "should refuse: {bad}");
        }
        let long = format!("http://localhost/{}", "x".repeat(2100));
        assert!(validate_debug_url(&long).is_err());
    }

    #[test]
    fn shell_urls_are_recognized() {
        assert!(is_shell_url("tauri://localhost/settings"));
        assert!(is_shell_url("http://tauri.localhost/x"));
        assert!(is_shell_url("http://localhost:1420/workflows"));
        assert!(!is_shell_url("http://localhost:5500/app"));
        assert!(!is_shell_url(DEBUG_PANE_INITIAL_URL));
    }

    #[test]
    fn target_cache_roundtrips() {
        let cache = TargetCache { target_id: "T-1".into(), updated_at: "now".into() };
        let raw = serde_json::to_string(&cache).unwrap();
        let back: TargetCache = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.target_id, "T-1");
    }

    #[test]
    fn hook_js_is_guarded_and_buffers_capped() {
        // Cheap invariants on the injected source itself.
        assert!(HOOK_JS.contains("if (window.__swerveDebug) return;"));
        assert!(HOOK_JS.contains("arr.length > 300"));
        assert!(HOOK_JS.contains("unhandledrejection"));
        assert!(HOOK_JS.contains("XMLHttpRequest"));
    }
}
