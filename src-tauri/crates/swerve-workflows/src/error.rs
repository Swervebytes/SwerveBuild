use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    Params,
    Expression,
    Permission,
    Http,
    Fs,
    Agent,
    Code,
    Timeout,
    Cancelled,
    Data,
    Internal,
}

/// A node-level failure. `item_index` is set when the failure is tied to one
/// input item (so the UI can point at it and `on_error: branch` can route it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_index: Option<usize>,
}

impl NodeError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        NodeError { kind, message: message.into(), item_index: None }
    }

    pub fn params(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Params, message)
    }

    pub fn expression(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Expression, message)
    }

    pub fn permission(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Permission, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn at_item(mut self, index: usize) -> Self {
        self.item_index = Some(index);
        self
    }
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.item_index {
            Some(i) => write!(f, "{} (item {i})", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}
