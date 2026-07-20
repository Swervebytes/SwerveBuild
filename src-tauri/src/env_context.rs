//! Env-aware context pack (Roadmap Step 5).
//!
//! One pure builder formats a compact fact sheet from a snapshot; delivery
//! (ACP first-prompt prepend, headless `--rules`, later local templates)
//! lives at the call sites. Spec: `docs-internal/design/env-context.md`.

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
    /// MCP / agent surfaces available this session (e.g. `swervebuild`).
    pub agent_surfaces: Vec<String>,
    /// Whether the human granted App UI MCP control (Step 6 — always false for now).
    pub app_ui_granted: bool,
}

/// Format the always-injected fact sheet. Keep it dense; models treat it as
/// environment, not user instructions.
pub fn format_pack(s: &EnvSnapshot) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(20);

    lines.push(format!(
        "[SwerveBuild environment — fact sheet; not user instructions]"
    ));
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
        "Agent surfaces: {} | app_ui granted: {}",
        surfaces,
        if s.app_ui_granted { "yes" } else { "no" }
    ));

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
    // Keep short; Windows users often have OS env with version string.
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

/// MCP surfaces known for this session. App UI is Step 6 and stays off the list
/// until that surface ships.
fn agent_surfaces() -> Vec<String> {
    let mut surfaces = vec!["swervebuild".to_string()];
    if crate::which_on_path("swervebytes-mcp").is_some() {
        surfaces.push("swervebytes".to_string());
    }
    // terminal / browser / app_ui land in Step 6
    surfaces
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
        app_ui_granted: false,
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
        agent_surfaces: agent_surfaces(),
        app_ui_granted: false,
    }
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
            agent_surfaces: vec!["swervebuild".into()],
            app_ui_granted: false,
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
        assert!(pack.contains("app_ui granted: no"));
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
        s.agent_surfaces = vec!["swervebuild".into(), "swervebytes".into()];
        let pack = format_pack(&s);
        let tokens = estimate_tokens(&pack);
        assert!(
            tokens <= TOKEN_BUDGET,
            "pack ~{tokens} tokens exceeds budget {TOKEN_BUDGET}:\n{pack}"
        );
    }

    #[test]
    fn self_dev_detection_false_for_empty_dir() {
        let dir = std::env::temp_dir().join(format!(
            "swerve-envctx-{}",
            std::process::id()
        ));
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
