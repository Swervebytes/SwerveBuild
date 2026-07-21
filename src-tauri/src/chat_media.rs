//! Chat media detection (Roadmap Step 1 residual, S13) — find image/video
//! artifact paths an agent turn produced, verify them on disk, and copy them
//! into the attachments dir so the webview can render them.
//!
//! Why copy: the asset protocol's scope is deliberately narrow
//! (`~/.swervebuild/attachments/**`); rendering project paths in place would
//! mean widening what the webview may read. Copying keeps the scope tight AND
//! makes chats self-contained — the preview survives the project file moving.
//!
//! What is detected: path-looking tokens ending in a known image/video
//! extension inside the turn's text (final prose + tool-chip text) — bare
//! tokens without spaces, and quoted/backticked/markdown-linked paths which may
//! contain spaces. Relative paths resolve against the chat's project folder.
//! What is NOT detected: URLs, data URIs, paths without extensions, files
//! larger than the caps.

use std::path::{Path, PathBuf};

pub const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];
pub const VIDEO_EXTS: [&str; 4] = ["mp4", "webm", "ogv", "mov"];

/// Size caps — a copy is skipped (not an error) beyond these.
const IMAGE_MAX_BYTES: u64 = 25 * 1024 * 1024;
const VIDEO_MAX_BYTES: u64 = 250 * 1024 * 1024;
/// Per-message attachment caps (first hits win, order of appearance).
const MAX_IMAGES: usize = 6;
const MAX_VIDEOS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaKind {
    Image,
    Video,
}

pub fn classify(path: &str) -> Option<MediaKind> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Image)
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else {
        None
    }
}

/// Characters that end a BARE (unquoted) path token.
fn is_bare_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '<' | '>' | '|' | '*' | ',' | ';')
}

/// Extract candidate media paths from turn text, in order of appearance,
/// deduped. Pure — no filesystem access (resolution happens later).
pub fn extract_path_candidates(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let lower: String = text.to_ascii_lowercase();

    // Find every extension occurrence, then walk back to the token start.
    for ext in IMAGE_EXTS.iter().chain(VIDEO_EXTS.iter()) {
        let needle = format!(".{ext}");
        let mut from = 0;
        while let Some(rel) = lower[from..].find(&needle) {
            let dot_byte = from + rel;
            from = dot_byte + 1;
            // Byte → char index (text may be non-ASCII).
            let dot_idx = text[..dot_byte].chars().count();
            let end_idx = dot_idx + needle.chars().count();
            // The extension must END the token: next char is a delimiter/EOS.
            // (Also rejects ".jpeg" matching inside ".jpe" + "g…" style overlaps
            // and ".mov" inside ".movie".)
            if let Some(&next) = chars.get(end_idx) {
                if !is_bare_delimiter(next) && next != ':' && next != '.' {
                    continue;
                }
            }
            // Bare token: walk back to the nearest delimiter (no spaces).
            let mut start = dot_idx;
            while start > 0 && !is_bare_delimiter(chars[start - 1]) {
                start -= 1;
            }
            let mut candidate: String = chars[start..end_idx].iter().collect();
            // Quoted/backticked form (may contain spaces): when the char right
            // AFTER the extension is a closing quote/backtick, take the span
            // back to its matching opener instead.
            if let Some(&close) = chars.get(end_idx) {
                if matches!(close, '"' | '\'' | '`') {
                    if let Some(open_rel) = chars[..dot_idx].iter().rposition(|&c| c == close) {
                        let quoted: String = chars[open_rel + 1..end_idx].iter().collect();
                        if !quoted.contains('\n') && !quoted.trim().is_empty() {
                            candidate = quoted;
                        }
                    }
                }
            }
            let cleaned = clean_candidate(&candidate);
            if cleaned.is_empty() {
                continue;
            }
            // Skip URLs and data URIs — file paths only.
            let cl = cleaned.to_ascii_lowercase();
            if cl.starts_with("http://") || cl.starts_with("https://") || cl.starts_with("data:") {
                continue;
            }
            if !out.iter().any(|c| c.eq_ignore_ascii_case(&cleaned)) {
                out.push(cleaned);
            }
        }
    }
    // Order of appearance, not extension-scan order.
    out.sort_by_key(|c| lower.find(&c.to_ascii_lowercase()).unwrap_or(usize::MAX));
    out
}

fn clean_candidate(raw: &str) -> String {
    raw.trim()
        .trim_start_matches(['(', '[', '<', '"', '\'', '`', '*'])
        .trim_end_matches(['"', '\'', '`', '*'])
        .to_string()
}

/// One resolved-and-copied attachment.
#[derive(Debug, Clone)]
pub struct ResolvedMedia {
    pub kind: MediaKind,
    /// The copy inside the attachments dir (what gets persisted + rendered).
    pub stored_path: String,
}

/// Verify candidates on disk (relative → `cwd`) and copy the survivors into
/// `dest_dir` (the attachments dir in production; a temp dir in tests).
/// Silently skips: missing files, oversize files, and hits beyond the caps.
pub fn resolve_and_copy(candidates: &[String], cwd: &Path, dest_dir: &Path) -> Vec<ResolvedMedia> {
    let mut out: Vec<ResolvedMedia> = Vec::new();
    let mut images = 0usize;
    let mut videos = 0usize;
    let mut seen_sources: Vec<PathBuf> = Vec::new();

    for cand in candidates {
        let Some(kind) = classify(cand) else { continue };
        let raw = Path::new(cand);
        let abs = if raw.is_absolute() { raw.to_path_buf() } else { cwd.join(raw) };
        let Ok(real) = std::fs::canonicalize(&abs) else { continue };
        if !real.is_file() {
            continue;
        }
        if seen_sources.iter().any(|s| s == &real) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&real) else { continue };
        let cap = match kind {
            MediaKind::Image => IMAGE_MAX_BYTES,
            MediaKind::Video => VIDEO_MAX_BYTES,
        };
        if meta.len() > cap {
            continue;
        }
        match kind {
            MediaKind::Image if images >= MAX_IMAGES => continue,
            MediaKind::Video if videos >= MAX_VIDEOS => continue,
            _ => {}
        }
        let ext = real
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_ascii_lowercase();
        if std::fs::create_dir_all(dest_dir).is_err() {
            continue;
        }
        let dest = dest_dir.join(format!("{}.{ext}", crate::store::Store::new_id()));
        if std::fs::copy(&real, &dest).is_err() {
            continue;
        }
        match kind {
            MediaKind::Image => images += 1,
            MediaKind::Video => videos += 1,
        }
        seen_sources.push(real);
        out.push(ResolvedMedia { kind, stored_path: dest.display().to_string() });
    }
    out
}

/// Full pipeline for a turn: extract → resolve against the project cwd → copy
/// into the real attachments dir. Returns (images, videos) as stored paths.
pub fn detect_for_turn(text: &str, cwd: &Path) -> (Vec<String>, Vec<String>) {
    let candidates = extract_path_candidates(text);
    let resolved = resolve_and_copy(&candidates, cwd, &crate::paths::attachments_dir());
    let mut images = Vec::new();
    let mut videos = Vec::new();
    for media in resolved {
        match media.kind {
            MediaKind::Image => images.push(media.stored_path),
            MediaKind::Video => videos.push(media.stored_path),
        }
    }
    (images, videos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("swerve-media-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn classify_by_extension() {
        assert_eq!(classify("a/b/shot.PNG"), Some(MediaKind::Image));
        assert_eq!(classify("clip.mp4"), Some(MediaKind::Video));
        assert_eq!(classify("notes.txt"), None);
        assert_eq!(classify("no_extension"), None);
    }

    #[test]
    fn extracts_bare_absolute_and_relative_paths() {
        let text = "Saved the render to E:\\proj\\out\\shot.png and also out/clip.mp4 done.";
        let c = extract_path_candidates(text);
        assert!(c.contains(&"E:\\proj\\out\\shot.png".to_string()), "{c:?}");
        assert!(c.contains(&"out/clip.mp4".to_string()), "{c:?}");
    }

    #[test]
    fn extracts_quoted_paths_with_spaces_and_backticks() {
        let text = r#"Wrote "E:\my project\final shot.png" and `renders/take 2.mp4` today."#;
        let c = extract_path_candidates(text);
        assert!(c.contains(&r"E:\my project\final shot.png".to_string()), "{c:?}");
        assert!(c.contains(&"renders/take 2.mp4".to_string()), "{c:?}");
    }

    #[test]
    fn extracts_markdown_link_target_and_strips_trailing_punct() {
        let text = "See ![img](out/render.webp), plus (E:\\x\\y.gif).";
        let c = extract_path_candidates(text);
        assert!(c.contains(&"out/render.webp".to_string()), "{c:?}");
        assert!(c.contains(&"E:\\x\\y.gif".to_string()), "{c:?}");
    }

    #[test]
    fn skips_urls_and_mid_word_extensions_and_dedupes() {
        let text = "https://example.com/a.png data:image/png;base64,x \
                    movie.moviegoer out.png out.png";
        let c = extract_path_candidates(text);
        assert!(!c.iter().any(|x| x.contains("example.com")), "{c:?}");
        assert!(!c.iter().any(|x| x.starts_with("data:")), "{c:?}");
        assert!(!c.iter().any(|x| x.contains("moviegoer")), "{c:?}");
        assert_eq!(c.iter().filter(|x| x.as_str() == "out.png").count(), 1, "{c:?}");
    }

    #[test]
    fn resolve_copies_existing_within_caps_and_skips_missing() {
        let src = temp_dir("src");
        let dest = temp_dir("dest");
        std::fs::write(src.join("real.png"), b"png-bytes").unwrap();
        std::fs::write(src.join("clip.mp4"), b"mp4-bytes").unwrap();

        let cands = vec![
            "real.png".to_string(),                        // relative, exists
            "missing.png".to_string(),                     // doesn't exist
            src.join("clip.mp4").display().to_string(),    // absolute, exists
        ];
        let resolved = resolve_and_copy(&cands, &src, &dest);
        assert_eq!(resolved.len(), 2, "{resolved:?}");
        assert!(resolved.iter().any(|m| m.kind == MediaKind::Image));
        assert!(resolved.iter().any(|m| m.kind == MediaKind::Video));
        for m in &resolved {
            assert!(Path::new(&m.stored_path).is_file(), "copy exists: {}", m.stored_path);
            assert!(m.stored_path.starts_with(&dest.display().to_string()), "{}", m.stored_path);
        }

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn resolve_dedupes_same_source_and_honors_per_message_caps() {
        let src = temp_dir("src2");
        let dest = temp_dir("dest2");
        for i in 0..8 {
            std::fs::write(src.join(format!("i{i}.png")), b"x").unwrap();
        }
        let mut cands: Vec<String> = (0..8).map(|i| format!("i{i}.png")).collect();
        cands.push("i0.png".into()); // duplicate source
        let resolved = resolve_and_copy(&cands, &src, &dest);
        // Capped at MAX_IMAGES, duplicate not double-copied.
        assert_eq!(resolved.len(), 6, "{resolved:?}");

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }
}
