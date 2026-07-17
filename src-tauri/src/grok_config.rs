//! Manage Grok Build's user-level `~/.grok/config.toml` so Swerve Build can route
//! Grok through a custom inference endpoint (local, self-hosted, or BYOK).
//!
//! We own exactly one managed model definition — `[model.swerve-endpoint]` —
//! carrying the user's `base_url` and `model`, and (while enabled) point
//! `[models] default` at it. The endpoint's API key is **not** written here: the
//! block references an `env_key`, and the key is injected into grok's process
//! environment at launch (see `providers::resolve_launch`). Grok's auth hierarchy
//! (`model.api_key > model.env_key > active session token > XAI_API_KEY`) means
//! that key wins, so a custom endpoint needs no xAI sign-in — the air-gapped path.
//!
//! The user's `config.toml` is backed up once to `config.toml.swerve-backup`
//! before the first modification, and edits preserve their existing content,
//! comments, and formatting (only our managed block and the `default` key change).

use std::fs;
use std::path::PathBuf;
use toml_edit::{value, DocumentMut, Item, Table};

/// The model id we own inside the user's config.toml.
pub const MODEL_ID: &str = "swerve-endpoint";
/// The env var grok reads for the endpoint API key (referenced as `env_key`).
pub const API_KEY_ENV: &str = "SWERVE_GROK_API_KEY";
/// Prefix for app-managed local-model blocks (`[model.swerve-local-<slug>]`).
pub const LOCAL_PREFIX: &str = "swerve-local-";
/// The env var carrying the app-generated token for the local llama-server
/// (distinct from the endpoint key — both can be live at once).
pub const LOCAL_API_KEY_ENV: &str = "SWERVE_LOCAL_API_KEY";

/// The fields Swerve manages for the custom endpoint. Strings are pre-trimmed by
/// the caller.
pub struct EndpointSpec<'a> {
    pub enabled: bool,
    pub base_url: &'a str,
    pub model: &'a str,
    pub api_backend: Option<&'a str>,
    pub context_window: Option<u32>,
}

fn config_path() -> PathBuf {
    crate::grok_home().join("config.toml")
}

pub fn config_file_display() -> String {
    config_path().display().to_string()
}

fn load_doc() -> Result<DocumentMut, String> {
    let path = config_path();
    if !path.is_file() {
        return Ok(DocumentMut::new());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read config.toml: {e}"))?;
    raw.parse::<DocumentMut>()
        .map_err(|e| format!("~/.grok/config.toml isn't valid TOML ({e}). Fix or remove it and try again."))
}

/// Copy `config.toml` → `config.toml.swerve-backup` the first time we touch it, so
/// a user who hand-edited theirs always has the original to fall back to.
fn backup_once() {
    let path = config_path();
    if !path.is_file() {
        return;
    }
    let backup = path.with_file_name("config.toml.swerve-backup");
    if !backup.exists() {
        let _ = fs::copy(&path, &backup);
    }
}

fn save_doc(doc: &DocumentMut) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create ~/.grok: {e}"))?;
    }
    crate::paths::write_atomic(&path, doc.to_string().as_bytes())
        .map_err(|e| format!("write config.toml: {e}"))
}

/// Write/refresh the managed `[model.swerve-endpoint]` block and set or restore
/// `[models] default` per `spec.enabled`. Returns the `[models] default` value we
/// displaced when enabling (so the caller can persist it and restore on disable),
/// or `None`.
///
/// `previous_default` is only consulted when disabling — it's the value to put
/// back if the current default is still ours.
pub fn apply(spec: &EndpointSpec, previous_default: Option<&str>) -> Result<Option<String>, String> {
    backup_once();
    let mut doc = load_doc()?;
    let displaced = transform(&mut doc, spec, previous_default);
    save_doc(&doc)?;
    Ok(displaced)
}

/// Pure document transform — the load-bearing logic, split out from I/O so it can
/// be tested without touching the real `~/.grok`. Mutates `doc` in place and
/// returns the `[models] default` we displaced when enabling (else `None`).
fn transform(
    doc: &mut DocumentMut,
    spec: &EndpointSpec,
    previous_default: Option<&str>,
) -> Option<String> {
    // Rebuild our managed model table from scratch (it's fully owned by Swerve).
    let mut tbl = Table::new();
    tbl["model"] = value(spec.model);
    tbl["base_url"] = value(spec.base_url);
    tbl["name"] = value("Swerve Endpoint");
    tbl["env_key"] = value(API_KEY_ENV);
    if let Some(backend) = spec.api_backend.filter(|b| !b.is_empty()) {
        tbl["api_backend"] = value(backend);
    }
    if let Some(cw) = spec.context_window {
        tbl["context_window"] = value(cw as i64);
    }

    if !doc.contains_key("model") || !doc["model"].is_table() {
        let mut parent = Table::new();
        parent.set_implicit(true); // render `[model.swerve-endpoint]`, not a bare `[model]`
        doc["model"] = Item::Table(parent);
    }
    doc["model"][MODEL_ID] = Item::Table(tbl);

    let mut displaced = None;
    if spec.enabled {
        let current = doc
            .get("models")
            .and_then(|m| m.get("default"))
            .and_then(|d| d.as_str())
            .map(str::to_string);
        if let Some(cur) = current {
            if cur != MODEL_ID {
                displaced = Some(cur);
            }
        }
        if !doc.contains_key("models") || !doc["models"].is_table() {
            doc["models"] = Item::Table(Table::new());
        }
        doc["models"]["default"] = value(MODEL_ID);
    } else {
        let is_ours = doc
            .get("models")
            .and_then(|m| m.get("default"))
            .and_then(|d| d.as_str())
            == Some(MODEL_ID);
        if is_ours {
            if let Some(models) = doc.get_mut("models").and_then(|m| m.as_table_mut()) {
                match previous_default {
                    Some(prev) if !prev.is_empty() => {
                        models["default"] = value(prev);
                    }
                    _ => {
                        models.remove("default");
                    }
                }
            }
        }
    }

    displaced
}

/// Sync the app-managed local-model blocks (`[model.swerve-local-<slug>]`) to
/// exactly `entries` (slug, label): upsert each against the app's local server,
/// and remove stale `swerve-local-*` blocks no longer registered. Only blocks
/// under our prefix are ever touched — the user's own models and the
/// `swerve-endpoint` block are left alone.
pub fn apply_local_models(entries: &[(String, String)], port: u16) -> Result<(), String> {
    backup_once();
    let mut doc = load_doc()?;
    transform_local(&mut doc, entries, port);
    save_doc(&doc)
}

/// Pure document transform for local-model blocks — split from I/O for tests.
fn transform_local(doc: &mut DocumentMut, entries: &[(String, String)], port: u16) {
    if !doc.contains_key("model") || !doc["model"].is_table() {
        let mut parent = Table::new();
        parent.set_implicit(true);
        doc["model"] = Item::Table(parent);
    }
    let base_url = format!("http://127.0.0.1:{port}/v1");

    if let Some(models) = doc["model"].as_table_mut() {
        // Drop stale blocks in our namespace only.
        let stale: Vec<String> = models
            .iter()
            .map(|(k, _)| k.to_string())
            .filter(|k| k.starts_with(LOCAL_PREFIX) && !entries.iter().any(|(slug, _)| k == slug))
            .collect();
        for key in stale {
            models.remove(&key);
        }
    }

    for (slug, label) in entries {
        let mut tbl = Table::new();
        // llama-server is started with `--model-alias <slug>`, so the model
        // field matches what the server reports.
        tbl["model"] = value(slug.as_str());
        tbl["base_url"] = value(base_url.as_str());
        tbl["name"] = value(label.as_str());
        tbl["env_key"] = value(LOCAL_API_KEY_ENV);
        tbl["api_backend"] = value("chat_completions");
        doc["model"][slug.as_str()] = Item::Table(tbl);
    }
}

/// Read back `config.toml` and report what Swerve wrote — used by the Settings
/// "Test" button. Returns `(ok, human message)`.
pub fn verify() -> (bool, String) {
    let path = config_path();
    if !path.is_file() {
        return (
            false,
            format!("No Grok config at {} yet — Save to create it.", path.display()),
        );
    }
    let doc = match load_doc() {
        Ok(d) => d,
        Err(e) => return (false, e),
    };

    let block = doc.get("model").and_then(|m| m.get(MODEL_ID));
    let Some(block) = block else {
        return (
            false,
            "Endpoint isn't in config.toml yet — Save first.".to_string(),
        );
    };
    let base = block.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
    let routed = doc
        .get("models")
        .and_then(|m| m.get("default"))
        .and_then(|d| d.as_str())
        == Some(MODEL_ID);

    let routing = if routed {
        "Grok's default model points here (routing on)."
    } else {
        "Saved, but not set as Grok's default (routing off)."
    };
    (
        true,
        format!("[model.{MODEL_ID}] → {base}. {routing} Config: {}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(enabled: bool) -> EndpointSpec<'static> {
        EndpointSpec {
            enabled,
            base_url: "http://localhost:11434/v1",
            model: "qwen2.5-coder:14b",
            api_backend: Some("chat_completions"),
            context_window: Some(32768),
        }
    }

    fn doc(s: &str) -> DocumentMut {
        s.parse::<DocumentMut>().unwrap()
    }

    #[test]
    fn enable_on_empty_writes_block_and_default() {
        let mut d = DocumentMut::new();
        let displaced = transform(&mut d, &spec(true), None);
        assert_eq!(displaced, None);
        let out = d.to_string();
        assert!(out.contains("[model.swerve-endpoint]"), "block header missing:\n{out}");
        assert!(!out.contains("[model]\n"), "bare [model] header should be implicit:\n{out}");
        assert!(out.contains(r#"base_url = "http://localhost:11434/v1""#));
        assert!(out.contains(r#"env_key = "SWERVE_GROK_API_KEY""#));
        assert!(out.contains(r#"api_backend = "chat_completions""#));
        assert!(out.contains("context_window = 32768"));
        assert_eq!(d["models"]["default"].as_str(), Some(MODEL_ID));
    }

    #[test]
    fn enable_preserves_other_content_and_reports_displaced() {
        let mut d = doc(concat!(
            "# my config\n",
            "[model.other]\n",
            "model = \"grok-4\"\n",
            "base_url = \"https://api.x.ai/v1\"\n\n",
            "[models]\n",
            "default = \"other\"\n",
        ));
        let displaced = transform(&mut d, &spec(true), None);
        assert_eq!(displaced, Some("other".to_string()));
        let out = d.to_string();
        assert!(out.contains("# my config"), "comment lost:\n{out}");
        assert!(out.contains("[model.other]"), "other model lost:\n{out}");
        assert!(out.contains(r#"model = "grok-4""#));
        assert!(out.contains("[model.swerve-endpoint]"));
        assert_eq!(d["models"]["default"].as_str(), Some(MODEL_ID));
    }

    #[test]
    fn disable_restores_previous_default() {
        let mut d = DocumentMut::new();
        transform(&mut d, &spec(true), None); // enabled, default = ours
        let displaced = transform(&mut d, &spec(false), Some("other"));
        assert_eq!(displaced, None);
        assert_eq!(d["models"]["default"].as_str(), Some("other"));
        assert!(d.to_string().contains("[model.swerve-endpoint]")); // block stays, just not default
    }

    #[test]
    fn disable_without_previous_removes_default() {
        let mut d = DocumentMut::new();
        transform(&mut d, &spec(true), None);
        transform(&mut d, &spec(false), None);
        let has_default = d
            .get("models")
            .and_then(|m| m.as_table())
            .map(|m| m.contains_key("default"))
            .unwrap_or(false);
        assert!(!has_default, "default should be removed:\n{}", d.to_string());
    }

    #[test]
    fn disable_leaves_foreign_default_untouched() {
        let mut d = doc("[models]\ndefault = \"grok-build\"\n");
        let displaced = transform(&mut d, &spec(false), Some("whatever"));
        assert_eq!(displaced, None);
        assert_eq!(d["models"]["default"].as_str(), Some("grok-build"));
    }

    fn locals(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn local_blocks_written_with_port_and_env() {
        let mut d = DocumentMut::new();
        transform_local(&mut d, &locals(&[("swerve-local-qwen", "Qwen Coder")]), 43117);
        let out = d.to_string();
        assert!(out.contains("[model.swerve-local-qwen]"), "{out}");
        assert!(out.contains(r#"base_url = "http://127.0.0.1:43117/v1""#), "{out}");
        assert!(out.contains(r#"env_key = "SWERVE_LOCAL_API_KEY""#), "{out}");
        assert!(out.contains(r#"api_backend = "chat_completions""#), "{out}");
        assert!(!out.contains("[model]\n"), "parent should stay implicit:\n{out}");
    }

    #[test]
    fn local_sync_removes_stale_ours_only() {
        let mut d = doc(concat!(
            "[model.swerve-local-old]\nmodel = \"old\"\n\n",
            "[model.swerve-endpoint]\nmodel = \"e\"\n\n",
            "[model.users-own]\nmodel = \"keep\"\n# hand comment\n",
        ));
        transform_local(&mut d, &locals(&[("swerve-local-new", "New")]), 5000);
        let out = d.to_string();
        assert!(!out.contains("swerve-local-old"), "stale block kept:\n{out}");
        assert!(out.contains("[model.swerve-local-new]"), "{out}");
        assert!(out.contains("[model.swerve-endpoint]"), "endpoint clobbered:\n{out}");
        assert!(out.contains("[model.users-own]"), "user block clobbered:\n{out}");
        assert!(out.contains("# hand comment"), "user comment lost:\n{out}");
    }

    #[test]
    fn local_sync_empty_clears_namespace() {
        let mut d = doc("[model.swerve-local-a]\nmodel = \"a\"\n\n[model.other]\nmodel = \"o\"\n");
        transform_local(&mut d, &[], 5000);
        let out = d.to_string();
        assert!(!out.contains("swerve-local-a"), "{out}");
        assert!(out.contains("[model.other]"), "{out}");
    }

    // Exercises the real file-IO path (`apply`) end to end against a temp
    // `$GROK_HOME`, so it never touches the developer's real `~/.grok`. This is
    // the only test that sets `GROK_HOME`; the `transform` tests above don't read
    // it, so parallel execution is safe.
    #[test]
    fn apply_roundtrips_real_file_under_temp_home() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("swerve-grok-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("GROK_HOME", &tmp);

        let cfg = tmp.join("config.toml");
        fs::write(&cfg, "[models]\ndefault = \"grok-build\"\n# hand-written\n").unwrap();

        // Enable: block written, default flipped, original captured + backed up.
        let displaced = apply(&spec(true), None).unwrap();
        assert_eq!(displaced, Some("grok-build".to_string()));
        let written = fs::read_to_string(&cfg).unwrap();
        assert!(written.contains("[model.swerve-endpoint]"), "{written}");
        assert!(written.contains("# hand-written"), "comment lost: {written}");
        assert!(written.contains("default = \"swerve-endpoint\""), "{written}");
        let backup = tmp.join("config.toml.swerve-backup");
        assert!(fs::read_to_string(&backup).unwrap().contains("default = \"grok-build\""));

        // Disable: previous default restored.
        apply(&spec(false), Some("grok-build")).unwrap();
        assert!(fs::read_to_string(&cfg).unwrap().contains("default = \"grok-build\""));

        std::env::remove_var("GROK_HOME");
        let _ = fs::remove_dir_all(&tmp);
    }
}
