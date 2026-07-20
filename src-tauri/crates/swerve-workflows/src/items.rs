//! The n8n-style items data model: every edge carries `Vec<Item>`, every node
//! consumes/produces items grouped by named port. v1 items are JSON-only; the
//! struct exists (instead of a bare `Value`) so a `binary` field can be added
//! without reshaping every node.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub json: Value,
}

impl Item {
    pub fn new(json: Value) -> Self {
        Item { json }
    }
}

pub type Items = Vec<Item>;

/// Items delivered to a node, grouped by input port.
#[derive(Debug, Default, Clone)]
pub struct NodeInput {
    pub ports: Vec<(String, Items)>,
}

impl NodeInput {
    pub fn port(&self, name: &str) -> &[Item] {
        self.ports
            .iter()
            .find(|(p, _)| p == name)
            .map(|(_, items)| items.as_slice())
            .unwrap_or(&[])
    }

    pub fn main(&self) -> &[Item] {
        self.port("main")
    }

    pub fn total(&self) -> usize {
        self.ports.iter().map(|(_, items)| items.len()).sum()
    }
}

/// Items a node produced, grouped by output port.
#[derive(Debug, Default, Clone)]
pub struct NodeOutput {
    pub ports: Vec<(String, Items)>,
}

impl NodeOutput {
    pub fn empty() -> Self {
        NodeOutput::default()
    }

    pub fn main(items: Items) -> Self {
        let mut out = NodeOutput::default();
        out.set_port("main", items);
        out
    }

    pub fn set_port(&mut self, name: &str, items: Items) {
        if let Some(slot) = self.ports.iter_mut().find(|(p, _)| p == name) {
            slot.1 = items;
        } else {
            self.ports.push((name.to_string(), items));
        }
    }

    pub fn push(&mut self, name: &str, item: Item) {
        if let Some(slot) = self.ports.iter_mut().find(|(p, _)| p == name) {
            slot.1.push(item);
        } else {
            self.ports.push((name.to_string(), vec![item]));
        }
    }

    pub fn port(&self, name: &str) -> &[Item] {
        self.ports
            .iter()
            .find(|(p, _)| p == name)
            .map(|(_, items)| items.as_slice())
            .unwrap_or(&[])
    }

    pub fn total(&self) -> usize {
        self.ports.iter().map(|(_, items)| items.len()).sum()
    }
}
