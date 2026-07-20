//! Merge — combines two input streams.

use super::{Needs, Node, NodeSpec, PortSpec, MAIN_OUT};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{Item, NodeInput, NodeOutput};
use serde_json::json;

pub struct MergeNode;

static INPUTS: &[PortSpec] = &[
    PortSpec { name: "a", label: "A" },
    PortSpec { name: "b", label: "B" },
];

impl Node for MergeNode {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "flow.merge",
            type_version: 1,
            label: "Merge",
            category: "flow",
            description: "Combines two branches: append A then B, or zip pairs by position.",
            inputs: INPUTS,
            outputs: MAIN_OUT,
            needs: Needs { network: false, code: false, fs_read: false, fs_write: false, agent: false },
            is_trigger: false,
            secrets_ok: false,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError> {
        let params = ctx.params_node()?;
        let mode = params.get("mode").and_then(|v| v.as_str()).unwrap_or("append");
        let a = input.port("a");
        let b = input.port("b");
        let items = match mode {
            "zip" => {
                let len = a.len().max(b.len());
                (0..len)
                    .map(|i| {
                        Item::new(json!({
                            "a": a.get(i).map(|it| it.json.clone()),
                            "b": b.get(i).map(|it| it.json.clone()),
                        }))
                    })
                    .collect()
            }
            _ => a.iter().chain(b.iter()).cloned().collect(),
        };
        Ok(NodeOutput::main(items))
    }
}
