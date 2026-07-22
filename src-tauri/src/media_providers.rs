//! Media generation providers (S16 + S18) — separate from chat/agent models.
//!
//! Chat model (local Qwen, Grok, …) only does text + tool *decisions*.
//! Image tools use a media provider: xAI Imagine (remote) or ComfyUI (local).

use crate::local_image;
use crate::store::{AppPreferences, Store};
use serde::{Deserialize, Serialize};

pub const IMAGE_PROVIDER_IMAGINE: &str = "imagine";
pub const IMAGE_PROVIDER_LOCAL: &str = "local";

pub const VIDEO_PROVIDER_IMAGINE: &str = "imagine";
pub const VIDEO_PROVIDER_LOCAL: &str = "local";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProviderInfo {
    pub id: String,
    pub label: String,
    /// `"image"` | `"video"`
    pub kind: String,
    /// `"remote"` | `"local"`
    pub locality: String,
    /// Whether the operator can select it *now*.
    pub available: bool,
    /// Short status for UI (e.g. "needs network", "coming later").
    pub note: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProvidersView {
    pub image_providers: Vec<MediaProviderInfo>,
    pub video_providers: Vec<MediaProviderInfo>,
    pub selected_image_provider_id: String,
    pub selected_video_provider_id: String,
    pub local_image: local_image::LocalImageStatus,
}

fn image_catalog(local: &local_image::LocalImageStatus) -> Vec<MediaProviderInfo> {
    vec![
        MediaProviderInfo {
            id: IMAGE_PROVIDER_IMAGINE.into(),
            label: "xAI Imagine".into(),
            kind: "image".into(),
            locality: "remote".into(),
            available: true,
            note: "Remote API — needs network; not your chat model".into(),
            is_default: true,
        },
        MediaProviderInfo {
            id: IMAGE_PROVIDER_LOCAL.into(),
            label: "Local (ComfyUI)".into(),
            kind: "image".into(),
            locality: "local".into(),
            available: local.reachable,
            note: local.note.clone(),
            is_default: false,
        },
    ]
}

fn video_catalog() -> Vec<MediaProviderInfo> {
    vec![
        MediaProviderInfo {
            id: VIDEO_PROVIDER_IMAGINE.into(),
            label: "xAI Imagine video".into(),
            kind: "video".into(),
            locality: "remote".into(),
            available: true,
            note: "Remote tools (image_to_video / …) — needs network".into(),
            is_default: true,
        },
        MediaProviderInfo {
            id: VIDEO_PROVIDER_LOCAL.into(),
            label: "Local video (planned)".into(),
            kind: "video".into(),
            locality: "local".into(),
            available: false,
            note: "After local image path is stable".into(),
            is_default: false,
        },
    ]
}

pub fn normalize_image_provider_id(id: &str) -> String {
    match id {
        IMAGE_PROVIDER_IMAGINE => IMAGE_PROVIDER_IMAGINE.into(),
        IMAGE_PROVIDER_LOCAL => IMAGE_PROVIDER_LOCAL.into(),
        other if other.is_empty() => IMAGE_PROVIDER_IMAGINE.into(),
        _ => IMAGE_PROVIDER_IMAGINE.into(),
    }
}

pub fn normalize_video_provider_id(id: &str) -> String {
    match id {
        VIDEO_PROVIDER_IMAGINE => VIDEO_PROVIDER_IMAGINE.into(),
        _ => VIDEO_PROVIDER_IMAGINE.into(),
    }
}

pub fn load_preferences() -> AppPreferences {
    Store::load().preferences
}

/// UI listing. Uses cached Comfy probe (short TTL) so chat/header opens stay snappy.
pub fn view() -> MediaProvidersView {
    view_inner(false)
}

/// Force a fresh Comfy probe (Probe button / after URL change).
pub fn view_refresh() -> MediaProvidersView {
    view_inner(true)
}

fn view_inner(force_probe: bool) -> MediaProvidersView {
    let prefs = load_preferences();
    let local = if force_probe {
        local_image::probe_fresh()
    } else {
        // Full probe when cache cold; otherwise TTL cache (see local_image).
        local_image::probe()
    };
    let catalog = image_catalog(&local);
    let mut image_id = normalize_image_provider_id(&prefs.image_provider_id);
    // If user selected local but Comfy is down, still report selection; UI shows offline note.
    if image_id == IMAGE_PROVIDER_LOCAL
        && !catalog
            .iter()
            .any(|p| p.id == IMAGE_PROVIDER_LOCAL && p.available)
    {
        // keep selection as local so reconnect restores without re-pick
    }
    if !catalog.iter().any(|p| p.id == image_id) {
        image_id = IMAGE_PROVIDER_IMAGINE.into();
    }
    let video_id = normalize_video_provider_id(&prefs.video_provider_id);
    MediaProvidersView {
        image_providers: catalog,
        video_providers: video_catalog(),
        selected_image_provider_id: image_id,
        selected_video_provider_id: video_id,
        local_image: local,
    }
}

/// Preference-only snapshot for header summary — **no network**.
pub fn view_prefs_only() -> MediaProvidersView {
    let prefs = load_preferences();
    let base = {
        let u = prefs.comfy_base_url.trim();
        if u.is_empty() {
            local_image::DEFAULT_COMFY_URL.to_string()
        } else {
            u.trim_end_matches('/').to_string()
        }
    };
    // Offline placeholder; open Models sheet / Probe for live reachability.
    let local = local_image::LocalImageStatus {
        reachable: false,
        base_url: base,
        note: "Open Models to probe ComfyUI".into(),
        checkpoints: vec![],
    };
    let catalog = image_catalog(&local);
    // When we don't probe, still show Local as selectable in summary sense —
    // actual availability is refreshed when the sheet loads with a real probe.
    // For prefs-only, mark local available:true so selection isn't forced away;
    // the live view corrects availability.
    let mut image_providers = catalog;
    for p in &mut image_providers {
        if p.id == IMAGE_PROVIDER_LOCAL {
            p.available = true;
            p.note = "Local ComfyUI — probe when Models opens".into();
        }
    }
    let mut image_id = normalize_image_provider_id(&prefs.image_provider_id);
    if !image_providers.iter().any(|p| p.id == image_id) {
        image_id = IMAGE_PROVIDER_IMAGINE.into();
    }
    let video_id = normalize_video_provider_id(&prefs.video_provider_id);
    MediaProvidersView {
        image_providers,
        video_providers: video_catalog(),
        selected_image_provider_id: image_id,
        selected_video_provider_id: video_id,
        local_image: local,
    }
}

pub fn set_image_provider(id: &str) -> Result<MediaProvidersView, String> {
    let local = local_image::probe();
    let catalog = image_catalog(&local);
    let entry = catalog
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Unknown image provider: {id}"))?;
    if !entry.available {
        return Err(format!(
            "{} is not available — {}",
            entry.label, entry.note
        ));
    }
    let _guard = Store::lock();
    let mut store = Store::load();
    store.preferences.image_provider_id = normalize_image_provider_id(id);
    Store::save(&store)?;
    Ok(view())
}

pub fn set_video_provider(id: &str) -> Result<MediaProvidersView, String> {
    let catalog = video_catalog();
    let entry = catalog
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Unknown video provider: {id}"))?;
    if !entry.available {
        return Err(format!(
            "{} is not available yet — {}",
            entry.label, entry.note
        ));
    }
    let _guard = Store::lock();
    let mut store = Store::load();
    store.preferences.video_provider_id = normalize_video_provider_id(id);
    Store::save(&store)?;
    Ok(view())
}

pub fn set_comfy_base_url(url: &str) -> Result<MediaProvidersView, String> {
    let trimmed = url.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Err("Comfy URL is empty".into());
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("Comfy URL must start with http:// or https://".into());
    }
    // Loopback-only for S18 safety (no remote Comfy over WAN without deliberate later work).
    let host_ok = trimmed.contains("127.0.0.1")
        || trimmed.contains("localhost")
        || trimmed.contains("[::1]");
    if !host_ok {
        return Err(
            "S18 allows loopback Comfy only (127.0.0.1 / localhost). Remote hosts later.".into(),
        );
    }
    let _guard = Store::lock();
    let mut store = Store::load();
    store.preferences.comfy_base_url = trimmed;
    Store::save(&store)?;
    local_image::invalidate_probe_cache();
    Ok(view_refresh())
}

/// One-line summary for env context / UI tooltips.
/// **No network** — session start must not wait on Comfy.
pub fn honesty_summary() -> String {
    let prefs = load_preferences();
    let image_id = normalize_image_provider_id(&prefs.image_provider_id);
    let video_id = normalize_video_provider_id(&prefs.video_provider_id);
    let img_s = match image_id.as_str() {
        IMAGE_PROVIDER_LOCAL => "Local (ComfyUI) (local)",
        _ => "xAI Imagine (remote)",
    };
    let vid_s = match video_id.as_str() {
        VIDEO_PROVIDER_LOCAL => "Local video (planned) (local)",
        _ => "xAI Imagine video (remote)",
    };
    let local_hint = if image_id == IMAGE_PROVIDER_LOCAL {
        let base = {
            let u = prefs.comfy_base_url.trim();
            if u.is_empty() {
                local_image::DEFAULT_COMFY_URL
            } else {
                u
            }
        };
        format!(
            "; preferred tool: swervebuild__local_image_generate via {base} (probe at gen time)"
        )
    } else {
        "; Grok image_gen/image_edit = Imagine remote (not local chat model)".into()
    };
    format!("images={img_s}; video={vid_s}; chat model does NOT render pixels{local_hint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_local_id() {
        assert_eq!(normalize_image_provider_id("imagine"), "imagine");
        assert_eq!(normalize_image_provider_id("local"), "local");
        assert_eq!(normalize_image_provider_id("nope"), "imagine");
    }

    #[test]
    fn honesty_summary_mentions_pixels() {
        let s = honesty_summary();
        assert!(s.contains("pixels") || s.contains("images="), "{s}");
    }

    #[test]
    fn catalog_has_both_image_providers() {
        let local = local_image::LocalImageStatus {
            reachable: false,
            base_url: local_image::DEFAULT_COMFY_URL.into(),
            note: "down".into(),
            checkpoints: vec![],
        };
        let imgs = image_catalog(&local);
        assert!(imgs.iter().any(|p| p.id == "imagine" && p.available));
        assert!(imgs.iter().any(|p| p.id == "local" && !p.available));
    }
}
