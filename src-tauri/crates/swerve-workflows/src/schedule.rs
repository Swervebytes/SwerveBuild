//! Pure trigger-timing helpers, ported from the proven Automations scheduler
//! (`jobs.rs`) so the app-side workflow scheduler stays thin. All functions are
//! side-effect-free except `git_head`, which shells out to `git`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleSpec {
    /// "interval" | "daily" | "weekly"
    #[serde(default)]
    pub every: String,
    #[serde(default)]
    pub interval_minutes: u64,
    #[serde(default)]
    pub hour: u32,
    #[serde(default)]
    pub minute: u32,
    /// 0 = Sunday .. 6 = Saturday (weekly only)
    #[serde(default)]
    pub weekday: u32,
    /// Webview `Date.getTimezoneOffset()` (minutes; UTC = local + offset).
    #[serde(default)]
    pub tz_offset_minutes: i32,
}

/// Most recent scheduled occurrence (UTC secs) for a daily/weekly schedule, or
/// None. Walks back up to 8 days to find the matching weekday for weekly.
pub fn most_recent_occurrence(s: &ScheduleSpec, now: u64) -> Option<u64> {
    if s.every == "interval" {
        return None;
    }
    let offset = s.tz_offset_minutes as i64 * 60; // UTC = local + offset
    let now_i = now as i64;
    let local_now = now_i - offset;
    for back in 0..8i64 {
        let day_num = local_now.div_euclid(86400) - back;
        let day_start_local = day_num * 86400;
        let target_local = day_start_local + s.hour as i64 * 3600 + s.minute as i64 * 60;
        let target_utc = target_local + offset;
        if target_utc > now_i {
            continue;
        }
        if s.every == "weekly" {
            let dow = ((day_num + 4).rem_euclid(7)) as u32; // 0 = Sunday
            if dow != s.weekday {
                continue;
            }
        }
        return Some(target_utc as u64);
    }
    None
}

/// Current HEAD (or a branch tip) of a git repo, or None.
pub fn git_head(cwd: &str, branch: Option<&str>) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let reference = branch.filter(|b| !b.is_empty()).unwrap_or("HEAD");
    let mut cmd = std::process::Command::new("git");
    cmd.args(["rev-parse", "--verify", reference]).current_dir(cwd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if head.is_empty() {
        None
    } else {
        Some(head)
    }
}

/// Depth-1 snapshot ("name:mtime:size|…") of a single file or one non-recursive
/// directory, filtered by an optional `*`/`?` glob. Errors → empty snapshot.
pub fn file_snapshot(path: &str, glob: Option<&str>) -> String {
    let p = Path::new(path);
    let mut entries: Vec<String> = Vec::new();
    let mut push = |name: &str, meta: &fs::Metadata| {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        entries.push(format!("{name}:{mtime}:{}", meta.len()));
    };
    if p.is_file() {
        if let Ok(meta) = p.metadata() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            push(name, &meta);
        }
    } else if let Ok(rd) = fs::read_dir(p) {
        for entry in rd.flatten() {
            let ep = entry.path();
            if !ep.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(g) = glob {
                if !glob_match(g, &name) {
                    continue;
                }
            }
            if let Ok(meta) = entry.metadata() {
                push(&name, &meta);
            }
        }
    }
    entries.sort();
    entries.join("|")
}

/// Minimal `*`/`?` glob over a single filename component (case-insensitive).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let s: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-01-01 00:00:00 UTC (a Thursday).
    const JAN_1_2026: u64 = 1_767_225_600;

    fn daily_at(hour: u32) -> ScheduleSpec {
        ScheduleSpec { every: "daily".into(), hour, ..Default::default() }
    }

    #[test]
    fn daily_after_the_time_is_today() {
        let occ = most_recent_occurrence(&daily_at(9), JAN_1_2026 + 10 * 3600).unwrap();
        assert_eq!(occ, JAN_1_2026 + 9 * 3600);
    }

    #[test]
    fn daily_before_the_time_is_yesterday() {
        let occ = most_recent_occurrence(&daily_at(9), JAN_1_2026 + 8 * 3600).unwrap();
        assert_eq!(occ, JAN_1_2026 - 86400 + 9 * 3600);
    }

    #[test]
    fn interval_schedules_have_no_occurrence() {
        let s = ScheduleSpec { every: "interval".into(), ..Default::default() };
        assert!(most_recent_occurrence(&s, JAN_1_2026).is_none());
    }

    #[test]
    fn glob_basics() {
        assert!(glob_match("*.md", "README.md"));
        assert!(glob_match("*.MD", "readme.md"));
        assert!(!glob_match("*.md", "readme.rs"));
        assert!(glob_match("a?c", "abc"));
    }
}
