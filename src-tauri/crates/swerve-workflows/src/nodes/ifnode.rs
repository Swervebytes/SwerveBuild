//! IF — routes each item to the true or false output by a condition list.

use super::{Needs, Node, NodeSpec, PortSpec, MAIN_IN};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{Item, NodeInput, NodeOutput};
use serde_json::{json, Value};

pub struct IfNode;

static OUTPUTS: &[PortSpec] = &[
    PortSpec { name: "true", label: "True" },
    PortSpec { name: "false", label: "False" },
];

impl Node for IfNode {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "flow.if",
            type_version: 1,
            label: "IF",
            category: "flow",
            description: "Routes each item to True or False by comparing values.",
            inputs: MAIN_IN,
            outputs: OUTPUTS,
            needs: Needs { network: false, code: false, fs_read: false, fs_write: false, agent: false },
            is_trigger: false,
            secrets_ok: false,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError> {
        let mut out = NodeOutput::default();
        out.set_port("true", Vec::new());
        out.set_port("false", Vec::new());
        for (i, item) in input.main().iter().enumerate() {
            ctx.check_cancel()?;
            let params = ctx.params(i)?;
            let combine = params.get("combine").and_then(|v| v.as_str()).unwrap_or("and");
            let empty = Vec::new();
            let conditions = params.get("conditions").and_then(|v| v.as_array()).unwrap_or(&empty);

            let mut verdict = combine != "or"; // and starts true, or starts false
            let mut failed: Option<NodeError> = None;
            for cond in conditions {
                let left = cond.get("left").cloned().unwrap_or(Value::Null);
                let op = cond.get("op").and_then(|v| v.as_str()).unwrap_or("eq");
                let right = cond.get("right").cloned().unwrap_or(Value::Null);
                let hit = match compare(ctx, &left, op, &right) {
                    Ok(hit) => hit,
                    Err(e) => {
                        failed = Some(NodeError::params(e).at_item(i));
                        break;
                    }
                };
                if combine == "or" {
                    verdict = verdict || hit;
                    if verdict {
                        break;
                    }
                } else {
                    verdict = verdict && hit;
                    if !verdict {
                        break;
                    }
                }
            }
            match failed {
                None => out.push(if verdict { "true" } else { "false" }, item.clone()),
                Some(err) if ctx.branch_errors() => out.push(
                    "error",
                    Item::new(json!({
                        "error": { "kind": err.kind, "message": err.message },
                        "item": item.json.clone(),
                    })),
                ),
                Some(err) => return Err(err),
            }
        }
        Ok(out)
    }
}

fn as_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn loose_eq(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (as_num(a), as_num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn stringify(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn compare(ctx: &mut NodeCtx, left: &Value, op: &str, right: &Value) -> Result<bool, String> {
    Ok(match op {
        "eq" => loose_eq(left, right),
        "ne" => !loose_eq(left, right),
        "gt" | "gte" | "lt" | "lte" => {
            let ordering = match (as_num(left), as_num(right)) {
                (Some(a), Some(b)) => a.partial_cmp(&b).ok_or("values are not comparable")?,
                _ => stringify(left).cmp(&stringify(right)),
            };
            match op {
                "gt" => ordering.is_gt(),
                "gte" => ordering.is_ge(),
                "lt" => ordering.is_lt(),
                _ => ordering.is_le(),
            }
        }
        "contains" | "notcontains" => {
            let hit = match left {
                Value::String(s) => s.contains(&stringify(right)),
                Value::Array(a) => a.iter().any(|v| loose_eq(v, right)),
                Value::Object(o) => o.contains_key(&stringify(right)),
                _ => false,
            };
            if op == "contains" {
                hit
            } else {
                !hit
            }
        }
        "exists" => !left.is_null(),
        "notexists" => left.is_null(),
        "matches" => ctx.regex_test(&stringify(right), &stringify(left))?,
        other => return Err(format!("unknown comparison {other}")),
    })
}
