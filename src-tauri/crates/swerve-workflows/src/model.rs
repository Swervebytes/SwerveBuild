//! The workflow document — the exact shape stored in
//! `~/.swervebuild/workflows/<id>.json` and edited by the canvas.

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_version() -> u32 {
    1
}
fn default_true() -> bool {
    true
}
fn default_one() -> u32 {
    1
}
fn main_port() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub nodes: Vec<NodeDef>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub state: WorkflowState,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// Forward-compat: unknown fields round-trip losslessly.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

impl Workflow {
    pub fn node(&self, id: &str) -> Option<&NodeDef> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlapPolicy {
    Skip,
    Replace,
}

impl Default for OverlapPolicy {
    fn default() -> Self {
        OverlapPolicy::Skip
    }
}

fn default_timeout_secs() -> u64 {
    600
}
fn default_keep_runs() -> usize {
    50
}
fn default_capture() -> String {
    "sample".to_string()
}
fn default_max_items() -> usize {
    10_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub overlap: OverlapPolicy,
    #[serde(default)]
    pub min_interval_secs: u64,
    #[serde(default = "default_keep_runs")]
    pub keep_runs: usize,
    /// "sample" | "full" | "none"
    #[serde(default = "default_capture")]
    pub capture: String,
    #[serde(default = "default_max_items")]
    pub max_items_per_port: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            timeout_secs: default_timeout_secs(),
            overlap: OverlapPolicy::default(),
            min_interval_secs: 0,
            keep_runs: default_keep_runs(),
            capture: default_capture(),
            max_items_per_port: default_max_items(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnError {
    Stop,
    Skip,
    Branch,
}

impl Default for OnError {
    fn default() -> Self {
        OnError::Stop
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrySpec {
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub backoff_secs: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDef {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default = "default_one")]
    pub type_version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub position: (f64, f64),
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub on_error: OnError,
    #[serde(default)]
    pub retry: Option<RetrySpec>,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub from: String,
    #[serde(rename = "out", default = "main_port")]
    pub from_port: String,
    pub to: String,
    #[serde(rename = "in", default = "main_port")]
    pub to_port: String,
}

// --------------------------------------------------------------- permissions

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub network: NetworkPermission,
    #[serde(default)]
    pub code: bool,
    #[serde(default)]
    pub fs: FsPermission,
    #[serde(default)]
    pub agent: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkPermission {
    #[serde(default)]
    pub enabled: bool,
    /// Exact host names or `*.suffix` wildcards. Empty + enabled = any public host.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// Allow loopback/RFC1918/link-local/ULA targets (homelab opt-in).
    #[serde(default)]
    pub private_ips: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FsPermission {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

// --------------------------------------------------------------- runtime state

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowState {
    #[serde(default)]
    pub last_fired_at: Option<u64>,
    #[serde(default)]
    pub last_run_id: Option<String>,
    #[serde(default)]
    pub last_status: Option<String>,
    /// Per-trigger-node bookkeeping (git last-seen commit, file snapshot),
    /// keyed by trigger node id. Owned by the scheduler.
    #[serde(default)]
    pub trigger: serde_json::Map<String, Value>,
}

/// Reject ids that could escape a directory when used in a path join.
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
