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
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

// ---- installable ACP CLIs (S37) ----------------------------------------------

/// An agent CLI the app can install for you.
///
/// These ship on npm, unlike the Grok CLI (a pinned, checksummed binary — see
/// `install_grok_pinned`). Two consequences we live with deliberately:
///
/// 1. **npm must already exist.** We never bundle Node — the README promises no
///    Node/Rust for end users, and the app itself still honours that. Install is
///    an opt-in convenience, so when npm is missing we say so and show the
///    command rather than failing obscurely.
/// 2. **Versions are pinned here.** Same rule as the Grok CLI pin (audit item
///    A5): never install an unpinned `latest`. Bump via the DEPENDENCIES ritual.
#[derive(Debug, Clone, Serialize)]
pub struct InstallableCli {
    pub provider_id: &'static str,
    pub package: &'static str,
    pub version: &'static str,
    /// Where to send someone who has to do it by hand.
    pub docs: &'static str,
}

pub const INSTALLABLE_CLIS: &[InstallableCli] = &[
    InstallableCli {
        provider_id: "claude-code",
        package: "@zed-industries/claude-code-acp",
        version: "0.16.2",
        docs: "https://github.com/zed-industries/claude-code-acp",
    },
    InstallableCli {
        provider_id: "gemini",
        package: "@google/gemini-cli",
        version: "0.54.4",
        docs: "https://github.com/google-gemini/gemini-cli",
    },
];

pub fn installable_for(provider_id: &str) -> Option<&'static InstallableCli> {
    INSTALLABLE_CLIS
        .iter()
        .find(|c| c.provider_id == provider_id)
}

#[cfg(test)]
mod install_tests {
    use super::*;

    /// Never ship an unpinned `latest` — same rule as the Grok CLI pin (A5).
    #[test]
    fn install_commands_are_version_pinned() {
        for c in INSTALLABLE_CLIS {
            let cmd = c.install_command();
            assert!(cmd.contains("npm install -g"), "{cmd}");
            assert!(cmd.contains(&format!("{}@{}", c.package, c.version)), "{cmd}");
            assert!(!cmd.contains("latest"), "unpinned install: {cmd}");
            assert!(!c.version.is_empty());
            // Uninstall is by package name only — versions are meaningless there.
            assert_eq!(c.uninstall_command(), format!("npm uninstall -g {}", c.package));
        }
    }

    /// Only the two npm-distributed ACP agents are app-managed. Grok has its own
    /// pinned+checksummed installer; HTTP rows install nothing.
    #[test]
    fn only_npm_acp_agents_are_installable() {
        assert!(installable_for("claude-code").is_some());
        assert!(installable_for("gemini").is_some());
        assert!(installable_for("grok").is_none(), "grok is self-managed");
        assert!(installable_for("ollama").is_none());
        assert!(installable_for("anthropic").is_none());
        assert!(installable_for("nope").is_none());
    }

    /// The install target must match the provider's spawn command, or we would
    /// install a package that never makes the row go Available.
    #[test]
    fn installable_ids_match_real_providers() {
        let all = builtin_providers();
        for c in INSTALLABLE_CLIS {
            let p = all
                .iter()
                .find(|p| p.id == c.provider_id)
                .unwrap_or_else(|| panic!("no provider for {}", c.provider_id));
            assert!(matches!(p.kind, ProviderKind::Acp));
            assert!(p.command.is_some(), "{} needs a command to resolve", p.id);
        }
    }
}

impl InstallableCli {
    /// Exact command we run — and the one we show the user when npm is absent,
    /// so what they copy is what we would have done.
    pub fn install_command(&self) -> String {
        format!("npm install -g {}@{}", self.package, self.version)
    }

    pub fn uninstall_command(&self) -> String {
        format!("npm uninstall -g {}", self.package)
    }
}

pub fn builtin_providers() -> Vec<Provider> {
    vec![
        // Default — behaves exactly like the old hardcoded path.
        p("grok", "Grok", ProviderKind::Acp, None, &["agent", "stdio"], "#6cb5ff", None),
        p("claude-code", "Claude Code", ProviderKind::Acp, Some("claude-code-acp"), &[], "#d97757", None),
        p("gemini", "Gemini", ProviderKind::Acp, Some("gemini"), &["--experimental-acp"], "#6c8cff", None),
        // HTTP — not spawnable. Ollama and OpenAI-compatible are already
        // covered by shipped features (managed llama-server / custom Grok
        // endpoint), so they are labelled as covered rather than "coming soon"
        // forever. Anthropic direct is the only genuinely new reach (S37).
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

/// A registered local GGUF, served by the app-managed llama-server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModel {
    /// Registry/config id: `swerve-local-<slug>` — also the server's model alias.
    pub id: String,
    pub label: String,
    pub path: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub added_at: String,
}

/// Persisted state for the local inference server. Port and token are chosen
/// once and reused so the managed `config.toml` blocks stay stable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalConfig {
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub api_token: Option<String>,
    #[serde(default)]
    pub models: Vec<LocalModel>,
    /// Folder for catalog downloads (Phase 3). None → `~/.swervebuild/models/`.
    #[serde(default)]
    pub models_dir: Option<String>,
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
    #[serde(default)]
    pub local: LocalConfig,
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
        match serde_json::from_str::<ProviderStore>(&raw) {
            Ok(mut store) => {
                // P1.2: drain any plaintext secrets into the OS keystore.
                migrate_secrets(&mut store);
                store
            }
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

// ---- secret storage (P1.2 / audit A3) ------------------------------------------
//
// providers.json historically held two secrets in plaintext: the custom-endpoint
// API key and the local llama-server token. Both now live in the OS keystore
// (S36 seam). The JSON fields stay for serde compatibility but are drained on
// first load; the S36 rule holds — **no Tauri command returns either value**
// (the UI already only ever saw `has_api_key`).

/// Keystore entry for `GrokEndpoint.api_key`.
pub const ENDPOINT_KEY_SECRET: &str = "endpoint.api-key";
/// Keystore entry for `LocalConfig.api_token`.
pub const LOCAL_TOKEN_SECRET: &str = "local.llama-token";

/// Move any plaintext secrets into `set`, clearing each field **only after its
/// write succeeds** — a locked/broken keystore must never lose the secret, so
/// the old plaintext behavior remains the fallback. Returns whether the store
/// changed (caller persists). Injected writer so tests run without a keystore.
fn drain_plaintext_secrets(
    store: &mut ProviderStore,
    set: &mut dyn FnMut(&str, &str) -> Result<(), String>,
) -> bool {
    let mut changed = false;
    if !store.endpoint.api_key.is_empty()
        && set(ENDPOINT_KEY_SECRET, &store.endpoint.api_key).is_ok()
    {
        store.endpoint.api_key = String::new();
        changed = true;
    }
    if let Some(token) = store.local.api_token.clone() {
        if !token.is_empty() && set(LOCAL_TOKEN_SECRET, &token).is_ok() {
            store.local.api_token = None;
            changed = true;
        }
    }
    changed
}

/// One-time migration hook. Cost after migration: two `is_empty` checks.
fn migrate_secrets(store: &mut ProviderStore) {
    let mut set = |name: &str, value: &str| crate::secrets::set(name, value);
    if drain_plaintext_secrets(store, &mut set) {
        if let Err(err) = store.save() {
            eprintln!("providers.json save after secret migration failed: {err}");
        }
    }
}

/// The endpoint API key, wherever it currently lives (keystore, or the
/// plaintext field on a machine whose keystore write failed). Rust-side only.
pub fn endpoint_api_key() -> Option<String> {
    if let Ok(Some(key)) = crate::secrets::get(ENDPOINT_KEY_SECRET) {
        return Some(key);
    }
    let plain = ProviderStore::load().endpoint.api_key;
    (!plain.is_empty()).then_some(plain)
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

/// Env vars every `grok` process should get so managed models authenticate:
/// the custom-endpoint API key and the local llama-server token, each under the
/// env var its config block's `env_key` references. Injected whenever they
/// exist — models can be selected per-chat/per-trigger via `-m`, so the vars
/// must be present regardless of global routing; extra vars are harmless when
/// unused. Both the chat launch and the headless automation runner apply this.
pub fn grok_endpoint_env() -> Vec<(String, String)> {
    let store = ProviderStore::load();
    let mut env = Vec::new();
    // P1.2: secrets come from the keystore (with the plaintext fields as the
    // not-yet-migrated fallback) — reading them here is Rust-side launch env,
    // exactly what the S36 no-read-command rule permits.
    if let Some(key) = endpoint_api_key() {
        env.push((crate::grok_config::API_KEY_ENV.to_string(), key));
    }
    if !store.local.models.is_empty() {
        if let Some(token) = stored_local_token(&store) {
            env.push((crate::grok_config::LOCAL_API_KEY_ENV.to_string(), token));
        }
    }
    env
}

/// Current llama-server token, if one exists anywhere (keystore first, then the
/// pre-migration plaintext field). Does not create one — see [`ensure_local_token`].
fn stored_local_token(store: &ProviderStore) -> Option<String> {
    if let Ok(Some(token)) = crate::secrets::get(LOCAL_TOKEN_SECRET) {
        return Some(token);
    }
    store.local.api_token.clone().filter(|t| !t.is_empty())
}

/// Persist the endpoint config and reflect it into `~/.grok/config.toml`.
/// `new_key`: `Some` replaces the stored API key (empty string clears it);
/// `None` keeps whatever was saved before. Base URL / model are trimmed.
pub fn save_endpoint(mut endpoint: GrokEndpoint, new_key: Option<String>) -> Result<(), String> {
    let mut store = ProviderStore::load();

    // P1.2: the key's home is the keystore. `Some("")` clears; `Some(key)`
    // replaces; `None` keeps whatever storage already holds. Only if the
    // keystore write fails does the plaintext field carry the key (the
    // pre-migration fallback, so BYOK keeps working on a broken keystore).
    endpoint.api_key = String::new();
    match new_key {
        Some(key) if key.is_empty() => {
            crate::secrets::delete(ENDPOINT_KEY_SECRET)?;
        }
        Some(key) => {
            if let Err(err) = crate::secrets::set(ENDPOINT_KEY_SECRET, &key) {
                eprintln!("keystore write failed ({err}); keeping endpoint key in providers.json");
                endpoint.api_key = key;
            }
        }
        None => {
            // Keep a not-yet-migrated plaintext key if that's where it lives.
            endpoint.api_key = store.endpoint.api_key.clone();
        }
    }
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
/// `grok models` shells out to the CLI, and the model pickers call this on every
/// open — each one spawning a subprocess on the UI thread. Cache the parsed
/// hosted list briefly. User-added, endpoint and local models are read from the
/// store below and stay immediate, so newly registered models still appear at once.
static HOSTED_CACHE: Mutex<Option<(u64, Vec<(String, bool)>)>> = Mutex::new(None);
const HOSTED_TTL_SECS: u64 = 60;

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hosted_models() -> Vec<(String, bool)> {
    let now = now_secs();
    if let Ok(guard) = HOSTED_CACHE.lock() {
        if let Some((at, cached)) = guard.as_ref() {
            if now.saturating_sub(*at) < HOSTED_TTL_SECS {
                return cached.clone();
            }
        }
    }

    let mut fresh = Vec::new();
    if let Some(grok) = crate::resolve_grok_executable() {
        if let Ok(output) = crate::util::hidden_command(&grok).arg("models").output() {
            fresh = parse_grok_models(&String::from_utf8_lossy(&output.stdout));
        }
    }
    if let Ok(mut guard) = HOSTED_CACHE.lock() {
        *guard = Some((now, fresh.clone()));
    }
    fresh
}

pub fn list_models() -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = Vec::new();

    for (id, is_default) in hosted_models() {
        out.push(ModelInfo {
            label: id.clone(),
            id,
            kind: "hosted".into(),
            note: None,
            is_default,
        });
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

    for m in &store.local.models {
        out.push(ModelInfo {
            id: m.id.clone(),
            label: m.label.clone(),
            kind: "local".into(),
            note: Some(human_size(m.size_bytes)),
            is_default: false,
        });
    }

    out
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / 1e6)
    } else {
        format!("{bytes} B")
    }
}

// ---- local model registry -----------------------------------------------------

/// Persisted port for the local server (chosen once so config.toml stays
/// stable; re-chosen only if never set).
pub fn ensure_local_port() -> Result<u16, String> {
    let mut store = ProviderStore::load();
    if let Some(port) = store.local.port {
        return Ok(port);
    }
    let port = crate::local_llm::find_free_port()?;
    store.local.port = Some(port);
    store.save()?;
    Ok(port)
}

/// Generated once; passed to llama-server as `--api-key` and to grok via env,
/// so nothing else on the machine can quietly use the server.
///
/// P1.2: lives in the keystore. A keystore that cannot store the token falls
/// back to the old plaintext field — a working local server beats purity, and
/// the migration drains the field the moment the keystore recovers.
pub fn ensure_local_token() -> Result<String, String> {
    let store = ProviderStore::load();
    if let Some(token) = stored_local_token(&store) {
        return Ok(token);
    }
    let token = crate::store::Store::new_id();
    if crate::secrets::set(LOCAL_TOKEN_SECRET, &token).is_err() {
        let mut store = store;
        store.local.api_token = Some(token.clone());
        store.save()?;
    }
    Ok(token)
}

/// Slugify a GGUF file stem into a config-safe id under our namespace.
fn local_id_from_path(path: &str, taken: &[String]) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    let mut slug: String = stem
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        slug = "model".into();
    }
    let base = format!("{}{}", crate::grok_config::LOCAL_PREFIX, slug);
    let mut id = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t == &id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Register a GGUF from disk and sync the managed config blocks.
pub fn add_local_model(path: String) -> Result<Vec<LocalModel>, String> {
    let p = std::path::Path::new(&path);
    if !p.is_file() {
        return Err(format!("Not a file: {path}"));
    }
    if p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()) != Some("gguf".into()) {
        return Err("Pick a .gguf model file.".to_string());
    }

    let mut store = ProviderStore::load();
    if store.local.models.iter().any(|m| m.path == path) {
        return Err("That model file is already registered.".to_string());
    }
    let taken: Vec<String> = store.local.models.iter().map(|m| m.id.clone()).collect();
    let id = local_id_from_path(&path, &taken);
    let label = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let size_bytes = fs::metadata(p).map(|m| m.len()).unwrap_or(0);

    store.local.models.push(LocalModel {
        id,
        label,
        path,
        size_bytes,
        added_at: crate::store::Store::now(),
    });
    store.save()?;
    sync_local_config(&store)?;
    Ok(store.local.models)
}

pub fn remove_local_model(id: &str) -> Result<Vec<LocalModel>, String> {
    let mut store = ProviderStore::load();
    store.local.models.retain(|m| m.id != id);
    store.save()?;
    sync_local_config(&store)?;
    Ok(store.local.models)
}

/// Reflect the registry into `~/.grok/config.toml` (`[model.swerve-local-*]`).
fn sync_local_config(store: &ProviderStore) -> Result<(), String> {
    let port = match store.local.port {
        Some(p) => p,
        None => ensure_local_port()?,
    };
    let entries: Vec<(String, String)> = store
        .local
        .models
        .iter()
        .map(|m| (m.id.clone(), m.label.clone()))
        .collect();
    crate::grok_config::apply_local_models(&entries, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- P1.2: plaintext-secret drain --------------------------------------

    fn store_with_secrets(key: &str, token: Option<&str>) -> ProviderStore {
        let mut s = ProviderStore::default();
        s.endpoint.api_key = key.to_string();
        s.local.api_token = token.map(String::from);
        s
    }

    #[test]
    fn drain_moves_both_secrets_and_clears_fields() {
        let mut store = store_with_secrets("sk-endpoint-123", Some("tok-local-456"));
        let mut written: Vec<(String, String)> = Vec::new();
        let changed = drain_plaintext_secrets(&mut store, &mut |name, value| {
            written.push((name.to_string(), value.to_string()));
            Ok(())
        });
        assert!(changed);
        assert_eq!(
            written,
            vec![
                (ENDPOINT_KEY_SECRET.to_string(), "sk-endpoint-123".to_string()),
                (LOCAL_TOKEN_SECRET.to_string(), "tok-local-456".to_string()),
            ]
        );
        assert!(store.endpoint.api_key.is_empty(), "plaintext key must be drained");
        assert!(store.local.api_token.is_none(), "plaintext token must be drained");
    }

    #[test]
    fn drain_is_a_noop_when_nothing_is_plaintext() {
        // The steady state after migration: two cheap checks, zero writes.
        let mut store = store_with_secrets("", None);
        let mut calls = 0;
        let changed = drain_plaintext_secrets(&mut store, &mut |_, _| {
            calls += 1;
            Ok(())
        });
        assert!(!changed);
        assert_eq!(calls, 0);
        // An empty-string token must also not be "migrated".
        let mut store = store_with_secrets("", Some(""));
        assert!(!drain_plaintext_secrets(&mut store, &mut |_, _| panic!("no write expected")));
    }

    #[test]
    fn keystore_failure_keeps_the_plaintext_copy() {
        // A broken keystore must never lose the only copy of a secret.
        let mut store = store_with_secrets("sk-keep-me", Some("tok-keep-me"));
        let changed =
            drain_plaintext_secrets(&mut store, &mut |_, _| Err("keystore locked".into()));
        assert!(!changed, "failed writes must not mark the store dirty");
        assert_eq!(store.endpoint.api_key, "sk-keep-me");
        assert_eq!(store.local.api_token.as_deref(), Some("tok-keep-me"));
    }

    #[test]
    fn drain_is_per_secret_not_all_or_nothing() {
        // One secret landing in the keystore while the other write fails must
        // drain exactly the one that landed.
        let mut store = store_with_secrets("sk-ok", Some("tok-fails"));
        let changed = drain_plaintext_secrets(&mut store, &mut |name, _| {
            if name == ENDPOINT_KEY_SECRET { Ok(()) } else { Err("nope".into()) }
        });
        assert!(changed);
        assert!(store.endpoint.api_key.is_empty());
        assert_eq!(store.local.api_token.as_deref(), Some("tok-fails"));
    }

    /// Live proof of the A3 guarantee on this machine: seed plaintext secrets,
    /// run the real migration, and canary-scan providers.json afterwards. The
    /// real file is snapshot and restored even on panic.
    /// `cargo test -p swerve-build --lib providers::tests::live_migration_leaves_no_plaintext -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_migration_leaves_no_plaintext() {
        struct RestoreFile(PathBuf, Option<Vec<u8>>);
        impl Drop for RestoreFile {
            fn drop(&mut self) {
                match &self.1 {
                    Some(bytes) => { let _ = std::fs::write(&self.0, bytes); }
                    None => { let _ = std::fs::remove_file(&self.0); }
                }
            }
        }

        let path = ProviderStore::path();
        let _restore = RestoreFile(path.clone(), std::fs::read(&path).ok());
        let key_canary = "SWERVE-A3-KEY-CANARY-51c9d0";
        let tok_canary = "SWERVE-A3-TOKEN-CANARY-51c9d0";
        let _ = crate::secrets::delete(ENDPOINT_KEY_SECRET);
        let _ = crate::secrets::delete(LOCAL_TOKEN_SECRET);

        store_with_secrets(key_canary, Some(tok_canary))
            .save()
            .expect("seed providers.json");

        // load() runs the migration and persists the drained store.
        let migrated = ProviderStore::load();
        assert!(migrated.endpoint.api_key.is_empty());
        assert!(migrated.local.api_token.is_none());

        let raw = std::fs::read_to_string(&path).expect("read providers.json");
        assert!(!raw.contains(key_canary), "endpoint key leaked to disk");
        assert!(!raw.contains(tok_canary), "local token leaked to disk");

        assert_eq!(
            crate::secrets::get(ENDPOINT_KEY_SECRET).unwrap().as_deref(),
            Some(key_canary)
        );
        assert_eq!(
            crate::secrets::get(LOCAL_TOKEN_SECRET).unwrap().as_deref(),
            Some(tok_canary)
        );
        // Accessors resolve from the keystore.
        assert_eq!(endpoint_api_key().as_deref(), Some(key_canary));

        let _ = crate::secrets::delete(ENDPOINT_KEY_SECRET);
        let _ = crate::secrets::delete(LOCAL_TOKEN_SECRET);
        eprintln!("A3 migration round-trip OK — no plaintext left in providers.json");
    }

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

    #[test]
    fn local_id_slugifies_and_uniquifies() {
        let id = local_id_from_path("E:/x/Qwen2.5-Coder (Q4_K_M).gguf", &[]);
        assert_eq!(id, "swerve-local-qwen2-5-coder-q4-k-m");
        let taken = vec![id.clone()];
        let id2 = local_id_from_path("E:/y/Qwen2.5-Coder (Q4_K_M).gguf", &taken);
        assert_eq!(id2, "swerve-local-qwen2-5-coder-q4-k-m-2");
    }
}
