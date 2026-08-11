//! Provider sign-in surface (GO-PUBLIC P1.1 / S-AUTH).
//!
//! S37 made agents installable from the app; this makes them *usable* on a
//! machine that has never signed in. Installing `claude-code-acp` or the Gemini
//! CLI does not authenticate it — the first chat fails with an auth error that
//! previously surfaced as a cryptic session/new failure.
//!
//! ROADMAP provider rule 3: **don't reimplement provider auth.** Each agent owns
//! its sign-in; our job is to *surface* it:
//!
//! - At `initialize`, ACP agents advertise `authMethods`
//!   (`claude-code-acp` → `[{ id: "claude-login", … }]`; Gemini →
//!   `oauth-personal` / `gemini-api-key` / `vertex-ai`).
//! - The ACP `authenticate` request runs the flow in-process where the agent
//!   supports it (Gemini's `oauth-personal` opens the browser and blocks until
//!   done).
//! - Where the real flow is a terminal program (`claude /login`), we open a
//!   **visible** terminal running exactly that — the one thing the agent's own
//!   error message tells the user to do.
//!
//! Deliberately NOT here: any MCP tool (sign-in is a human action from
//! Settings), any credential handling (the browser/terminal flows own the
//! secrets; we never see them), and any change to Grok's existing
//! `open_grok_login` flow.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// initialize + read-back budget for a probe. Agents answer initialize fast.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// `authenticate` budget. Gemini's oauth flow round-trips a real browser
/// login, so this must be generous — the user is typing a password somewhere.
const AUTH_TIMEOUT: Duration = Duration::from_secs(240);

// ------------------------------------------------------------------- shapes

/// One way an agent says it can be signed in (ACP `authMethods[]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    /// Human name; falls back to the id when the agent omits it.
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProbe {
    pub provider_id: String,
    pub label: String,
    pub auth_methods: Vec<AuthMethod>,
}

/// What `sign_in` actually did — the UI copy hangs off `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInOutcome {
    /// "authenticated" | "terminal-opened"
    pub kind: String,
    pub message: String,
}

/// How to run one method. Pure decision, unit-tested.
#[derive(Debug, Clone, PartialEq)]
pub enum SignInAction {
    /// Send ACP `authenticate { methodId }` on a live agent connection.
    Authenticate,
    /// The flow lives in a terminal program the user must interact with.
    Terminal { command: PathBuf, args: Vec<String> },
}

// ------------------------------------------------------------------ parsing

/// Pull `authMethods` out of an initialize result. Tolerant of missing
/// fields — an agent with no auth (Grok signs in via its own CLI flow)
/// yields an empty list, which the UI reads as "nothing to do here".
pub fn parse_auth_methods(initialize_result: &Value) -> Vec<AuthMethod> {
    let Some(arr) = initialize_result.get("authMethods").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let name = m
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(id);
            let description = m
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            Some(AuthMethod {
                id: id.to_string(),
                name: name.to_string(),
                description: description.to_string(),
            })
        })
        .collect()
}

/// Classify a stringified JSON-RPC error (what `Transport::rpc` returns) as
/// "the agent wants sign-in". ACP's `auth_required` is code **-32000**; the
/// message text is the fallback for agents that phrase it without the code.
pub fn is_auth_required_error(err: &str) -> bool {
    if let Ok(v) = serde_json::from_str::<Value>(err) {
        if v.get("code").and_then(|c| c.as_i64()) == Some(-32000) {
            return true;
        }
    }
    let lower = err.to_ascii_lowercase();
    ["authentication required", "auth required", "not authenticated", "please log in", "please login", "/login"]
        .iter()
        .any(|needle| lower.contains(needle))
}

// ------------------------------------------------------------- method plans

/// Decide how a given (provider, method) signs in.
///
/// `claude-login` is a terminal flow: `claude-code-acp`'s own description says
/// "Run `claude /login`" — its `authenticate` handler cannot complete an
/// interactive login for us, so we open the real thing instead of relaying an
/// error that tells the user to open it themselves.
pub fn plan_sign_in(provider_id: &str, method_id: &str) -> Result<SignInAction, String> {
    if provider_id == "claude-code" && method_id == "claude-login" {
        let claude = resolve_claude_cli().ok_or_else(|| {
            "Could not find the `claude` CLI (looked on PATH and inside the claude-code-acp install). Reinstall Claude Code from Settings → Providers, or run `claude /login` in your own terminal.".to_string()
        })?;
        return Ok(SignInAction::Terminal {
            command: claude,
            args: vec!["/login".into()],
        });
    }
    Ok(SignInAction::Authenticate)
}

/// The `claude` binary that ships *inside* the globally-installed
/// `claude-code-acp` package. Pure path construction so tests can point it at
/// a fake npm root; existence is checked by the caller.
pub fn nested_claude_shim(npm_global_root: &std::path::Path) -> PathBuf {
    npm_global_root
        .join("node_modules")
        .join("@zed-industries")
        .join("claude-code-acp")
        .join("node_modules")
        .join(".bin")
        .join("claude.cmd")
}

/// PATH first (many users have Claude Code proper installed), then the shim
/// nested in our own installed adapter package.
fn resolve_claude_cli() -> Option<PathBuf> {
    if let Some(on_path) = crate::which_on_path("claude") {
        return Some(on_path);
    }
    let appdata = std::env::var_os("APPDATA")?;
    let shim = nested_claude_shim(&PathBuf::from(appdata).join("npm"));
    shim.is_file().then_some(shim)
}

// ------------------------------------------------------------ agent probing

/// Kill-on-drop so an early `?` return can never leak an agent process.
///
/// Must be a **tree** kill: the launch command for npm agents is a `.cmd` shim,
/// and killing the shim leaves the node process under it alive (found live in
/// this feature's own drive — six orphaned `claude-code-acp` wrappers). Same
/// fix the automation runner uses (`jobs::tree_kill`, taskkill /T /F).
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        crate::jobs::tree_kill(self.0.id());
        let _ = self.0.wait();
    }
}

/// Spawn the provider's agent, run `initialize`, hand back (child, rx, stdin).
/// The reader thread forwards each stdout line; it ends when the pipe closes.
fn spawn_agent(
    provider_id: &str,
) -> Result<(ChildGuard, mpsc::Receiver<Value>, std::process::ChildStdin, String), String> {
    let provider = crate::providers::get_provider(provider_id)
        .ok_or_else(|| format!("unknown provider: {provider_id}"))?;
    let launch = crate::providers::resolve_launch(&provider, None)?;

    let mut cmd = crate::util::hidden_command(&launch.command);
    cmd.args(&launch.args)
        .envs(launch.env.iter().cloned())
        // Same rule as the ACP session pool (S37): an inherited CLAUDECODE
        // marker makes claude-code-acp refuse to start.
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", launch.label))?;
    let stdin = child.stdin.take().ok_or("agent stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("agent stdout unavailable")?;

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });

    Ok((ChildGuard(child), rx, stdin, launch.label))
}

fn rpc_line(id: u64, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// Wait for the response with `id`, ignoring notifications and other traffic.
fn wait_for_id(rx: &mpsc::Receiver<Value>, id: u64, deadline: Instant) -> Result<Value, String> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("timed out waiting for the agent".into());
        }
        match rx.recv_timeout(deadline - now) {
            Ok(v) => {
                if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    if let Some(err) = v.get("error") {
                        return Err(err.to_string());
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
            }
            Err(_) => return Err("agent exited or timed out".into()),
        }
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": crate::acp::ACP_PROTOCOL_VERSION,
        "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
        "clientInfo": {
            "name": "swerve-build",
            "title": "Swerve Build",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// One-shot: spawn → initialize → collect `authMethods` → kill.
pub fn probe(provider_id: &str) -> Result<AuthProbe, String> {
    let (_guard, rx, mut stdin, label) = spawn_agent(provider_id)?;
    let deadline = Instant::now() + PROBE_TIMEOUT;

    writeln!(stdin, "{}", rpc_line(1, "initialize", initialize_params()))
        .map_err(|e| format!("agent stdin write failed: {e}"))?;
    let init = wait_for_id(&rx, 1, deadline)?;

    Ok(AuthProbe {
        provider_id: provider_id.to_string(),
        label,
        auth_methods: parse_auth_methods(&init),
    })
}

// ----------------------------------------------------------------- sign-in

/// Run one sign-in method to completion (or hand off to a visible terminal).
pub fn sign_in(provider_id: &str, method_id: &str) -> Result<SignInOutcome, String> {
    match plan_sign_in(provider_id, method_id)? {
        SignInAction::Terminal { command, args } => {
            open_visible_terminal(&command, &args)?;
            Ok(SignInOutcome {
                kind: "terminal-opened".into(),
                message: format!(
                    "A terminal window opened running `{} {}` — finish sign-in there, then send a chat message to verify.",
                    command.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| command.display().to_string()),
                    args.join(" ")
                ),
            })
        }
        SignInAction::Authenticate => {
            let (_guard, rx, mut stdin, label) = spawn_agent(provider_id)?;
            let init_deadline = Instant::now() + PROBE_TIMEOUT;
            writeln!(stdin, "{}", rpc_line(1, "initialize", initialize_params()))
                .map_err(|e| format!("agent stdin write failed: {e}"))?;
            wait_for_id(&rx, 1, init_deadline)?;

            writeln!(
                stdin,
                "{}",
                rpc_line(2, "authenticate", json!({ "methodId": method_id }))
            )
            .map_err(|e| format!("agent stdin write failed: {e}"))?;

            // Browser flows block here until the user finishes (or gives up).
            wait_for_id(&rx, 2, Instant::now() + AUTH_TIMEOUT).map_err(|e| {
                format!("{label} sign-in did not complete: {e}")
            })?;

            Ok(SignInOutcome {
                kind: "authenticated".into(),
                message: format!("{label} signed in. Send a chat message to verify."),
            })
        }
    }
}

/// A real console window the user can interact with — the opposite of
/// `hidden_command`. `cmd /k` keeps it open so the flow's output stays
/// readable after it finishes.
#[cfg(windows)]
fn open_visible_terminal(command: &std::path::Path, args: &[String]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    std::process::Command::new("cmd")
        .arg("/k")
        .arg(command)
        .args(args)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|e| format!("could not open a terminal: {e}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn open_visible_terminal(_command: &std::path::Path, _args: &[String]) -> Result<(), String> {
    Err("visible terminal sign-in is Windows-only in this build".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_shaped_auth_methods() {
        // Real shape from claude-code-acp initialize (S37 finding).
        let init = json!({
            "protocolVersion": 1,
            "agentCapabilities": { "loadSession": true },
            "authMethods": [
                { "id": "claude-login", "description": "Run `claude /login` in the terminal" }
            ]
        });
        let methods = parse_auth_methods(&init);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].id, "claude-login");
        // No name from the agent → falls back to the id, never empty.
        assert_eq!(methods[0].name, "claude-login");
        assert!(methods[0].description.contains("/login"));
    }

    #[test]
    fn parses_gemini_shaped_auth_methods_and_keeps_order() {
        let init = json!({
            "authMethods": [
                { "id": "oauth-personal", "name": "Log in with Google", "description": "" },
                { "id": "gemini-api-key", "name": "Use Gemini API key" },
                { "id": "vertex-ai", "name": "Vertex AI" }
            ]
        });
        let methods = parse_auth_methods(&init);
        assert_eq!(
            methods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["oauth-personal", "gemini-api-key", "vertex-ai"],
            "first method is the default the UI offers — order must survive"
        );
        assert_eq!(methods[0].name, "Log in with Google");
    }

    #[test]
    fn missing_or_malformed_auth_methods_yield_empty() {
        assert!(parse_auth_methods(&json!({})).is_empty());
        assert!(parse_auth_methods(&json!({ "authMethods": "nope" })).is_empty());
        assert!(parse_auth_methods(&json!({ "authMethods": [{ "name": "no id" }, { "id": "  " }] }))
            .is_empty());
    }

    #[test]
    fn auth_required_detection_by_code_and_by_text() {
        // ACP auth_required code, as Transport::rpc stringifies it.
        assert!(is_auth_required_error(
            r#"{"code":-32000,"message":"Authentication required"}"#
        ));
        // Text-only phrasings agents actually use.
        assert!(is_auth_required_error("Please run `claude /login` first"));
        assert!(is_auth_required_error("not authenticated"));
        // Ordinary failures must NOT be misread as sign-in problems.
        assert!(!is_auth_required_error(
            r#"{"code":-32603,"message":"internal error"}"#
        ));
        assert!(!is_auth_required_error("timed out waiting for Grok"));
        // "author"/"authorization header" style words must not trip the check.
        assert!(!is_auth_required_error("unknown author field"));
    }

    #[test]
    fn claude_login_routes_to_a_terminal_everything_else_to_authenticate() {
        // Gemini methods → in-process ACP authenticate.
        assert_eq!(
            plan_sign_in("gemini", "oauth-personal").unwrap(),
            SignInAction::Authenticate
        );
        assert_eq!(
            plan_sign_in("gemini", "gemini-api-key").unwrap(),
            SignInAction::Authenticate
        );
        // Unknown provider/method combos default to the spec path, not an error.
        assert_eq!(
            plan_sign_in("some-future-agent", "whatever").unwrap(),
            SignInAction::Authenticate
        );
        // claude-login: Terminal when the CLI resolves, or a helpful error when
        // it does not — never Authenticate (the adapter can't complete it).
        match plan_sign_in("claude-code", "claude-login") {
            Ok(SignInAction::Terminal { command, args }) => {
                assert!(command.to_string_lossy().to_ascii_lowercase().contains("claude"));
                assert_eq!(args, vec!["/login".to_string()]);
            }
            Ok(SignInAction::Authenticate) => panic!("claude-login must never route to authenticate"),
            Err(msg) => assert!(msg.contains("claude /login"), "error must tell the user the manual path: {msg}"),
        }
    }

    #[test]
    fn nested_shim_path_is_the_adapter_scoped_bin() {
        let p = nested_claude_shim(std::path::Path::new(r"C:\Users\x\AppData\Roaming\npm"));
        assert_eq!(
            p,
            PathBuf::from(
                r"C:\Users\x\AppData\Roaming\npm\node_modules\@zed-industries\claude-code-acp\node_modules\.bin\claude.cmd"
            )
        );
    }
}
