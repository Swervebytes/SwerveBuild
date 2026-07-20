//! Swerve Workflows — headless node-graph workflow engine.
//!
//! Design doc: `docs/workflow-engine-design.md`. This crate has ZERO Tauri
//! dependencies: the app (and the headless CLI, and tests) drive it through
//! [`run_workflow`] plus the injected service traits in [`services`].

pub mod engine;
pub mod error;
pub mod expr;
pub mod items;
pub mod model;
pub mod nodes;
pub mod permissions;
pub mod runs;
pub mod schedule;
pub mod services;
pub mod validate;

pub use engine::{run_workflow, CancelFlag, CancelReason, EngineConfig, NodeCtx, TriggerFire};
pub use error::{ErrorKind, NodeError};
pub use items::{Item, Items, NodeInput, NodeOutput};
pub use model::{Permissions, Workflow};
pub use runs::{RunEvent, RunRecord, RunStatus, RunStore};
pub use services::{AgentRequest, AgentResult, AgentRunner, EngineServices, RunEvents, SecretStore};
