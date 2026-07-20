//! Read File / Write File — permission-gated, per item, UTF-8 text in v1.

use super::{Needs, Node, NodeSpec, MAIN_IN, MAIN_OUT};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{Item, NodeInput, NodeOutput};
use serde_json::{json, Value};

pub struct ReadFile;

impl Node for ReadFile {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "file.read",
            type_version: 1,
            label: "Read File",
            category: "action",
            description: "Reads a text file per item from a granted folder.",
            inputs: MAIN_IN,
            outputs: MAIN_OUT,
            needs: Needs { network: false, code: false, fs_read: true, fs_write: false, agent: false },
            is_trigger: false,
            secrets_ok: false,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError> {
        let mut out = NodeOutput::default();
        out.set_port("main", Vec::new());
        for (i, item) in input.main().iter().enumerate() {
            ctx.check_cancel()?;
            let params = ctx.params(i)?;
            let path = required_path(&params, i)?;
            match ctx.fs_read(&path) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    out.push("main", Item::new(json!({ "path": path, "text": text })));
                }
                Err(err) if ctx.branch_errors() => out.push(
                    "error",
                    Item::new(json!({
                        "error": { "kind": err.kind, "message": err.message },
                        "item": item.json.clone(),
                    })),
                ),
                Err(err) => return Err(err.at_item(i)),
            }
        }
        Ok(out)
    }
}

pub struct WriteFile;

impl Node for WriteFile {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "file.write",
            type_version: 1,
            label: "Write File",
            category: "action",
            description: "Writes text per item into a granted folder, then passes items through.",
            inputs: MAIN_IN,
            outputs: MAIN_OUT,
            needs: Needs { network: false, code: false, fs_read: false, fs_write: true, agent: false },
            is_trigger: false,
            secrets_ok: false,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError> {
        let mut out = NodeOutput::default();
        out.set_port("main", Vec::new());
        for (i, item) in input.main().iter().enumerate() {
            ctx.check_cancel()?;
            let params = ctx.params(i)?;
            let path = required_path(&params, i)?;
            let append = params.get("mode").and_then(|v| v.as_str()) == Some("append");
            let content = match params.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => serde_json::to_string_pretty(other).unwrap_or_default(),
            };
            match ctx.fs_write(&path, content.as_bytes(), append) {
                Ok(()) => out.push("main", item.clone()),
                Err(err) if ctx.branch_errors() => out.push(
                    "error",
                    Item::new(json!({
                        "error": { "kind": err.kind, "message": err.message },
                        "item": item.json.clone(),
                    })),
                ),
                Err(err) => return Err(err.at_item(i)),
            }
        }
        Ok(out)
    }
}

fn required_path(params: &Value, item_index: usize) -> Result<String, NodeError> {
    params
        .get("path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| NodeError::params("a file path is required").at_item(item_index))
}
