//! Phase 3 — curated local-model catalog + resumable HuggingFace downloads.
//!
//! Catalog is compile-time (refreshed when we bump entries). Downloads use
//! `curl.exe -C -` (same pattern as the llama-server engine install) so large
//! GGUFs can resume after network blips. On success the file is registered via
//! `providers::add_local_model`. Spec: `docs/local-models-plan.md` Phase 3.

use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// One curated catalog row. Sizes/VRAM are honest estimates for agentic use
/// with the app's default context (`local_llm` CTX_TOKENS), not bare chat.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    /// Stable id used by the UI / download command (not the swerve-local-* id).
    pub id: &'static str,
    pub label: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub size_bytes: u64,
    pub quant: &'static str,
    pub context_tokens: u32,
    /// Recommended free VRAM (GB) for comfortable GPU offload + KV.
    pub vram_gb: f32,
    pub license: &'static str,
    pub good_at: &'static str,
}

/// Hardware fit for a catalog entry given detected VRAM (or lack of GPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FitTier {
    Fits,
    Tight,
    TooBig,
    /// No nvidia-smi — CPU path; still downloadable but slow for large models.
    CpuOnly,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntryView {
    pub id: String,
    pub label: String,
    pub filename: String,
    pub size_bytes: u64,
    pub quant: String,
    pub context_tokens: u32,
    pub vram_gb: f32,
    pub license: String,
    pub good_at: String,
    pub fit: FitTier,
    /// Already registered (path matches dest or same filename under models dir).
    pub installed: bool,
    pub dest_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogState {
    pub models_dir: String,
    pub free_bytes: Option<u64>,
    pub vram_mb: Option<u32>,
    pub entries: Vec<CatalogEntryView>,
    pub downloading: Option<String>,
}

/// Static catalog — 3 agentic-capable coder GGUFs, non-gated HF resolves.
/// Refresh deliberately when bumping; prefer Q4_K_M as the quality/size default.
pub fn catalog() -> &'static [CatalogEntry] {
    &CATALOG
}

static CATALOG: [CatalogEntry; 3] = [
    CatalogEntry {
        id: "qwen25-coder-1.5b-q4km",
        label: "Qwen2.5-Coder 1.5B (Q4_K_M)",
        filename: "Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf",
        size_bytes: 1_120_000_000,
        quant: "Q4_K_M",
        context_tokens: 32_768,
        vram_gb: 2.5,
        license: "Apache-2.0 (base)",
        good_at: "Light coding on low VRAM / CPU; quick drafts. Not for hard multi-file agent turns.",
    },
    CatalogEntry {
        id: "qwen25-coder-7b-q4km",
        label: "Qwen2.5-Coder 7B (Q4_K_M)",
        filename: "Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        size_bytes: 4_680_000_000,
        quant: "Q4_K_M",
        context_tokens: 32_768,
        vram_gb: 7.0,
        license: "Apache-2.0 (base)",
        good_at: "Default local agent model: tools + multi-file edits when VRAM ≥ ~8 GB.",
    },
    CatalogEntry {
        id: "qwen25-coder-14b-q4km",
        label: "Qwen2.5-Coder 14B (Q4_K_M)",
        filename: "Qwen2.5-Coder-14B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-Coder-14B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-14B-Instruct-Q4_K_M.gguf",
        size_bytes: 9_000_000_000,
        quant: "Q4_K_M",
        context_tokens: 32_768,
        vram_gb: 12.0,
        license: "Apache-2.0 (base)",
        good_at: "Stronger reasoning for agentic coding on 16 GB+ GPUs; slower to load.",
    },
];

static DOWNLOAD_BUSY: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_ID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Default models folder: `~/.swervebuild/models/`.
pub fn default_models_dir() -> PathBuf {
    crate::paths::data_dir().join("models")
}

/// Resolved models directory (persisted override or default).
pub fn models_dir() -> PathBuf {
    let store = crate::providers::ProviderStore::load();
    store
        .local
        .models_dir
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(default_models_dir)
}

pub fn set_models_dir(path: String) -> Result<PathBuf, String> {
    let p = PathBuf::from(path.trim());
    if p.as_os_str().is_empty() {
        return Err("Models folder path is empty.".into());
    }
    fs::create_dir_all(&p).map_err(|e| format!("create models dir: {e}"))?;
    let mut store = crate::providers::ProviderStore::load();
    store.local.models_dir = Some(p.display().to_string());
    store.save()?;
    Ok(p)
}

/// Free bytes on the volume that holds `dir` (Windows via PowerShell).
pub fn free_space_bytes(dir: &Path) -> Option<u64> {
    let path = dir.display().to_string();
    // Use .NET drive info from the path root (works even if dir does not exist yet).
    let output = crate::util::hidden_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "$root = [System.IO.Path]::GetPathRoot('{}'); \
                 $d = New-Object System.IO.DriveInfo $root; \
                 if ($d.IsReady) {{ [string][uint64]$d.AvailableFreeSpace }} else {{ '' }}",
                ps_quote(&path),
            ),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    s.parse().ok()
}

fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// Total VRAM in MiB from `nvidia-smi` when present (sum of GPUs).
pub fn detect_vram_mb() -> Option<u32> {
    detect_vram_usage().map(|u| u.total_mb)
}

/// Live VRAM snapshot from `nvidia-smi` (S17). Sums all GPUs.
/// Returns `None` when nvidia-smi is missing or fails — callers must show honest `—`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VramUsage {
    pub used_mb: u32,
    pub total_mb: u32,
}

pub fn detect_vram_usage() -> Option<VramUsage> {
    let output = crate::util::hidden_command("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut used: u32 = 0;
    let mut total: u32 = 0;
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // "1234, 16384" or "1234,16384"
        let mut parts = t.split(',').map(|p| p.trim());
        let u: u32 = parts.next()?.parse().ok()?;
        let tot: u32 = parts.next()?.parse().ok()?;
        used = used.saturating_add(u);
        total = total.saturating_add(tot);
    }
    if total == 0 {
        None
    } else {
        Some(VramUsage {
            used_mb: used,
            total_mb: total,
        })
    }
}

/// Catalog estimate for a loaded model id, if we know it.
pub fn estimate_vram_gb_for_model(model_id: &str) -> Option<f32> {
    catalog()
        .iter()
        .find(|e| e.id == model_id)
        .map(|e| e.vram_gb)
}

/// Pure fit policy (unit-tested).
pub fn classify_fit(vram_mb: Option<u32>, need_gb: f32) -> FitTier {
    let Some(mb) = vram_mb else {
        return FitTier::CpuOnly;
    };
    let have_gb = mb as f32 / 1024.0;
    if have_gb >= need_gb {
        FitTier::Fits
    } else if have_gb >= need_gb * 0.75 {
        FitTier::Tight
    } else {
        FitTier::TooBig
    }
}

fn entry_by_id(id: &str) -> Option<&'static CatalogEntry> {
    catalog().iter().find(|e| e.id == id)
}

fn dest_for(entry: &CatalogEntry) -> PathBuf {
    models_dir().join(entry.filename)
}

fn is_installed(entry: &CatalogEntry) -> bool {
    let dest = dest_for(entry);
    if dest.is_file() {
        // Partial downloads leave a short file — only count as installed if
        // within 2% of expected size (or larger, in case HF grew slightly).
        if let Ok(meta) = fs::metadata(&dest) {
            let n = meta.len();
            let min = entry.size_bytes.saturating_mul(98) / 100;
            if n >= min {
                // Also registered?
                let store = crate::providers::ProviderStore::load();
                return store
                    .local
                    .models
                    .iter()
                    .any(|m| Path::new(&m.path) == dest || m.path == dest.display().to_string());
            }
        }
    }
    // Registered elsewhere with same filename stem still counts.
    let store = crate::providers::ProviderStore::load();
    store.local.models.iter().any(|m| {
        Path::new(&m.path)
            .file_name()
            .and_then(|n| n.to_str())
            == Some(entry.filename)
    })
}

pub fn catalog_state() -> CatalogState {
    let dir = models_dir();
    let free = free_space_bytes(&dir);
    let vram = detect_vram_mb();
    let downloading = DOWNLOAD_ID
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .filter(|_| DOWNLOAD_BUSY.load(Ordering::SeqCst));
    let entries = catalog()
        .iter()
        .map(|e| CatalogEntryView {
            id: e.id.to_string(),
            label: e.label.to_string(),
            filename: e.filename.to_string(),
            size_bytes: e.size_bytes,
            quant: e.quant.to_string(),
            context_tokens: e.context_tokens,
            vram_gb: e.vram_gb,
            license: e.license.to_string(),
            good_at: e.good_at.to_string(),
            fit: classify_fit(vram, e.vram_gb),
            installed: is_installed(e),
            dest_path: dest_for(e).display().to_string(),
        })
        .collect();
    CatalogState {
        models_dir: dir.display().to_string(),
        free_bytes: free,
        vram_mb: vram,
        entries,
        downloading,
    }
}

fn emit_progress(app: &AppHandle, catalog_id: &str, phase: &str, received: u64, total: u64) {
    let _ = app.emit(
        "local-model-download-progress",
        json!({
            "catalogId": catalog_id,
            "phase": phase,
            "received": received,
            "total": total,
        }),
    );
}

/// Download a catalog GGUF (resumable), verify size, register it.
/// Blocking — call from `spawn_blocking`.
pub fn download_catalog_model(app: &AppHandle, catalog_id: &str) -> Result<String, String> {
    let entry = entry_by_id(catalog_id)
        .ok_or_else(|| format!("Unknown catalog model: {catalog_id}"))?;

    if DOWNLOAD_BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let who = DOWNLOAD_ID
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| "another model".into());
        return Err(format!("Already downloading {who}. Wait for it to finish."));
    }
    if let Ok(mut g) = DOWNLOAD_ID.lock() {
        *g = Some(catalog_id.to_string());
    }

    let result = download_inner(app, entry);

    DOWNLOAD_BUSY.store(false, Ordering::SeqCst);
    if let Ok(mut g) = DOWNLOAD_ID.lock() {
        *g = None;
    }
    result
}

fn download_inner(app: &AppHandle, entry: &CatalogEntry) -> Result<String, String> {
    let dir = models_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create models dir: {e}"))?;

    if let Some(free) = free_space_bytes(&dir) {
        // Leave 1 GB headroom for OS + partial file.
        let need = entry.size_bytes.saturating_add(1_000_000_000);
        if free < need {
            return Err(format!(
                "Not enough free space in {}. Need ~{:.1} GB free, have ~{:.1} GB.",
                dir.display(),
                need as f64 / 1e9,
                free as f64 / 1e9
            ));
        }
    }

    let dest = dest_for(entry);
    let partial = dest.with_extension("gguf.partial");

    // Resume into .partial; rename when complete.
    if dest.is_file() {
        if let Ok(meta) = fs::metadata(&dest) {
            let min = entry.size_bytes.saturating_mul(98) / 100;
            if meta.len() >= min {
                // Already on disk — just ensure registered.
                crate::providers::add_local_model(dest.display().to_string())?;
                emit_progress(app, entry.id, "done", entry.size_bytes, entry.size_bytes);
                return Ok(format!("{} already present — registered.", entry.label));
            }
        }
    }

    // If a previous full file failed size check, start from partial.
    if dest.is_file() && !partial.is_file() {
        let _ = fs::rename(&dest, &partial);
    }

    let done = Arc::new(AtomicBool::new(false));
    let done_bg = Arc::clone(&done);
    let app_bg = app.clone();
    let partial_bg = partial.clone();
    let catalog_id = entry.id.to_string();
    let total = entry.size_bytes;
    let reporter = thread::spawn(move || {
        while !done_bg.load(Ordering::SeqCst) {
            let received = fs::metadata(&partial_bg).map(|m| m.len()).unwrap_or(0);
            emit_progress(&app_bg, &catalog_id, "downloading", received, total);
            thread::sleep(Duration::from_millis(500));
        }
    });

    // curl -C - resumes; -L follows HF redirects; --fail surfaces HTTP errors.
    let status = crate::util::hidden_command("curl.exe")
        .args([
            "-L",
            "--fail",
            "--retry",
            "3",
            "--retry-delay",
            "2",
            "-C",
            "-",
            "-o",
            &partial.display().to_string(),
            entry.url,
        ])
        .status();
    done.store(true, Ordering::SeqCst);
    let _ = reporter.join();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            return Err(format!(
                "download failed (curl exit {:?}). Partial file kept for resume.",
                s.code()
            ));
        }
        Err(e) => return Err(format!("could not run curl.exe: {e}")),
    }

    let got = fs::metadata(&partial)
        .map(|m| m.len())
        .map_err(|e| format!("stat download: {e}"))?;
    let min = entry.size_bytes.saturating_mul(95) / 100;
    // HF may revise file size slightly; require at least 95% of catalog estimate
    // and at least 100 MB so we never register a truncated stub.
    if got < min || got < 100_000_000 {
        return Err(format!(
            "download looks incomplete ({got} bytes, expected ~{}). Partial kept for resume.",
            entry.size_bytes
        ));
    }

    emit_progress(app, entry.id, "registering", got, got);
    fs::rename(&partial, &dest).map_err(|e| format!("finalize download: {e}"))?;
    crate::providers::add_local_model(dest.display().to_string())?;
    emit_progress(app, entry.id, "done", got, got);
    Ok(format!(
        "{} downloaded ({:.1} GB) and added to pickers.",
        entry.label,
        got as f64 / 1e9
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_three_unique_ids() {
        let ids: Vec<_> = catalog().iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 3);
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
        for e in catalog() {
            assert!(e.size_bytes > 500_000_000, "{} too small", e.id);
            assert!(e.url.starts_with("https://huggingface.co/"));
            assert!(e.filename.ends_with(".gguf"));
        }
    }

    #[test]
    fn estimate_known_catalog_model() {
        let id = catalog()[0].id;
        assert_eq!(estimate_vram_gb_for_model(id), Some(catalog()[0].vram_gb));
        assert!(estimate_vram_gb_for_model("not-a-real-model-id").is_none());
    }

    #[test]
    fn fit_fits_when_vram_above_need() {
        // 24 GB card, 7 GB need
        assert_eq!(classify_fit(Some(24_576), 7.0), FitTier::Fits);
    }

    #[test]
    fn fit_tight_near_boundary() {
        // 6 GB card, 7 GB need → 6/7 ≈ 0.86 > 0.75 → Tight
        assert_eq!(classify_fit(Some(6 * 1024), 7.0), FitTier::Tight);
    }

    #[test]
    fn fit_too_big_when_far_under() {
        // 4 GB card, 12 GB need
        assert_eq!(classify_fit(Some(4 * 1024), 12.0), FitTier::TooBig);
    }

    #[test]
    fn fit_cpu_only_without_smi() {
        assert_eq!(classify_fit(None, 7.0), FitTier::CpuOnly);
    }

    #[test]
    fn default_models_dir_under_swervebuild() {
        let d = default_models_dir();
        assert!(d.ends_with("models"));
        assert!(d.to_string_lossy().contains(".swervebuild"));
    }

    /// Live smoke: builds catalog views (touches providers.json + nvidia-smi).
    #[test]
    fn live_catalog_state_returns_three_rows() {
        let state = catalog_state();
        assert_eq!(state.entries.len(), 3);
        assert!(!state.models_dir.is_empty());
        for e in &state.entries {
            assert!(!e.label.is_empty());
            assert!(!e.dest_path.is_empty());
        }
    }
}
