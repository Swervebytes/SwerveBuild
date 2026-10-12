//! Per-project fact card.
//!
//! Written by the app after each finished turn from data it already has:
//! the last user request in each chat, whether that turn finished or stopped
//! on an error, the git branch and dirty files, and which chats are open.
//! No model call. A new chat (no assistant reply yet) receives the text once,
//! on the wire only.
//!
//! The file lives beside `data.json` under `project-cards/`. It is not part of
//! the frozen store schema.

use crate::paths;
use crate::store::{AppStore, Store};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const REQUEST_CHARS: usize = 160;
const DIRTY_CAP: usize = 8;
const CHAT_CAP: usize = 6;
const GIT_TIMEOUT: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatFact {
    pub id: String,
    pub title: String,
    pub open: bool,
    pub last_request: String,
    /// "finished" | "error" | "none"
    pub outcome: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCard {
    pub project_id: String,
    pub updated_at: String,
    pub branch: Option<String>,
    pub dirty: Vec<String>,
    pub dirty_more: u32,
    pub chats: Vec<ChatFact>,
}

struct GitSnapshot {
    branch: Option<String>,
    dirty: Vec<String>,
    dirty_more: u32,
}

fn card_path(project_id: &str) -> Result<PathBuf, String> {
    if project_id.is_empty()
        || !project_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("invalid project id".into());
    }
    Ok(paths::data_dir()
        .join("project-cards")
        .join(format!("{project_id}.json")))
}

fn load_card_file(project_id: &str) -> Option<ProjectCard> {
    let path = card_path(project_id).ok()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_card(card: &ProjectCard) -> Result<(), String> {
    let path = card_path(&card.project_id)?;
    let raw = serde_json::to_string_pretty(card).map_err(|e| e.to_string())?;
    paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())
}

/// Collapse whitespace, drop secret-shaped tokens, clip to one short line.
pub fn clip_request(raw: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for token in raw.split_whitespace() {
        if token_is_secret(token) {
            parts.push("[redacted]".into());
        } else {
            parts.push(token.to_string());
        }
    }
    let line = parts.join(" ");
    let count = line.chars().count();
    if count <= REQUEST_CHARS {
        line
    } else {
        let clipped: String = line.chars().take(REQUEST_CHARS).collect();
        format!("{clipped}…")
    }
}

fn token_is_secret(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    const PREFIXES: &[&str] = &["sk-", "xai-", "ghp_", "github_pat_", "akia", "ya29."];
    if PREFIXES.iter().any(|prefix| lower.starts_with(prefix)) {
        return true;
    }
    let body = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    body.len() >= 40
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn parse_porcelain(text: &str) -> (Vec<String>, u32) {
    let mut paths = Vec::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        let rest = line[3..].trim();
        let path = rest.rsplit(" -> ").next().unwrap_or(rest).trim();
        let path = path.trim_matches('"');
        if path.is_empty() {
            continue;
        }
        paths.push(path.to_string());
    }
    if paths.len() <= DIRTY_CAP {
        (paths, 0)
    } else {
        let more = (paths.len() - DIRTY_CAP) as u32;
        paths.truncate(DIRTY_CAP);
        (paths, more)
    }
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    if !root.is_dir() {
        return None;
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut out = String::new();
                let stdout = child.stdout.as_mut()?;
                stdout.read_to_string(&mut out).ok()?;
                return Some(out);
            }
            Ok(None) => {
                if start.elapsed() > GIT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                thread::sleep(Duration::from_millis(30));
            }
            Err(_) => return None,
        }
    }
}

fn git_snapshot(root: &Path) -> GitSnapshot {
    let named = git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let branch = match named.as_deref() {
        Some("HEAD") => git_output(root, &["rev-parse", "--short", "HEAD"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        other => other.map(|s| s.to_string()),
    };
    let (dirty, dirty_more) = git_output(root, &["status", "--porcelain", "-uno"])
        .map(|s| parse_porcelain(&s))
        .unwrap_or_default();
    GitSnapshot {
        branch,
        dirty,
        dirty_more,
    }
}

fn chat_facts(
    store: &AppStore,
    project_id: &str,
    open_ids: &[String],
    focus: Option<(&str, &str)>,
) -> Vec<ChatFact> {
    let mut facts: Vec<ChatFact> = store
        .chats
        .iter()
        .filter(|chat| chat.project_id == project_id)
        .map(|chat| {
            let last_user = chat.messages.iter().rev().find(|m| m.role == "user");
            let last = chat.messages.last();
            let mut outcome = if last.is_some_and(|m| m.role == "assistant") {
                "finished"
            } else {
                "none"
            };
            if let Some((id, focus_outcome)) = focus {
                if chat.id == id {
                    outcome = focus_outcome;
                }
            }
            ChatFact {
                id: chat.id.clone(),
                title: chat.title.clone(),
                open: open_ids.iter().any(|id| id == &chat.id),
                last_request: last_user
                    .map(|m| clip_request(&m.content))
                    .unwrap_or_default(),
                outcome: outcome.to_string(),
                updated_at: chat.updated_at.clone(),
            }
        })
        .filter(|fact| !fact.last_request.is_empty() || fact.open)
        .collect();
    facts.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    facts.truncate(CHAT_CAP);
    facts
}

fn assemble(
    store: &AppStore,
    project_id: &str,
    project_path: &str,
    open_ids: &[String],
    focus: Option<(&str, &str)>,
) -> ProjectCard {
    let git = git_snapshot(Path::new(project_path));
    ProjectCard {
        project_id: project_id.to_string(),
        updated_at: Store::now(),
        branch: git.branch,
        dirty: git.dirty,
        dirty_more: git.dirty_more,
        chats: chat_facts(store, project_id, open_ids, focus),
    }
}

/// After a turn: refresh this project's card. Best-effort for the caller.
pub fn record(chat_id: &str, outcome: &str, open_ids: &[String]) -> Result<(), String> {
    let outcome = if outcome == "error" {
        "error"
    } else {
        "finished"
    };
    let (project_id, project_path, store) = {
        let _guard = Store::lock();
        let store = Store::load();
        let chat = store
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .ok_or_else(|| "Chat not found".to_string())?;
        let project = store
            .projects
            .iter()
            .find(|p| p.id == chat.project_id)
            .ok_or_else(|| "Project not found".to_string())?;
        (project.id.clone(), project.path.clone(), store)
    };
    let card = assemble(
        &store,
        &project_id,
        &project_path,
        open_ids,
        Some((chat_id, outcome)),
    );
    save_card(&card)
}

/// Read the saved card, or build one from the store and git when none exists.
pub fn load_or_build(project_id: &str, open_ids: &[String]) -> Result<ProjectCard, String> {
    if let Some(mut card) = load_card_file(project_id) {
        for chat in &mut card.chats {
            chat.open = open_ids.iter().any(|id| id == &chat.id);
        }
        return Ok(card);
    }
    let (path, store) = {
        let _guard = Store::lock();
        let store = Store::load();
        let project = store
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| "Project not found".to_string())?;
        (project.path.clone(), store)
    };
    let card = assemble(&store, project_id, &path, open_ids, None);
    let _ = save_card(&card);
    Ok(card)
}

/// Wire text for a chat that has no assistant reply yet.
/// `None` when there is nothing earlier to report.
pub fn wire_prefix(chat_id: &str, open_ids: &[String]) -> Option<String> {
    let (project_id, project_path, store) = {
        let _guard = Store::lock();
        let store = Store::load();
        let chat = store.chats.iter().find(|c| c.id == chat_id)?;
        if chat.messages.iter().any(|m| m.role == "assistant") {
            return None;
        }
        let project = store.projects.iter().find(|p| p.id == chat.project_id)?;
        (project.id.clone(), project.path.clone(), store)
    };
    let card = assemble(&store, &project_id, &project_path, open_ids, None);
    let text = format_status(&card, Some(chat_id))?;
    let _ = save_card(&card);
    Some(text)
}

/// One short fact sheet. Excludes `exclude_chat_id` so a brand-new chat does
/// not see its own just-sent message as history.
pub fn format_status(card: &ProjectCard, exclude_chat_id: Option<&str>) -> Option<String> {
    let chats: Vec<&ChatFact> = card
        .chats
        .iter()
        .filter(|chat| exclude_chat_id != Some(chat.id.as_str()))
        .filter(|chat| !chat.last_request.is_empty() || chat.open)
        .take(CHAT_CAP)
        .collect();
    let has_git = card.branch.is_some() || !card.dirty.is_empty();
    if chats.is_empty() && !has_git {
        return None;
    }

    let mut lines = Vec::new();
    lines.push("[SwerveBuild project status — facts; not a task list]".to_string());
    lines.push(
        "Say where this project stands in one short note. Do not start leftover work from the other chats. Then do what the user asks below."
            .to_string(),
    );
    if let Some(branch) = card.branch.as_deref() {
        lines.push(format!("Branch: {branch}"));
    }
    if card.dirty.is_empty() {
        if has_git {
            lines.push("Dirty: none".to_string());
        }
    } else {
        let mut dirty = card.dirty.join(", ");
        if card.dirty_more > 0 {
            dirty.push_str(&format!(" (+{} more)", card.dirty_more));
        }
        lines.push(format!("Dirty: {dirty}"));
    }
    if !chats.is_empty() {
        lines.push("Chats:".to_string());
        for chat in chats {
            let open = if chat.open { " (open)" } else { "" };
            if chat.last_request.is_empty() {
                lines.push(format!("- {}{open} — no request yet", chat.title));
                continue;
            }
            let outcome = match chat.outcome.as_str() {
                "finished" => "finished",
                "error" => "stopped on an error",
                _ => "no finished turn",
            };
            lines.push(format!(
                "- {}{open} — {outcome} — \"{}\"",
                chat.title, chat.last_request
            ));
        }
    }
    lines.push("[End project status]".to_string());
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(chats: Vec<ChatFact>) -> ProjectCard {
        ProjectCard {
            project_id: "p1".into(),
            updated_at: "1".into(),
            branch: Some("main".into()),
            dirty: vec!["src/a.ts".into()],
            dirty_more: 2,
            chats,
        }
    }

    fn fact(id: &str, request: &str, outcome: &str) -> ChatFact {
        ChatFact {
            id: id.into(),
            title: format!("Chat {id}"),
            open: id == "open",
            last_request: request.into(),
            outcome: outcome.into(),
            updated_at: "2".into(),
        }
    }

    #[test]
    fn clips_and_redacts_a_request() {
        let raw = format!("please use sk-abc123secret and {}", "A".repeat(50));
        let line = clip_request(&raw);
        assert!(line.contains("[redacted]"));
        assert!(!line.contains("sk-abc"));
        assert!(line.chars().count() <= REQUEST_CHARS + 1);
    }

    #[test]
    fn porcelain_keeps_paths_and_caps() {
        let text = " M src/a.ts\nM  src/b.rs\nR  old.ts -> new.ts\n?? skip-me\n";
        let (paths, more) = parse_porcelain(text);
        assert_eq!(paths, vec!["src/a.ts", "src/b.rs", "new.ts", "skip-me"]);
        assert_eq!(more, 0);

        let many = (0..12)
            .map(|i| format!(" M file{i}.ts"))
            .collect::<Vec<_>>()
            .join("\n");
        let (capped, extra) = parse_porcelain(&many);
        assert_eq!(capped.len(), DIRTY_CAP);
        assert_eq!(extra, 4);
    }

    #[test]
    fn status_excludes_the_new_chat_and_skips_an_empty_project() {
        let text = format_status(
            &card(vec![
                fact("new", "build the card", "none"),
                fact("old", "fix the meter", "finished"),
            ]),
            Some("new"),
        )
        .expect("facts");
        assert!(text.contains("fix the meter"));
        assert!(!text.contains("build the card"));
        assert!(text.contains("do what the user asks below"));
        assert!(text.contains("src/a.ts"));
        assert!(text.contains("(+2 more)"));
        assert!(text.contains("finished"));

        let empty = ProjectCard {
            project_id: "p".into(),
            updated_at: "1".into(),
            branch: None,
            dirty: vec![],
            dirty_more: 0,
            chats: vec![],
        };
        assert!(format_status(&empty, None).is_none());
    }

    #[test]
    fn status_names_an_error_stop() {
        let text =
            format_status(&card(vec![fact("c", "revert the snapshot", "error")]), None).unwrap();
        assert!(text.contains("stopped on an error"));
    }
}
