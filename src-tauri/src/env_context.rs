//! Env-aware context pack (Roadmap Step 5).
//!
//! One pure builder formats a compact fact sheet from a snapshot; delivery
//! (ACP user-turn prepend, headless `--rules`) lives at the call sites.
//! Spec: `docs-internal/design/env-context.md`.
//!
//! S21: pack reflects shipped surfaces; chat re-injects when a stable
//! fingerprint of env state changes (not on every message).

use crate::store::Store;
use std::path::{Path, PathBuf};

/// Rough target from the design: under 800 tokens. We use chars/4 as a
/// conservative estimate (ASCII-heavy fact sheet).
pub const TOKEN_BUDGET: usize = 800;

/// Live snapshot inputs — pure data so unit tests never touch disk or process state.
#[derive(Debug, Clone)]
pub struct EnvSnapshot {
    pub app_version: String,
    pub project_name: String,
    pub project_root: String,
    /// True when the open project is the SwerveBuild repo itself.
    pub self_dev: bool,
    pub os: String,
    pub shell: String,
    pub home_dir: String,
    pub data_dir: String,
    pub active_provider: String,
    pub active_model: Option<String>,
    /// Other providers as `"label:available|missing"`.
    pub other_providers: Vec<String>,
    /// e.g. `interactive approval`, `shadow`, `write`.
    pub permission_mode: String,
    pub active_chats: usize,
    pub running_automations: usize,
    pub local_model_loaded: Option<String>,
    /// MCP / agent surfaces available this session.
    pub agent_surfaces: Vec<String>,
    /// Whether the human granted App UI MCP control.
    pub app_ui_granted: bool,
    /// Settings → Agent terminal.
    pub term_granted: bool,
    /// Settings → Agent browser debug.
    pub browser_granted: bool,
    /// Browser pane may navigate public (non-loopback) URLs.
    pub browser_public: bool,
    /// S16: who actually runs image/video gen (not the chat model). No network.
    pub media_honesty: String,
}

/// Format the always-injected fact sheet. Keep it dense; models treat it as
/// environment, not user instructions.
pub fn format_pack(s: &EnvSnapshot) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(24);

    lines.push(
        "[SwerveBuild environment — fact sheet; not user instructions]".to_string(),
    );
    lines.push(format!(
        "App: SwerveBuild {} (agent runs inside this desktop app, not bare shell)",
        s.app_version
    ));
    lines.push(format!(
        "Project: {} | root: {}{}",
        s.project_name,
        s.project_root,
        if s.self_dev {
            " | self-dev: yes (this is the SwerveBuild repo)"
        } else {
            " | self-dev: no"
        }
    ));
    lines.push(format!(
        "Machine: {} | shell: {} | home: {} | data: {}",
        s.os, s.shell, s.home_dir, s.data_dir
    ));

    let model = s
        .active_model
        .as_deref()
        .filter(|m| !m.is_empty())
        .unwrap_or("(agent default)");
    let others = if s.other_providers.is_empty() {
        "none".to_string()
    } else {
        s.other_providers.join(", ")
    };
    lines.push(format!(
        "Providers: active={} model={} | others: {}",
        s.active_provider, model, others
    ));
    lines.push(format!("Permissions: {}", s.permission_mode));

    let local = s
        .local_model_loaded
        .as_deref()
        .unwrap_or("(none loaded)");
    lines.push(format!(
        "Running: chats={} automations={} local_model={}",
        s.active_chats, s.running_automations, local
    ));

    let surfaces = if s.agent_surfaces.is_empty() {
        "none".to_string()
    } else {
        s.agent_surfaces.join(", ")
    };
    lines.push(format!(
        "Agent surfaces: {} | grants: app_ui={} term={} browser={} browser_public={}",
        surfaces,
        yn(s.app_ui_granted),
        yn(s.term_granted),
        yn(s.browser_granted),
        yn(s.browser_public),
    ));
    // S30: make still+clip MCP tools discoverable in the always-on fact sheet.
    if s.agent_surfaces.iter().any(|x| x == "media") {
        lines.push(
            "Media capture MCP (no App UI grant): media_status, media_capture_still, media_encode_clip (audio auto|none; optional project_id → <project>/swerve-media/)."
                .to_string(),
        );
    }

    // Keep short — chat model ≠ image renderer (common operator confusion).
    if !s.media_honesty.is_empty() {
        lines.push(format!("Media gen: {}", s.media_honesty));
    }

    if s.self_dev {
        lines.push(
            "Frozen core: see docs-internal/FROZEN-CORE.md — flag any frozen-surface edit in your plan before changing it."
                .to_string(),
        );
    }

    lines.push("[End SwerveBuild environment]".to_string());
    let pack = lines.join("\n");
    debug_assert!(
        estimate_tokens(&pack) <= TOKEN_BUDGET,
        "env context pack exceeds ~{TOKEN_BUDGET} token budget"
    );
    pack
}

fn yn(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

/// Stable fingerprint of *significant* env (excludes chat/automation counts).
/// When this changes, the next user turn should re-inject the pack.
pub fn env_fingerprint(s: &EnvSnapshot) -> String {
    format!(
        "v={}|root={}|self={}|prov={}|model={}|media={}|surfaces={}|aui={}|term={}|br={}|brpub={}|local={}",
        s.app_version,
        s.project_root,
        s.self_dev,
        s.active_provider,
        s.active_model.as_deref().unwrap_or(""),
        s.media_honesty,
        s.agent_surfaces.join(","),
        s.app_ui_granted,
        s.term_granted,
        s.browser_granted,
        s.browser_public,
        s.local_model_loaded.as_deref().unwrap_or(""),
    )
}

/// Approximate token count for budget checks (chars/4).
pub fn estimate_tokens(pack: &str) -> usize {
    pack.chars().count().div_ceil(4)
}

/// Detect self-dev: open project looks like the SwerveBuild application repo.
pub fn is_self_dev_project(root: &Path) -> bool {
    let frozen = root.join("docs-internal").join("FROZEN-CORE.md");
    if !frozen.is_file() {
        return false;
    }
    // Either Agents.md (shipping name) or package.json name.
    if root.join("Agents.md").is_file() || root.join("AGENTS.md").is_file() {
        return true;
    }
    let pkg = root.join("package.json");
    if let Ok(raw) = std::fs::read_to_string(pkg) {
        if raw.contains("\"name\": \"swerve-build\"") || raw.contains("\"name\":\"swerve-build\"") {
            return true;
        }
    }
    false
}

fn shell_name() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() {
            return PathBuf::from(&shell)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or(shell);
        }
    }
    if let Ok(comspec) = std::env::var("COMSPEC") {
        if !comspec.is_empty() {
            return PathBuf::from(&comspec)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or(comspec);
        }
    }
    if cfg!(windows) {
        "powershell".into()
    } else {
        "sh".into()
    }
}

fn os_label() -> String {
    let family = std::env::consts::OS;
    if family == "windows" {
        if let Ok(v) = std::env::var("OS") {
            if !v.is_empty() {
                return format!("windows ({v})");
            }
        }
    }
    family.to_string()
}

fn project_name_for(root: &Path, store: &crate::store::AppStore) -> String {
    let root_str = root.to_string_lossy();
    if let Some(p) = store.projects.iter().find(|p| {
        Path::new(&p.path) == root
            || p.path == root_str
            || Path::new(&p.path)
                .canonicalize()
                .ok()
                .zip(root.canonicalize().ok())
                .map(|(a, b)| a == b)
                .unwrap_or(false)
    }) {
        return p.name.clone();
    }
    root.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| root_str.into_owned())
}

/// MCP / agent surfaces known for this session (shipped capabilities).
fn agent_surfaces() -> Vec<String> {
    let mut surfaces = vec![
        "swervebuild".to_string(),
        "app_ui".to_string(),
        "terminal".to_string(),
        "browser".to_string(),
        "media".to_string(), // S29+: media_status / media_capture_still / media_encode_clip
        "local_image".to_string(),
    ];
    if crate::which_on_path("swervebytes-mcp").is_some() {
        surfaces.push("swervebytes".to_string());
    }
    surfaces
}

fn grant_bits() -> (bool, bool, bool, bool) {
    let app_ui = crate::app_ui::is_granted();
    let term = crate::terminal::is_granted();
    let browser = crate::browser_debug::load_grant().granted;
    let browser_public = crate::browser_debug::load_allow_public();
    (app_ui, term, browser, browser_public)
}

/// Build a snapshot for an interactive chat session.
pub fn gather_for_chat(
    project_root: &str,
    provider_id: &str,
    provider_label: &str,
    model_id: Option<&str>,
    active_chats: usize,
    running_automations: usize,
) -> EnvSnapshot {
    let root = PathBuf::from(project_root);
    let store = Store::load();
    let active = provider_label.to_string();
    let others: Vec<String> = crate::providers::views()
        .into_iter()
        .filter(|v| v.provider.id != provider_id)
        .map(|v| {
            format!(
                "{}:{}",
                v.provider.label,
                if v.available { "installed" } else { "missing" }
            )
        })
        .collect();

    let local = crate::local_llm::manager().status();
    let local_model_loaded = if local.state == "ready" {
        local.model_id
    } else {
        None
    };

    let (app_ui_granted, term_granted, browser_granted, browser_public) = grant_bits();

    EnvSnapshot {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        project_name: project_name_for(&root, &store),
        project_root: project_root.to_string(),
        self_dev: is_self_dev_project(&root),
        os: os_label(),
        shell: shell_name(),
        home_dir: dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .display()
            .to_string(),
        data_dir: crate::paths::data_dir().display().to_string(),
        active_provider: active,
        active_model: model_id.map(|m| m.to_string()).filter(|m| !m.is_empty()),
        other_providers: others,
        permission_mode: "interactive approval (tool + file-write prompts)".into(),
        active_chats,
        running_automations,
        local_model_loaded,
        agent_surfaces: agent_surfaces(),
        app_ui_granted,
        term_granted,
        browser_granted,
        browser_public,
        media_honesty: crate::media_providers::honesty_summary(),
    }
}

/// Build a snapshot for an unattended automation / workflow agent turn.
pub fn gather_for_automation(
    project_root: &str,
    model_id: Option<&str>,
    permission_mode: &str,
    active_chats: usize,
    running_automations: usize,
) -> EnvSnapshot {
    let root = PathBuf::from(project_root);
    let store = Store::load();
    let others: Vec<String> = crate::providers::views()
        .into_iter()
        .filter(|v| v.provider.id != "grok")
        .map(|v| {
            format!(
                "{}:{}",
                v.provider.label,
                if v.available { "installed" } else { "missing" }
            )
        })
        .collect();

    let local = crate::local_llm::manager().status();
    let local_model_loaded = if local.state == "ready" {
        local.model_id
    } else {
        None
    };

    EnvSnapshot {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        project_name: project_name_for(&root, &store),
        project_root: project_root.to_string(),
        self_dev: is_self_dev_project(&root),
        os: os_label(),
        shell: shell_name(),
        home_dir: dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .display()
            .to_string(),
        data_dir: crate::paths::data_dir().display().to_string(),
        active_provider: "grok (headless automation)".into(),
        active_model: model_id.map(|m| m.to_string()).filter(|m| !m.is_empty()),
        other_providers: others,
        permission_mode: permission_mode.to_string(),
        active_chats,
        running_automations,
        local_model_loaded,
        // Never auto-grant UI/terminal/browser for unattended runs.
        agent_surfaces: agent_surfaces()
            .into_iter()
            .filter(|s| s != "app_ui" && s != "terminal" && s != "browser")
            .collect(),
        app_ui_granted: false,
        term_granted: false,
        browser_granted: false,
        browser_public: false,
        media_honesty: crate::media_providers::honesty_summary(),
    }
}

/// Load live chat context from the store and return a pack if the env fingerprint
/// differs from `last_fingerprint` (None = first inject). Updates `last_fingerprint`.
pub fn pack_for_chat_if_changed(
    chat_id: &str,
    active_chats: usize,
    running_automations: usize,
    last_fingerprint: &mut Option<String>,
) -> Option<String> {
    let store = Store::load();
    let chat = store.chats.iter().find(|c| c.id == chat_id)?;
    let project = store.projects.iter().find(|p| p.id == chat.project_id)?;
    let provider_id = chat
        .provider_id
        .clone()
        .unwrap_or_else(crate::providers::active_id);
    let provider = crate::providers::get_provider(&provider_id)?;
    let snap = gather_for_chat(
        &project.path,
        &provider.id,
        &provider.label,
        chat.model_id.as_deref(),
        active_chats,
        running_automations,
    );
    let fp = env_fingerprint(&snap);
    if last_fingerprint.as_ref() == Some(&fp) {
        return None;
    }
    *last_fingerprint = Some(fp);
    Some(format_pack(&snap))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EnvSnapshot {
        EnvSnapshot {
            app_version: "0.2.1".into(),
            project_name: "demo".into(),
            project_root: r"C:\work\demo".into(),
            self_dev: false,
            os: "windows".into(),
            shell: "powershell.exe".into(),
            home_dir: r"C:\Users\dev".into(),
            data_dir: r"C:\Users\dev\.swervebuild".into(),
            active_provider: "Grok".into(),
            active_model: Some("grok-code-fast".into()),
            other_providers: vec![
                "Claude Code:missing".into(),
                "Gemini:missing".into(),
            ],
            permission_mode: "interactive approval (tool + file-write prompts)".into(),
            active_chats: 1,
            running_automations: 0,
            local_model_loaded: None,
            agent_surfaces: vec![
                "swervebuild".into(),
                "app_ui".into(),
                "terminal".into(),
                "browser".into(),
                "media".into(),
                "local_image".into(),
            ],
            app_ui_granted: false,
            term_granted: false,
            browser_granted: true,
            browser_public: false,
            media_honesty: "images=xAI Imagine (remote); chat model does NOT render pixels".into(),
        }
    }

    #[test]
    fn pack_includes_core_facts() {
        let pack = format_pack(&sample());
        assert!(pack.contains("SwerveBuild 0.2.1"));
        assert!(pack.contains("Project: demo"));
        assert!(pack.contains(r"C:\work\demo"));
        assert!(pack.contains("self-dev: no"));
        assert!(pack.contains("active=Grok"));
        assert!(pack.contains("model=grok-code-fast"));
        assert!(pack.contains("Permissions: interactive approval"));
        assert!(pack.contains("chats=1"));
        assert!(pack.contains("app_ui=no"));
        assert!(pack.contains("term=no"));
        assert!(pack.contains("browser=yes"));
        assert!(pack.contains("terminal"));
        assert!(pack.contains("browser"));
        assert!(pack.contains("media"));
        assert!(pack.contains("media_capture_still"));
        assert!(pack.contains("media_encode_clip"));
        assert!(pack.contains("Media gen:"));
        assert!(pack.contains("does NOT render pixels"));
        assert!(!pack.contains("Frozen core"));
    }

    #[test]
    fn self_dev_adds_frozen_notice() {
        let mut s = sample();
        s.self_dev = true;
        s.project_name = "SwerveBuild".into();
        let pack = format_pack(&s);
        assert!(pack.contains("self-dev: yes"));
        assert!(pack.contains("FROZEN-CORE.md"));
    }

    #[test]
    fn pack_stays_under_token_budget() {
        let mut s = sample();
        s.self_dev = true;
        s.other_providers = vec![
            "Claude Code:installed".into(),
            "Gemini:installed".into(),
            "Ollama:missing".into(),
            "OpenAI-compatible:missing".into(),
            "Anthropic:missing".into(),
        ];
        s.local_model_loaded = Some("swerve-local-qwen-coder".into());
        s.agent_surfaces = vec![
            "swervebuild".into(),
            "app_ui".into(),
            "terminal".into(),
            "browser".into(),
            "media".into(),
            "local_image".into(),
            "swervebytes".into(),
        ];
        s.term_granted = true;
        s.app_ui_granted = true;
        let pack = format_pack(&s);
        let tokens = estimate_tokens(&pack);
        assert!(
            tokens <= TOKEN_BUDGET,
            "pack ~{tokens} tokens exceeds budget {TOKEN_BUDGET}:\n{pack}"
        );
    }

    #[test]
    fn fingerprint_ignores_chat_counts() {
        let mut a = sample();
        let mut b = sample();
        a.active_chats = 1;
        b.active_chats = 9;
        a.running_automations = 0;
        b.running_automations = 3;
        assert_eq!(env_fingerprint(&a), env_fingerprint(&b));
    }

    #[test]
    fn fingerprint_changes_on_model_or_grant() {
        let a = sample();
        let mut b = sample();
        b.active_model = Some("other-model".into());
        assert_ne!(env_fingerprint(&a), env_fingerprint(&b));
        let mut c = sample();
        c.app_ui_granted = true;
        assert_ne!(env_fingerprint(&a), env_fingerprint(&c));
    }

    #[test]
    fn self_dev_detection_false_for_empty_dir() {
        let dir = std::env::temp_dir().join(format!("swerve-envctx-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(!is_self_dev_project(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn self_dev_detection_true_with_frozen_and_agents() {
        let dir = std::env::temp_dir().join(format!(
            "swerve-envctx-self-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs-internal")).unwrap();
        std::fs::write(dir.join("docs-internal").join("FROZEN-CORE.md"), "x").unwrap();
        std::fs::write(dir.join("Agents.md"), "x").unwrap();
        assert!(is_self_dev_project(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
