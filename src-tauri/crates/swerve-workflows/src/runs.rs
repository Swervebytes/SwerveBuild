//! Run records + the append-only event log, mirroring the Automations run
//! store (`<run>.json` meta + `<run>.jsonl` log, atomic writes, prune keep-N).

use crate::error::NodeError;
use crate::model::is_safe_id;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Success,
    Error,
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeRunStatus {
    Success,
    Error,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TriggerInfo {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRunSummary {
    pub node_id: String,
    pub name: String,
    pub status: NodeRunStatus,
    pub items_in: usize,
    pub items_out: usize,
    pub duration_ms: u64,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<NodeError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunErrorInfo {
    pub node_id: String,
    pub node_name: String,
    #[serde(flatten)]
    pub error: NodeError,
}

/// Captured output items for one port of one node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedPort {
    pub items: Vec<Value>,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub workflow_id: String,
    #[serde(default)]
    pub workflow_name: String,
    #[serde(default)]
    pub trigger: TriggerInfo,
    pub status: RunStatus,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunErrorInfo>,
    #[serde(default)]
    pub nodes: Vec<NodeRunSummary>,
    /// node_id -> port -> captured items (per settings.capture).
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub data: serde_json::Map<String, Value>,
    #[serde(default)]
    pub seen: bool,
}

/// Live progress events. The Tauri layer forwards these to the webview; the
/// CLI prints them; every one is also appended to the run's `.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    RunStarted {
        workflow_id: String,
        run_id: String,
    },
    NodeStarted {
        workflow_id: String,
        run_id: String,
        node_id: String,
        name: String,
    },
    NodeFinished {
        workflow_id: String,
        run_id: String,
        node_id: String,
        name: String,
        status: NodeRunStatus,
        items_in: usize,
        items_out: usize,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Log {
        workflow_id: String,
        run_id: String,
        node_id: String,
        level: LogLevel,
        message: String,
    },
    RunFinished {
        workflow_id: String,
        run_id: String,
        status: RunStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

// ------------------------------------------------------------------ store

/// Write bytes atomically: sibling `.tmp` then rename (REPLACE_EXISTING on
/// Windows) — a crash mid-write can never leave a torn primary file.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}

pub struct RunStore {
    runs_dir: PathBuf,
}

impl RunStore {
    pub fn new(runs_dir: PathBuf) -> Self {
        RunStore { runs_dir }
    }

    fn dir(&self, workflow_id: &str) -> PathBuf {
        self.runs_dir.join(workflow_id)
    }

    fn meta_path(&self, workflow_id: &str, run_id: &str) -> PathBuf {
        self.dir(workflow_id).join(format!("{run_id}.json"))
    }

    fn log_path(&self, workflow_id: &str, run_id: &str) -> PathBuf {
        self.dir(workflow_id).join(format!("{run_id}.jsonl"))
    }

    pub fn write_record(&self, rec: &RunRecord) -> Result<(), String> {
        if !is_safe_id(&rec.workflow_id) || !is_safe_id(&rec.id) {
            return Err("invalid id".into());
        }
        let raw = serde_json::to_string_pretty(rec).map_err(|e| e.to_string())?;
        write_atomic(&self.meta_path(&rec.workflow_id, &rec.id), raw.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn append_event(&self, workflow_id: &str, run_id: &str, event: &RunEvent) {
        if !is_safe_id(workflow_id) || !is_safe_id(run_id) {
            return;
        }
        let path = self.log_path(workflow_id, run_id);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(line) = serde_json::to_string(event) {
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    /// All run records for a workflow, newest first. Skips unparseable files.
    pub fn list_records(&self, workflow_id: &str) -> Vec<RunRecord> {
        if !is_safe_id(workflow_id) {
            return Vec::new();
        }
        let mut out: Vec<RunRecord> = Vec::new();
        if let Ok(entries) = fs::read_dir(self.dir(workflow_id)) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(&path) {
                    if let Ok(rec) = serde_json::from_str::<RunRecord>(&raw) {
                        out.push(rec);
                    }
                }
            }
        }
        out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
        out
    }

    pub fn read_record(&self, workflow_id: &str, run_id: &str) -> Option<RunRecord> {
        if !is_safe_id(workflow_id) || !is_safe_id(run_id) {
            return None;
        }
        let raw = fs::read_to_string(self.meta_path(workflow_id, run_id)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn read_log(&self, workflow_id: &str, run_id: &str) -> String {
        if !is_safe_id(workflow_id) || !is_safe_id(run_id) {
            return String::new();
        }
        fs::read_to_string(self.log_path(workflow_id, run_id)).unwrap_or_default()
    }

    /// Prune run records + logs beyond `keep` (newest kept).
    pub fn prune(&self, workflow_id: &str, keep: usize) {
        for rec in self.list_records(workflow_id).into_iter().skip(keep.max(1)) {
            let _ = fs::remove_file(self.meta_path(workflow_id, &rec.id));
            let _ = fs::remove_file(self.log_path(workflow_id, &rec.id));
        }
    }

    /// Remove every run for a workflow (called when the workflow is deleted).
    pub fn remove_all(&self, workflow_id: &str) {
        if !is_safe_id(workflow_id) {
            return;
        }
        let _ = fs::remove_dir_all(self.dir(workflow_id));
    }
}

pub fn now_epoch() -> String {
    crate::schedule::now_secs().to_string()
}
