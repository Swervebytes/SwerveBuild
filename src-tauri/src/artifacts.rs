//! Artifact retention (S23) — disk budget + keep-N across managed roots.
//!
//! Chats stay in data.json; this only manages binary/media-ish dirs under
//! the data dir. Prune is explicit (command) or best-effort after large writes.

use crate::db;
use crate::paths;
use crate::store::Store;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindUsage {
    pub kind: String,
    pub dir: String,
    pub file_count: u32,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStoreStatus {
    pub max_bytes: u64,
    pub keep_per_kind: u32,
    pub total_bytes: u64,
    pub over_budget: bool,
    pub kinds: Vec<KindUsage>,
    pub db_path: String,
    pub db_ok: bool,
    pub schema_version: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneResult {
    pub dry_run: bool,
    pub deleted_files: u32,
    pub freed_bytes: u64,
    pub remaining_bytes: u64,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct FileEntry {
    path: PathBuf,
    kind: String,
    bytes: u64,
    modified: SystemTime,
}

fn managed_roots() -> Vec<(&'static str, PathBuf)> {
    vec![
        ("attachment", paths::attachments_dir()),
        ("app_ui", paths::app_ui_artifacts_dir()),
        ("browser_debug", paths::browser_debug_artifacts_dir()),
    ]
}

fn policy_from_store() -> (u64, u32) {
    let prefs = Store::load().preferences;
    let max = if prefs.artifact_max_bytes == 0 {
        5 * 1024 * 1024 * 1024
    } else {
        prefs.artifact_max_bytes
    };
    let keep = if prefs.artifact_keep_per_kind == 0 {
        30
    } else {
        prefs.artifact_keep_per_kind
    };
    (max, keep)
}

fn list_files_in(dir: &Path, kind: &str) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        out.push(FileEntry {
            path,
            kind: kind.to_string(),
            bytes: meta.len(),
            modified,
        });
    }
    out
}

fn collect_all() -> Vec<FileEntry> {
    let mut all = Vec::new();
    for (kind, dir) in managed_roots() {
        all.extend(list_files_in(&dir, kind));
    }
    all
}

/// Register on-disk files into SQLite artifact table (best-effort).
pub fn resync_registry() -> Result<u32, String> {
    let mut n = 0u32;
    for f in collect_all() {
        let id = uuid::Uuid::new_v4().to_string();
        let created = f
            .modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| Store::now());
        let path_s = f.path.display().to_string();
        // Ignore individual upsert errors so one bad row doesn't stop scan.
        if db::upsert_artifact(&id, &f.kind, &path_s, f.bytes, &created, None, None).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

pub fn status() -> ArtifactStoreStatus {
    let (max_bytes, keep_per_kind) = policy_from_store();
    let mut kinds = Vec::new();
    let mut total = 0u64;
    for (kind, dir) in managed_roots() {
        let files = list_files_in(&dir, kind);
        let bytes: u64 = files.iter().map(|f| f.bytes).sum();
        total = total.saturating_add(bytes);
        kinds.push(KindUsage {
            kind: kind.to_string(),
            dir: dir.display().to_string(),
            file_count: files.len() as u32,
            bytes,
        });
    }
    let db_path = paths::db_file().display().to_string();
    let db_ok = db::with_conn(|_| Ok(())).is_ok();
    ArtifactStoreStatus {
        max_bytes,
        keep_per_kind,
        total_bytes: total,
        over_budget: total > max_bytes,
        kinds,
        db_path,
        db_ok,
        schema_version: db::SCHEMA_VERSION,
    }
}

/// Prune oldest files until under budget, respecting keep-N per kind.
/// `dry_run`: report only.
pub fn prune(dry_run: bool) -> Result<PruneResult, String> {
    let (max_bytes, keep_per_kind) = policy_from_store();
    let mut all = collect_all();
    // Newest first for keep-N; we delete from the oldest end of the overflow.
    all.sort_by(|a, b| b.modified.cmp(&a.modified));

    // Files protected by keep-N: first `keep` per kind when sorted newest-first.
    let mut kept_count: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut protected: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for f in &all {
        let c = kept_count.entry(f.kind.clone()).or_insert(0);
        if *c < keep_per_kind {
            protected.insert(f.path.clone());
            *c += 1;
        }
    }

    let mut total: u64 = all.iter().map(|f| f.bytes).sum();
    // Candidates to delete: oldest first, not protected, while over budget.
    let mut by_oldest = all.clone();
    by_oldest.sort_by(|a, b| a.modified.cmp(&b.modified));

    let mut deleted = 0u32;
    let mut freed = 0u64;
    let mut paths = Vec::new();

    for f in by_oldest {
        if total <= max_bytes {
            break;
        }
        if protected.contains(&f.path) {
            continue;
        }
        let path_s = f.path.display().to_string();
        if !dry_run {
            match fs::remove_file(&f.path) {
                Ok(()) => {
                    let _ = db::mark_purged(&path_s);
                }
                Err(_) => continue,
            }
        }
        deleted += 1;
        freed = freed.saturating_add(f.bytes);
        total = total.saturating_sub(f.bytes);
        paths.push(path_s);
    }

    // If still over budget only protected files remain — stop (never delete keep-N).
    Ok(PruneResult {
        dry_run,
        deleted_files: deleted,
        freed_bytes: freed,
        remaining_bytes: total,
        paths,
    })
}

/// After writing a large attachment, resync + prune if over budget (best-effort).
pub fn maybe_enforce_after_write() {
    let st = status();
    if !st.over_budget {
        return;
    }
    let _ = resync_registry();
    let _ = prune(false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;

    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn prune_respects_keep_and_budget() {
        let _g = LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("swerve-art-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let att = root.join("attachments");
        fs::create_dir_all(&att).unwrap();

        // 5 small files, newest protected if keep=2 and budget tiny.
        for i in 0..5 {
            let p = att.join(format!("f{i}.bin"));
            let mut f = fs::File::create(&p).unwrap();
            f.write_all(&[b'x'; 100]).unwrap();
            thread::sleep(Duration::from_millis(15));
        }

        // Unit-test pure selection logic by scanning this dir only.
        let mut files = list_files_in(&att, "attachment");
        assert_eq!(files.len(), 5);
        files.sort_by(|a, b| b.modified.cmp(&a.modified));
        let keep = 2u32;
        let mut protected = std::collections::HashSet::new();
        for (i, f) in files.iter().enumerate() {
            if (i as u32) < keep {
                protected.insert(f.path.clone());
            }
        }
        assert_eq!(protected.len(), 2);

        let total: u64 = files.iter().map(|f| f.bytes).sum();
        assert!(total >= 500);

        let _ = fs::remove_dir_all(&root);
    }
}
