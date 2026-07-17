// Provider registry — the seam that lets Swerve Build drive any agent, not just
// Grok. Grok is the built-in default and behaves exactly as before. Other ACP
// agents (Claude Code, Gemini) become drop-in once their CLI is on PATH. HTTP /
// local-LLM providers (Ollama, OpenAI-compatible, Anthropic) are DESIGNED and
// listed here but intentionally not yet spawnable — see resolve_launch().
//
// Future HTTP adapter seam: when we implement local/HTTP chat, ProviderKind::Http
// gets its own path in AcpManager::send_prompt that streams from base_url instead
// of spawning an ACP subprocess. Everything else (the store, the picker, the
// Settings UI) already understands Http providers.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Acp,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub kind: ProviderKind,
    /// ACP executable. None means "resolve the built-in Grok binary".
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub accent: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub builtin: bool,
}

/// What the frontend receives: a Provider plus computed availability/active flags,
/// flattened so JSON looks like `{ ...provider, available, active }`.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    #[serde(flatten)]
    pub provider: Provider,
    pub available: bool,
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct ProviderStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub available: bool,
    pub kind: ProviderKind,
}

/// Resolved launch spec handed to the ACP layer.
#[derive(Debug, Clone)]
pub struct AcpLaunch {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub label: String,
}

fn p(
    id: &str,
    label: &str,
    kind: ProviderKind,
    command: Option<&str>,
    args: &[&str],
    accent: &str,
    base_url: Option<&str>,
) -> Provider {
    Provider {
        id: id.to_string(),
        label: label.to_string(),
        kind,
        command: command.map(|c| c.to_string()),
        args: args.iter().map(|a| a.to_string()).collect(),
        env: Vec::new(),
        accent: accent.to_string(),
        model: None,
        base_url: base_url.map(|b| b.to_string()),
        builtin: true,
    }
}

pub fn builtin_providers() -> Vec<Provider> {
    vec![
        // Default — behaves exactly like the old hardcoded path.
        p("grok", "Grok", ProviderKind::Acp, None, &["agent", "stdio"], "#6cb5ff", None),
        p("claude-code", "Claude Code", ProviderKind::Acp, Some("claude-code-acp"), &[], "#d97757", None),
        p("gemini", "Gemini", ProviderKind::Acp, Some("gemini"), &["--experimental-acp"], "#6c8cff", None),
        // HTTP / local — designed, not yet spawnable.
        p("ollama", "Ollama", ProviderKind::Http, None, &[], "#cbd5e1", Some("http://localhost:11434")),
        p("openai-compatible", "OpenAI-compatible", ProviderKind::Http, None, &[], "#4dd2c0", Some("")),
        p("anthropic", "Anthropic", ProviderKind::Http, None, &[], "#d97757", Some("https://api.anthropic.com")),
    ]
}

// ---- persistence: ~/.swervebuild/providers.json -------------------------------

/// Custom-endpoint config for the built-in Grok provider — lets users run Grok
/// Build against their own OpenAI-compatible inference (local / self-hosted /
/// BYOK) so code never leaves the machine. The heavy lifting (writing grok's
/// `~/.grok/config.toml`) lives in `crate::grok_config`; this struct is just the
/// persisted state. `api_key` is stored here (like the rest of providers.json,
/// plaintext) and injected into grok's env at launch — never written to
/// config.toml. `previous_default` remembers the `[models] default` we displaced
/// when enabling, so disabling restores it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrokEndpoint {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_backend: String,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub previous_default: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProviderStore {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub custom: Vec<Provider>,
    #[serde(default)]
    pub endpoint: GrokEndpoint,
    /// Extra hosted model IDs the user added by hand (shown in the model picker
    /// alongside whatever `grok models` reports).
    #[serde(default)]
    pub custom_model_ids: Vec<String>,
}

impl ProviderStore {
    fn path() -> PathBuf {
        crate::paths::providers_file()
    }

    pub fn load() -> ProviderStore {
        let path = Self::path();
        if !path.exists() {
            return ProviderStore::default();
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return ProviderStore::default();
        };
        match serde_json::from_str(&raw) {
            Ok(store) => store,
            Err(err) => {
                if let Some(dest) =
                    crate::paths::quarantine_corrupt(&path, &crate::store::Store::now())
                {
                    eprintln!(
                        "providers.json failed to parse ({err}); quarantined to {}",
                        dest.display()
                    );
                }
                ProviderStore::default()
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())
    }
}

// ---- queries ------------------------------------------------------------------

pub fn all_providers() -> Vec<Provider> {
    let store = ProviderStore::load();
    let mut list = builtin_providers();
    for custom in store.custom {
        if !list.iter().any(|p| p.id == custom.id) {
            list.push(custom);
        }
    }
    list
}

pub fn active_id() -> String {
    ProviderStore::load()
        .active
        .unwrap_or_else(|| "grok".to_string())
}

pub fn get_provider(id: &str) -> Option<Provider> {
    all_providers().into_iter().find(|p| p.id == id)
}

pub fn views() -> Vec<ProviderView> {
    let active = active_id();
    all_providers()
        .into_iter()
        .map(|provider| {
            let available = is_available(&provider);
            let active = provider.id == active;
            ProviderView { available, active, provider }
        })
        .collect()
}

/// Resolve the executable for an ACP provider (or the built-in Grok binary).
fn resolve_command(provider: &Provider) -> Option<PathBuf> {
    match &provider.command {
        None => crate::resolve_grok_executable(),
        Some(cmd) => {
            let path = PathBuf::from(cmd);
            if path.is_absolute() && path.is_file() {
                Some(path)
            } else {
                crate::which_on_path(cmd)
            }
        }
    }
}

pub fn is_available(provider: &Provider) -> bool {
    match provider.kind {
        ProviderKind::Acp => resolve_command(provider).is_some(),
        ProviderKind::Http => false,
    }
}

pub fn resolve_launch(provider: &Provider, model_id: Option<&str>) -> Result<AcpLaunch, String> {
    match provider.kind {
        ProviderKind::Http => Err(format!(
            "{} is an HTTP / local provider — chat sessions aren't supported yet. Pick an ACP agent (Grok, Claude Code, Gemini).",
            provider.label
        )),
        ProviderKind::Acp => {
            let command = resolve_command(provider).ok_or_else(|| {
                if provider.id == "grok" {
                    "Grok Build is not installed.".to_string()
                } else {
                    format!(
                        "{} not found. Install its CLI ({}) and make sure it's on your PATH.",
                        provider.label,
                        provider.command.clone().unwrap_or_default()
                    )
                }
            })?;
            let mut args = provider.args.clone();
            let mut env = provider.env.clone();
            if provider.id == "grok" {
                if let Some(model) = model_id.filter(|m| !m.trim().is_empty()) {
                    insert_grok_model_flag(&mut args, model.trim());
                }
                // A custom Grok endpoint's API key rides in via env — see
                // `grok_endpoint_env`. The headless automation runner applies it too.
                env.extend(grok_endpoint_env());
            }
            Ok(AcpLaunch {
                command,
                args,
                env,
                label: provider.label.clone(),
            })
        }
    }
}

/// Insert `-m <model>` where the grok CLI accepts it: `-m` is an option of the
/// `agent` subcommand and must precede `stdio` — `grok agent -m X stdio` runs,
/// while `grok agent stdio -m X` exits 2 with "unexpected argument" (verified
/// against v0.2.93). So it goes immediately after "agent"; if there's no
/// "agent" in the args (plain/headless invocations take `-m` at root level),
/// appending is correct.
fn insert_grok_model_flag(args: &mut Vec<String>, model: &str) {
    let at = args
        .iter()
        .position(|a| a == "agent")
        .map(|p| p + 1)
        .unwrap_or(args.len());
    args.insert(at, model.to_string());
    args.insert(at, "-m".into());
}

fn command_version(path: &Path) -> Option<String> {
    let output = crate::util::hidden_command(path)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|line| line.trim().to_string())
}

pub fn provider_status(id: &str) -> ProviderStatus {
    let Some(provider) = get_provider(id) else {
        return ProviderStatus {
            installed: false,
            version: None,
            path: None,
            available: false,
            kind: ProviderKind::Acp,
        };
    };

    match provider.kind {
        ProviderKind::Http => ProviderStatus {
            installed: false,
            version: None,
            path: provider.base_url.clone(),
            available: false,
            kind: ProviderKind::Http,
        },
        ProviderKind::Acp => {
            let path = resolve_command(&provider);
            let installed = path.is_some();
            let version = path.as_deref().and_then(command_version);
            ProviderStatus {
                installed,
                version,
                path: path.map(|p| p.display().to_string()),
                available: installed,
                kind: ProviderKind::Acp,
            }
        }
    }
}

/// (success, message) for the Settings "Test" button.
pub fn test(id: &str) -> (bool, String) {
    let Some(provider) = get_provider(id) else {
        return (false, "Unknown provider.".to_string());
    };
    match provider.kind {
        ProviderKind::Http => (
            false,
            format!(
                "{} is designed, but the HTTP adapter isn't implemented yet.",
                provider.label
            ),
        ),
        ProviderKind::Acp => match resolve_command(&provider) {
            None => (
                false,
                format!("{} CLI not found on your PATH.", provider.label),
            ),
            Some(path) => match command_version(&path) {
                Some(version) => (true, format!("{} · {}", path.display(), version)),
                None => (
                    true,
                    format!("Found at {} (version unavailable).", path.display()),
                ),
            },
        },
    }
}

// ---- custom Grok endpoint -----------------------------------------------------

pub fn get_endpoint() -> GrokEndpoint {
    ProviderStore::load().endpoint
}

/// Env vars every `grok` process should get when a custom-endpoint API key is
/// saved: the key referenced by the managed config block's `env_key`. Injected
/// whenever a key exists — not just when global routing is on — because the
/// endpoint model can also be selected per-chat/per-trigger via `-m`; the extra
/// env var is harmless when unused. Both the interactive chat launch and the
/// headless automation runner apply this.
pub fn grok_endpoint_env() -> Vec<(String, String)> {
    let endpoint = ProviderStore::load().endpoint;
    if !endpoint.api_key.is_empty() {
        vec![(crate::grok_config::API_KEY_ENV.to_string(), endpoint.api_key)]
    } else {
        Vec::new()
    }
}

/// Persist the endpoint config and reflect it into `~/.grok/config.toml`.
/// `new_key`: `Some` replaces the stored API key (empty string clears it);
/// `None` keeps whatever was saved before. Base URL / model are trimmed.
pub fn save_endpoint(mut endpoint: GrokEndpoint, new_key: Option<String>) -> Result<(), String> {
    let mut store = ProviderStore::load();

    endpoint.api_key = match new_key {
        Some(key) => key,
        None => store.endpoint.api_key.clone(),
    };
    // Carry forward the displaced-default memory before (re)applying.
    endpoint.previous_default = store.endpoint.previous_default.clone();

    let base_url = endpoint.base_url.trim().to_string();
    let model = endpoint.model.trim().to_string();
    let backend = endpoint.api_backend.trim().to_string();

    let spec = crate::grok_config::EndpointSpec {
        enabled: endpoint.enabled,
        base_url: &base_url,
        model: &model,
        api_backend: if backend.is_empty() { None } else { Some(&backend) },
        context_window: endpoint.context_window,
    };
    let displaced = crate::grok_config::apply(&spec, endpoint.previous_default.as_deref())?;

    if endpoint.enabled {
        // Only overwrite the remembered default if we actually displaced one this
        // time (re-saving while already enabled displaces nothing — keep the old).
        if let Some(prev) = displaced {
            endpoint.previous_default = Some(prev);
        }
    } else {
        endpoint.previous_default = None;
    }

    endpoint.base_url = base_url;
    endpoint.model = model;
    endpoint.api_backend = backend;

    store.endpoint = endpoint;
    store.save()
}

// ---- model registry -----------------------------------------------------------
//
// A "model" is anything a chat or automation can pin via `grok -m`: a hosted
// model reported by `grok models`, a hosted ID the user added by hand, or the
// managed custom-endpoint entry. Local GGUF models join this list in a later
// phase — same registry, one more kind.

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    /// "hosted" (from `grok models`), "custom" (user-added ID), or "endpoint".
    pub kind: String,
    pub note: Option<String>,
    pub is_default: bool,
}

/// Parse `grok models` stdout. The list follows an "Available models:" header,
/// one per line, e.g. `  * grok-4.5 (default)` — the star and/or "(default)"
/// suffix mark the agent's default. Prose lines (anything with spaces after
/// cleanup) are skipped so header/footer text can't leak in as model IDs.
fn parse_grok_models(output: &str) -> Vec<(String, bool)> {
    let mut in_list = false;
    let mut out = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        if !in_list {
            if line.starts_with("Available models") {
                in_list = true;
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let mut is_default = line.starts_with('*');
        let mut name = line.trim_start_matches('*').trim();
        if let Some(stripped) = name.strip_suffix("(default)") {
            name = stripped.trim();
            is_default = true;
        }
        if name.is_empty() || name.contains(' ') {
            continue;
        }
        out.push((name.to_string(), is_default));
    }
    out
}

/// Everything selectable in the model pickers. Queries the grok CLI live so the
/// hosted list tracks whatever xAI currently offers this account; degrades to
/// custom/endpoint entries only when grok is missing or the query fails.
pub fn list_models() -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = Vec::new();

    if let Some(grok) = crate::resolve_grok_executable() {
        if let Ok(output) = crate::util::hidden_command(&grok).arg("models").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for (id, is_default) in parse_grok_models(&stdout) {
                out.push(ModelInfo {
                    label: id.clone(),
                    id,
                    kind: "hosted".into(),
                    note: None,
                    is_default,
                });
            }
        }
    }

    let store = ProviderStore::load();
    for id in &store.custom_model_ids {
        if out.iter().any(|m| &m.id == id) {
            continue;
        }
        out.push(ModelInfo {
            id: id.clone(),
            label: id.clone(),
            kind: "custom".into(),
            note: Some("added by you".into()),
            is_default: false,
        });
    }

    let endpoint = store.endpoint;
    if !endpoint.base_url.trim().is_empty() {
        out.push(ModelInfo {
            id: crate::grok_config::MODEL_ID.into(),
            label: "Custom endpoint".into(),
            kind: "endpoint".into(),
            note: Some(endpoint.base_url.trim().to_string()),
            is_default: false,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_models_single_default() {
        let out = parse_grok_models(
            "You are logged in with grok.com.\n\nDefault model: grok-4.5\n\nAvailable models:\n  * grok-4.5 (default)\n",
        );
        assert_eq!(out, vec![("grok-4.5".to_string(), true)]);
    }

    #[test]
    fn parse_models_multiple_mixed_markers() {
        let out = parse_grok_models(
            "Available models:\n  * grok-4.5 (default)\n    grok-code-fast-1\n  grok-4\n",
        );
        assert_eq!(
            out,
            vec![
                ("grok-4.5".to_string(), true),
                ("grok-code-fast-1".to_string(), false),
                ("grok-4".to_string(), false),
            ]
        );
    }

    #[test]
    fn parse_models_ignores_prose_and_preamble() {
        let out = parse_grok_models(
            "Default model: grok-4.5\nThese are not models.\nAvailable models:\n  * grok-4.5\n  some trailing prose here\n",
        );
        assert_eq!(out, vec![("grok-4.5".to_string(), true)]);
    }

    #[test]
    fn parse_models_empty_when_no_header() {
        assert!(parse_grok_models("no list in this output").is_empty());
    }

    #[test]
    fn model_flag_inserted_between_agent_and_stdio() {
        // Regression: `grok agent stdio -m X` exits 2 — the flag must precede
        // the stdio subcommand.
        let mut args = vec!["agent".to_string(), "stdio".to_string()];
        insert_grok_model_flag(&mut args, "grok-4.5");
        assert_eq!(args, vec!["agent", "-m", "grok-4.5", "stdio"]);
    }

    #[test]
    fn model_flag_appended_when_no_agent_subcommand() {
        let mut args = vec!["--some-flag".to_string()];
        insert_grok_model_flag(&mut args, "m1");
        assert_eq!(args, vec!["--some-flag", "-m", "m1"]);
    }
}
