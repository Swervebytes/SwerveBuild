//! The node registry. Every node is a stateless unit struct; all per-run state
//! and every capability flows through `NodeCtx` — adding a node type never
//! widens the security review surface.

use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{NodeInput, NodeOutput};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub mod agent;
pub mod code;
pub mod file;
pub mod http;
pub mod ifnode;
pub mod merge;
pub mod set;
pub mod trigger;
pub mod wait;

pub struct PortSpec {
    pub name: &'static str,
    pub label: &'static str,
}

#[derive(Default, Clone, Copy)]
pub struct Needs {
    pub network: bool,
    pub code: bool,
    pub fs_read: bool,
    pub fs_write: bool,
    pub agent: bool,
}

pub struct NodeSpec {
    pub type_name: &'static str,
    pub type_version: u32,
    pub label: &'static str,
    /// "trigger" | "flow" | "transform" | "action" | "code" | "agent"
    pub category: &'static str,
    pub description: &'static str,
    pub inputs: &'static [PortSpec],
    pub outputs: &'static [PortSpec],
    pub needs: Needs,
    pub is_trigger: bool,
    /// May `{{ $secret("…") }}` resolve inside this node's params?
    pub secrets_ok: bool,
}

pub const MAIN_IN: &[PortSpec] = &[PortSpec { name: "main", label: "In" }];
pub const MAIN_OUT: &[PortSpec] = &[PortSpec { name: "main", label: "Out" }];
pub const NO_PORTS: &[PortSpec] = &[];

pub trait Node: Send + Sync {
    fn spec(&self) -> &'static NodeSpec;
    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError>;
}

static MANUAL: trigger::ManualTrigger = trigger::ManualTrigger;
static SCHEDULE: trigger::ScheduleTrigger = trigger::ScheduleTrigger;
static GIT: trigger::GitTrigger = trigger::GitTrigger;
static FILE_TRIGGER: trigger::FileTrigger = trigger::FileTrigger;
static HTTP: http::HttpRequest = http::HttpRequest;
static SET: set::SetFields = set::SetFields;
static IF: ifnode::IfNode = ifnode::IfNode;
static MERGE: merge::MergeNode = merge::MergeNode;
static CODE: code::CodeNode = code::CodeNode;
static AGENT: agent::AgentNode = agent::AgentNode;
static FILE_READ: file::ReadFile = file::ReadFile;
static FILE_WRITE: file::WriteFile = file::WriteFile;
static WAIT: wait::WaitNode = wait::WaitNode;

pub fn registry() -> &'static BTreeMap<&'static str, &'static dyn Node> {
    static REG: OnceLock<BTreeMap<&'static str, &'static dyn Node>> = OnceLock::new();
    REG.get_or_init(|| {
        let all: Vec<&'static dyn Node> = vec![
            &MANUAL,
            &SCHEDULE,
            &GIT,
            &FILE_TRIGGER,
            &HTTP,
            &SET,
            &IF,
            &MERGE,
            &CODE,
            &AGENT,
            &FILE_READ,
            &FILE_WRITE,
            &WAIT,
        ];
        let mut map: BTreeMap<&'static str, &'static dyn Node> = BTreeMap::new();
        for node in all {
            map.insert(node.spec().type_name, node);
        }
        map
    })
}

pub fn get(type_name: &str) -> Option<&'static dyn Node> {
    registry().get(type_name).copied()
}

/// Serialized specs for the UI (palette, port rendering, needs badges).
pub fn catalog_json() -> Value {
    let specs: Vec<Value> = registry()
        .values()
        .map(|n| {
            let s = n.spec();
            json!({
                "type": s.type_name,
                "type_version": s.type_version,
                "label": s.label,
                "category": s.category,
                "description": s.description,
                "inputs": s.inputs.iter().map(|p| json!({"name": p.name, "label": p.label})).collect::<Vec<_>>(),
                "outputs": s.outputs.iter().map(|p| json!({"name": p.name, "label": p.label})).collect::<Vec<_>>(),
                "needs": {
                    "network": s.needs.network,
                    "code": s.needs.code,
                    "fs_read": s.needs.fs_read,
                    "fs_write": s.needs.fs_write,
                    "agent": s.needs.agent,
                },
                "is_trigger": s.is_trigger,
                "secrets_ok": s.secrets_ok,
            })
        })
        .collect();
    Value::Array(specs)
}
