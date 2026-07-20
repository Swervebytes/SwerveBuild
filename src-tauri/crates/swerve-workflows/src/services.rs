//! The engine's only doors to the outside world. The app injects real
//! implementations; tests and the CLI inject fakes. HTTP and filesystem are
//! deliberately NOT injectable — their safety checks are engine-owned
//! (`permissions.rs`) and must not vary by host.

use crate::engine::CancelFlag;
use crate::runs::RunEvent;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub prompt: String,
    pub cwd: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_turns: u32,
    pub timeout_secs: u64,
    pub web_search: bool,
    pub json_schema: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentResult {
    pub text: String,
    pub structured: Option<Value>,
    pub stop_reason: Option<String>,
    pub session_id: Option<String>,
}

/// Runs one headless agent turn. The app's implementation wraps the same
/// grok invocation discipline as Automations (shadow-enforced, tree-killed).
pub trait AgentRunner: Send + Sync {
    fn run(&self, req: AgentRequest, cancel: &CancelFlag) -> Result<AgentResult, String>;
}

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
    /// All secrets, for injecting the `$secret()` helper into a JS scope.
    fn all(&self) -> HashMap<String, String>;
}

pub trait RunEvents: Send + Sync {
    fn emit(&self, event: &RunEvent);
}

#[derive(Clone)]
pub struct EngineServices {
    pub agent: Option<Arc<dyn AgentRunner>>,
    pub secrets: Arc<dyn SecretStore>,
    pub events: Arc<dyn RunEvents>,
}

impl Default for EngineServices {
    fn default() -> Self {
        EngineServices {
            agent: None,
            secrets: Arc::new(NoSecrets),
            events: Arc::new(NoEvents),
        }
    }
}

// --------------------------------------------------------------- stock impls

pub struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _name: &str) -> Option<String> {
        None
    }
    fn all(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

pub struct MapSecrets(pub HashMap<String, String>);

impl SecretStore for MapSecrets {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
    fn all(&self) -> HashMap<String, String> {
        self.0.clone()
    }
}

pub struct NoEvents;

impl RunEvents for NoEvents {
    fn emit(&self, _event: &RunEvent) {}
}

/// Collects events in memory — used by tests and available to any embedder.
#[derive(Default)]
pub struct CollectEvents(pub std::sync::Mutex<Vec<RunEvent>>);

impl RunEvents for CollectEvents {
    fn emit(&self, event: &RunEvent) {
        if let Ok(mut g) = self.0.lock() {
            g.push(event.clone());
        }
    }
}
