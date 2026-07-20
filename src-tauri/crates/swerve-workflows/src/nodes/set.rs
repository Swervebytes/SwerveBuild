//! Edit Fields (Set) — per-item field surgery with dotted paths.

use super::{Needs, Node, NodeSpec, MAIN_IN, MAIN_OUT};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{Item, NodeInput, NodeOutput};
use serde_json::{json, Map, Value};

pub struct SetFields;

impl Node for SetFields {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "transform.set",
            type_version: 1,
            label: "Edit Fields",
            category: "transform",
            description: "Set, rename, remove, or keep fields on every item.",
            inputs: MAIN_IN,
            outputs: MAIN_OUT,
            needs: Needs { network: false, code: false, fs_read: false, fs_write: false, agent: false },
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
            let mut json = item.json.clone();
            let empty = Vec::new();
            let mut failed: Option<NodeError> = None;
            for op in params.get("ops").and_then(|v| v.as_array()).unwrap_or(&empty) {
                if let Err(e) = apply_op(&mut json, op) {
                    failed = Some(NodeError::params(e).at_item(i));
                    break;
                }
            }
            match failed {
                None => out.push("main", Item::new(json)),
                // Per-item failure with branch policy: route this item to `error`,
                // keep the ones that succeeded flowing on `main` (matches http/file).
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

fn apply_op(json: &mut Value, op: &Value) -> Result<(), String> {
    let kind = op.get("op").and_then(|v| v.as_str()).unwrap_or("set");
    match kind {
        "set" => {
            let path = op.get("path").and_then(|v| v.as_str()).ok_or("set op needs a path")?;
            let value = op.get("value").cloned().unwrap_or(Value::Null);
            set_path(json, path, value);
        }
        "rename" => {
            let path = op.get("path").and_then(|v| v.as_str()).ok_or("rename op needs a path")?;
            let to = op.get("to").and_then(|v| v.as_str()).ok_or("rename op needs a target name")?;
            if let Some(value) = get_path(json, path).cloned() {
                remove_path(json, path);
                set_path(json, to, value);
            }
        }
        "remove" => {
            let path = op.get("path").and_then(|v| v.as_str()).ok_or("remove op needs a path")?;
            remove_path(json, path);
        }
        "keep" => {
            let paths: Vec<&str> = op
                .get("paths")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|p| p.as_str()).collect())
                .unwrap_or_default();
            let mut kept = Value::Object(Map::new());
            for path in paths {
                if let Some(value) = get_path(json, path).cloned() {
                    set_path(&mut kept, path, value);
                }
            }
            *json = kept;
        }
        other => return Err(format!("unknown op {other}")),
    }
    Ok(())
}

/// Dotted-path get; numeric segments index arrays.
pub fn get_path<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = json;
    for seg in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(seg)?,
            Value::Array(arr) => arr.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Dotted-path set; creates intermediate objects for missing segments.
pub fn set_path(json: &mut Value, path: &str, value: Value) {
    let segments: Vec<&str> = path.split('.').collect();
    let mut current = json;
    for (idx, seg) in segments.iter().enumerate() {
        let last = idx == segments.len() - 1;
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        let map = current.as_object_mut().expect("just ensured object");
        if last {
            map.insert((*seg).to_string(), value);
            return;
        }
        current = map.entry((*seg).to_string()).or_insert(Value::Object(Map::new()));
    }
}

pub fn remove_path(json: &mut Value, path: &str) {
    let Some((parent_path, leaf)) = path.rsplit_once('.') else {
        if let Value::Object(map) = json {
            map.remove(path);
        }
        return;
    };
    let mut current = json;
    for seg in parent_path.split('.') {
        current = match current {
            Value::Object(map) => match map.get_mut(seg) {
                Some(v) => v,
                None => return,
            },
            Value::Array(arr) => match seg.parse::<usize>().ok().and_then(|i| arr.get_mut(i)) {
                Some(v) => v,
                None => return,
            },
            _ => return,
        };
    }
    if let Value::Object(map) = current {
        map.remove(leaf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn path_helpers() {
        let mut v = json!({"a": {"b": 1}, "list": [10, 20]});
        assert_eq!(get_path(&v, "a.b"), Some(&json!(1)));
        assert_eq!(get_path(&v, "list.1"), Some(&json!(20)));
        assert_eq!(get_path(&v, "missing.x"), None);

        set_path(&mut v, "a.c.d", json!("deep"));
        assert_eq!(get_path(&v, "a.c.d"), Some(&json!("deep")));

        remove_path(&mut v, "a.b");
        assert_eq!(get_path(&v, "a.b"), None);
        assert_eq!(get_path(&v, "a.c.d"), Some(&json!("deep")));
    }

    #[test]
    fn keep_op_builds_a_fresh_object() {
        let mut v = json!({"keep": {"me": 1}, "drop": 2, "also": 3});
        apply_op(&mut v, &json!({"op": "keep", "paths": ["keep.me"]})).unwrap();
        assert_eq!(v, json!({"keep": {"me": 1}}));
    }
}
