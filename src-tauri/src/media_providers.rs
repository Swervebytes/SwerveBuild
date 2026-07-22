//! Media generation providers (S16) — separate from chat/agent models.
//!
//! Chat model (local Qwen, Grok, …) only does text + tool *decisions*.
//! Image/video tools are routed to a media provider (today: xAI Imagine remote).
//! Local image/video backends are reserved slots — not fabricated as available.

use crate::store::{AppPreferences, Store};
use serde::{Deserialize, Serialize};

pub const IMAGE_PROVIDER_IMAGINE: &str = "imagine";
/// Reserved for a future local diffusion / Comfy path (S18+). Not selectable yet.
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
}

fn image_catalog() -> Vec<MediaProviderInfo> {
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
            label: "Local image (planned)".into(),
            kind: "image".into(),
            locality: "local".into(),
            available: false,
            note: "Offline gen planned after VRAM UI — not installed".into(),
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
        // Local not selectable yet — fall back to Imagine.
        IMAGE_PROVIDER_LOCAL => IMAGE_PROVIDER_IMAGINE.into(),
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

pub fn view() -> MediaProvidersView {
    let prefs = load_preferences();
    let image_id = normalize_image_provider_id(&prefs.image_provider_id);
    let video_id = normalize_video_provider_id(&prefs.video_provider_id);
    MediaProvidersView {
        image_providers: image_catalog(),
        video_providers: video_catalog(),
        selected_image_provider_id: image_id,
        selected_video_provider_id: video_id,
    }
}

pub fn set_image_provider(id: &str) -> Result<MediaProvidersView, String> {
    let catalog = image_catalog();
    let entry = catalog
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Unknown image provider: {id}"))?;
    if !entry.available {
        return Err(format!(
            "{} is not available yet — {}",
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

/// One-line summary for env context / UI tooltips.
pub fn honesty_summary() -> String {
    let v = view();
    let img = v
        .image_providers
        .iter()
        .find(|p| p.id == v.selected_image_provider_id);
    let vid = v
        .video_providers
        .iter()
        .find(|p| p.id == v.selected_video_provider_id);
    let img_s = img
        .map(|p| format!("{} ({})", p.label, p.locality))
        .unwrap_or_else(|| "unknown".into());
    let vid_s = vid
        .map(|p| format!("{} ({})", p.label, p.locality))
        .unwrap_or_else(|| "unknown".into());
    format!(
        "images={img_s}; video={vid_s}; chat model does NOT render pixels — image_gen/image_edit use the image provider"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_falls_back_unknown_and_local() {
        assert_eq!(normalize_image_provider_id("imagine"), "imagine");
        assert_eq!(normalize_image_provider_id("local"), "imagine");
        assert_eq!(normalize_image_provider_id("nope"), "imagine");
        assert_eq!(normalize_image_provider_id(""), "imagine");
    }

    #[test]
    fn catalog_has_imagine_available_local_not() {
        let imgs = image_catalog();
        let imagine = imgs.iter().find(|p| p.id == "imagine").unwrap();
        let local = imgs.iter().find(|p| p.id == "local").unwrap();
        assert!(imagine.available);
        assert!(!local.available);
        assert_eq!(imagine.locality, "remote");
        assert_eq!(local.locality, "local");
    }

    #[test]
    fn honesty_summary_mentions_pixels() {
        let s = honesty_summary();
        assert!(s.contains("image_gen"), "{s}");
        assert!(s.contains("Imagine") || s.contains("imagine") || s.contains("images="), "{s}");
    }
}
