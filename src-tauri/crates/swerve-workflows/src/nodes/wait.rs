//! Wait — cancel-aware pause, passing items through untouched.

use super::{Needs, Node, NodeSpec, MAIN_IN, MAIN_OUT};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{NodeInput, NodeOutput};
use std::time::{Duration, Instant};

pub struct WaitNode;

impl Node for WaitNode {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "util.wait",
            type_version: 1,
            label: "Wait",
            category: "flow",
            description: "Pauses the run for a number of seconds, then passes items through.",
            inputs: MAIN_IN,
            outputs: MAIN_OUT,
            needs: Needs { network: false, code: false, fs_read: false, fs_write: false, agent: false },
            is_trigger: false,
            secrets_ok: false,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError> {
        let params = ctx.params_node()?;
        let seconds = params
            .get("seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0)
            .clamp(0.0, 3600.0);
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            ctx.check_cancel()?;
            std::thread::sleep(Duration::from_millis(200).min(deadline - Instant::now()));
        }
        Ok(NodeOutput::main(input.main().to_vec()))
    }
}
