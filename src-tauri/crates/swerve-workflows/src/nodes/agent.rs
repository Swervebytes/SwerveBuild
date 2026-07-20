//! Agent — one headless agent turn per execution (a batch node, not per-item).
//! Always effective-shadow: the injected runner enforces the same read-safe
//! tool discipline as Automations; this node cannot widen it.

use super::{Needs, Node, NodeSpec, MAIN_IN, MAIN_OUT};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{Item, NodeInput, NodeOutput};
use crate::services::AgentRequest;
use serde_json::json;

pub struct AgentNode;

impl Node for AgentNode {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "agent.run",
            type_version: 1,
            label: "Agent",
            category: "agent",
            description: "Runs one read-only agent turn and outputs its reply.",
            inputs: MAIN_IN,
            outputs: MAIN_OUT,
            needs: Needs { network: false, code: false, fs_read: false, fs_write: false, agent: true },
            is_trigger: false,
            secrets_ok: false,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, _input: NodeInput) -> Result<NodeOutput, NodeError> {
        let params = ctx.params_node()?;
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| NodeError::params("a prompt is required"))?
            .to_string();
        let cwd = params
            .get("cwd")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| NodeError::params("a project folder is required"))?
            .to_string();

        let request = AgentRequest {
            prompt,
            cwd,
            model: params.get("model").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(String::from),
            effort: params.get("effort").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from),
            max_turns: params.get("max_turns").and_then(|v| v.as_u64()).unwrap_or(15).clamp(1, 50) as u32,
            timeout_secs: params.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(600).clamp(30, 3600),
            web_search: params.get("web_search").and_then(|v| v.as_bool()).unwrap_or(false),
            json_schema: params.get("json_schema").cloned().filter(|v| !v.is_null()),
        };

        let runner = ctx.agent()?;
        let result = runner
            .run(request, ctx.cancel_flag())
            .map_err(|e| NodeError::new(crate::error::ErrorKind::Agent, e))?;
        ctx.check_cancel()?;

        Ok(NodeOutput::main(vec![Item::new(json!({
            "text": result.text,
            "structured": result.structured,
            "stop_reason": result.stop_reason,
            "session_id": result.session_id,
        }))]))
    }
}
