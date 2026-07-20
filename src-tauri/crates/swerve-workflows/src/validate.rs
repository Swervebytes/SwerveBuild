//! Load-time + pre-run validation. The editor calls this live; the engine
//! calls it again before every run.

use crate::model::{is_safe_id, OnError, Workflow};
use crate::nodes;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Serialize)]
pub struct ValidationItem {
    pub node_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Validation {
    pub errors: Vec<ValidationItem>,
    pub warnings: Vec<ValidationItem>,
}

impl Validation {
    fn error(&mut self, node_id: Option<&str>, message: impl Into<String>) {
        self.errors.push(ValidationItem { node_id: node_id.map(String::from), message: message.into() });
    }
    fn warn(&mut self, node_id: Option<&str>, message: impl Into<String>) {
        self.warnings.push(ValidationItem { node_id: node_id.map(String::from), message: message.into() });
    }
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn error_summary(&self) -> String {
        self.errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

pub fn validate(w: &Workflow) -> Validation {
    let mut v = Validation::default();

    // --- nodes: ids, names, types, params shape --------------------------------
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut trigger_count = 0usize;
    for node in &w.nodes {
        if !is_safe_id(&node.id) {
            v.error(Some(&node.id), format!("node id {:?} is not a valid id", node.id));
        }
        if !ids.insert(node.id.clone()) {
            v.error(Some(&node.id), format!("duplicate node id {}", node.id));
        }
        let name = node.name.trim();
        if name.is_empty() {
            v.error(Some(&node.id), "every node needs a name");
        } else if !names.insert(name.to_string()) {
            v.error(Some(&node.id), format!("duplicate node name {name:?} — names must be unique so expressions can reference them"));
        }
        match nodes::get(&node.node_type) {
            None => v.error(Some(&node.id), format!("unknown node type {}", node.node_type)),
            Some(n) => {
                let spec = n.spec();
                if node.type_version != spec.type_version {
                    v.warn(
                        Some(&node.id),
                        format!("{} is version {} here but this app speaks version {}", node.node_type, node.type_version, spec.type_version),
                    );
                }
                if spec.is_trigger {
                    trigger_count += 1;
                }
            }
        }
        if !node.params.is_object() && !node.params.is_null() {
            v.error(Some(&node.id), "params must be an object");
        }
    }
    if trigger_count == 0 {
        v.error(None, "the workflow needs at least one trigger node");
    }

    // --- connections: endpoints + ports ----------------------------------------
    for c in &w.connections {
        let from = w.node(&c.from);
        let to = w.node(&c.to);
        if from.is_none() {
            v.error(None, format!("connection references missing node {}", c.from));
        }
        if to.is_none() {
            v.error(None, format!("connection references missing node {}", c.to));
        }
        if let Some(from) = from {
            if let Some(n) = nodes::get(&from.node_type) {
                let spec = n.spec();
                let virtual_error = from.on_error == OnError::Branch && c.from_port == "error";
                if !virtual_error && !spec.outputs.iter().any(|p| p.name == c.from_port) {
                    v.error(Some(&from.id), format!("{} has no output named {:?}", from.name, c.from_port));
                }
            }
        }
        if let Some(to) = to {
            if let Some(n) = nodes::get(&to.node_type) {
                let spec = n.spec();
                if spec.is_trigger {
                    v.error(Some(&to.id), format!("{} is a trigger and cannot receive connections", to.name));
                } else if !spec.inputs.iter().any(|p| p.name == c.to_port) {
                    v.error(Some(&to.id), format!("{} has no input named {:?}", to.name, c.to_port));
                }
            }
        }
    }

    // --- DAG check (Kahn) -------------------------------------------------------
    let mut indegree: BTreeMap<&str, usize> = w.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
    for c in &w.connections {
        if let Some(d) = indegree.get_mut(c.to.as_str()) {
            *d += 1;
        }
    }
    let mut queue: VecDeque<&str> = indegree.iter().filter(|(_, d)| **d == 0).map(|(id, _)| *id).collect();
    let mut visited = 0usize;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        for c in w.connections.iter().filter(|c| c.from == id) {
            if let Some(d) = indegree.get_mut(c.to.as_str()) {
                *d -= 1;
                if *d == 0 {
                    queue.push_back(c.to.as_str());
                }
            }
        }
    }
    if visited < w.nodes.len() {
        let stuck: Vec<&str> = indegree.iter().filter(|(_, d)| **d > 0).map(|(id, _)| *id).collect();
        v.error(None, format!("the graph has a cycle involving: {}", stuck.join(", ")));
    }

    // --- reachability from any trigger ------------------------------------------
    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    let mut frontier: VecDeque<&str> = w
        .nodes
        .iter()
        .filter(|n| nodes::get(&n.node_type).map(|x| x.spec().is_trigger).unwrap_or(false))
        .map(|n| n.id.as_str())
        .collect();
    reachable.extend(frontier.iter().copied());
    while let Some(id) = frontier.pop_front() {
        for c in w.connections.iter().filter(|c| c.from == id) {
            if reachable.insert(c.to.as_str()) {
                frontier.push_back(c.to.as_str());
            }
        }
    }
    for node in &w.nodes {
        let is_trigger = nodes::get(&node.node_type).map(|x| x.spec().is_trigger).unwrap_or(false);
        if !is_trigger && !reachable.contains(node.id.as_str()) {
            v.warn(Some(&node.id), format!("{} is not connected to any trigger and will never run", node.name));
        }
    }

    // --- needs vs permissions ----------------------------------------------------
    for node in &w.nodes {
        if node.disabled || !reachable.contains(node.id.as_str()) {
            continue;
        }
        let Some(n) = nodes::get(&node.node_type) else { continue };
        let needs = n.spec().needs;
        let p = &w.permissions;
        if needs.network && !p.network.enabled {
            v.error(Some(&node.id), format!("{} needs network access — enable it in Permissions", node.name));
        }
        if needs.code && !p.code {
            v.error(Some(&node.id), format!("{} needs the code permission — enable it in Permissions", node.name));
        }
        if needs.fs_read && p.fs.read.is_empty() {
            v.error(Some(&node.id), format!("{} needs a readable folder — grant one in Permissions", node.name));
        }
        if needs.fs_write && p.fs.write.is_empty() {
            v.error(Some(&node.id), format!("{} needs a writable folder — grant one in Permissions", node.name));
        }
        if needs.agent && !p.agent {
            v.error(Some(&node.id), format!("{} needs the agent permission — enable it in Permissions", node.name));
        }
    }

    // --- settings ---------------------------------------------------------------
    if !matches!(w.settings.capture.as_str(), "none" | "sample" | "full") {
        v.error(None, format!("settings.capture must be none, sample, or full (got {:?})", w.settings.capture));
    }

    v
}
