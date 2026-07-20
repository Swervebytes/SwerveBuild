//! Trigger nodes. The *scheduler* (app side) decides when one fires; inside a
//! run the fired trigger executes first and emits the seed item carrying the
//! fire payload (`{"trigger": {kind, reason, …}}`).

use super::{Needs, Node, NodeSpec, MAIN_OUT, NO_PORTS};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{Item, NodeInput, NodeOutput};

fn emit_seed(ctx: &mut NodeCtx) -> Result<NodeOutput, NodeError> {
    Ok(NodeOutput::main(vec![Item::new(ctx.fire_payload())]))
}

macro_rules! trigger_node {
    ($struct_name:ident, $type_name:literal, $label:literal, $desc:literal) => {
        pub struct $struct_name;

        impl Node for $struct_name {
            fn spec(&self) -> &'static NodeSpec {
                static SPEC: NodeSpec = NodeSpec {
                    type_name: $type_name,
                    type_version: 1,
                    label: $label,
                    category: "trigger",
                    description: $desc,
                    inputs: NO_PORTS,
                    outputs: MAIN_OUT,
                    needs: Needs {
                        network: false,
                        code: false,
                        fs_read: false,
                        fs_write: false,
                        agent: false,
                    },
                    is_trigger: true,
                    secrets_ok: false,
                };
                &SPEC
            }

            fn run(&self, ctx: &mut NodeCtx, _input: NodeInput) -> Result<NodeOutput, NodeError> {
                emit_seed(ctx)
            }
        }
    };
}

trigger_node!(
    ManualTrigger,
    "trigger.manual",
    "Manual",
    "Starts the workflow when you press Run."
);
trigger_node!(
    ScheduleTrigger,
    "trigger.schedule",
    "Schedule",
    "Starts the workflow on an interval or at a daily or weekly time."
);
trigger_node!(
    GitTrigger,
    "trigger.git",
    "Git commit",
    "Starts the workflow when a repository's head commit changes."
);
trigger_node!(
    FileTrigger,
    "trigger.file",
    "File change",
    "Starts the workflow when files in a folder change."
);
