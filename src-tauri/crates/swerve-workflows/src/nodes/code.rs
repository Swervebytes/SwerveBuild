//! Code — user JavaScript in the same no-capability sandbox as expressions.
//! The `code` param is deliberately NEVER template-resolved: JS source is code,
//! not a template, and `{{` sequences inside it must stay literal.

use super::{Needs, Node, NodeSpec, MAIN_IN, MAIN_OUT};
use crate::engine::NodeCtx;
use crate::error::{ErrorKind, NodeError};
use crate::items::{Item, NodeInput, NodeOutput};
use crate::runs::LogLevel;
use serde_json::Value;
use std::time::Duration;

pub struct CodeNode;

const CONSOLE_SHIM: &str = "globalThis.__logs = [];\n\
globalThis.console = { log: function() { __logs.push(Array.prototype.map.call(arguments, function(a) {\n\
  try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (e) { return String(a); }\n\
}).join(' ')); } };\nconsole.warn = console.log; console.error = console.log; console.info = console.log;";

impl Node for CodeNode {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "code.js",
            type_version: 1,
            label: "Code",
            category: "code",
            description: "Runs JavaScript over all items or once per item, sandboxed.",
            inputs: MAIN_IN,
            outputs: MAIN_OUT,
            needs: Needs { network: false, code: true, fs_read: false, fs_write: false, agent: false },
            is_trigger: false,
            secrets_ok: false,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError> {
        let raw = ctx.params_raw().clone();
        let code = raw
            .get("code")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| NodeError::params("write some code first"))?
            .to_string();
        let mode = raw.get("mode").and_then(|v| v.as_str()).unwrap_or("all_items").to_string();
        let timeout = Duration::from_secs(
            raw.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(5).clamp(1, 60),
        );

        // Fresh, permission-checked context with $items/$run/$node injected.
        let scope = ctx.code_scope()?;
        scope
            .set_prelude(CONSOLE_SHIM)
            .map_err(|e| NodeError::new(ErrorKind::Code, e))?;

        let mut items: Vec<Item> = Vec::new();
        if mode == "per_item" {
            scope
                .set_prelude(&format!("globalThis.__fn = function(item, index) {{\n{code}\n}};"))
                .map_err(|e| NodeError::new(ErrorKind::Code, format!("code does not parse: {e}")))?;
            for i in 0..input.main().len() {
                ctx.check_cancel()?;
                let result = scope
                    .eval_code(&format!("__fn($items[{i}], {i})"), timeout)
                    .map_err(|e| NodeError::new(ErrorKind::Code, e).at_item(i))?;
                if !result.is_null() {
                    items.push(Item::new(result));
                }
            }
        } else {
            let result = scope
                .eval_code(&format!("(function(items) {{\n{code}\n}})($items)"), timeout)
                .map_err(|e| NodeError::new(ErrorKind::Code, e))?;
            match result {
                Value::Array(values) => items.extend(values.into_iter().map(Item::new)),
                Value::Null => {}
                other => {
                    return Err(NodeError::new(
                        ErrorKind::Code,
                        format!("code must return an array of items, got {}", kind_name(&other)),
                    ))
                }
            }
        }

        // Surface console.log output into the run log.
        if let Ok(Value::Array(logs)) = scope.eval_code("__logs", Duration::from_secs(2)) {
            for line in logs.iter().filter_map(|l| l.as_str()) {
                ctx.log(LogLevel::Info, line);
            }
        }
        Ok(NodeOutput::main(items))
    }
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}
