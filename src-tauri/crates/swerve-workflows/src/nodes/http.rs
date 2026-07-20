//! HTTP Request — one permission-checked request per input item.

use super::{Needs, Node, NodeSpec, MAIN_IN, MAIN_OUT};
use crate::engine::NodeCtx;
use crate::error::NodeError;
use crate::items::{Item, NodeInput, NodeOutput};
use crate::permissions::{HttpRequestSpec, DEFAULT_HTTP_TIMEOUT_SECS};
use serde_json::{json, Map, Value};

pub struct HttpRequest;

impl Node for HttpRequest {
    fn spec(&self) -> &'static NodeSpec {
        static SPEC: NodeSpec = NodeSpec {
            type_name: "http.request",
            type_version: 1,
            label: "HTTP Request",
            category: "action",
            description: "Calls a URL once per item and outputs status, headers, and body.",
            inputs: MAIN_IN,
            outputs: MAIN_OUT,
            needs: Needs { network: true, code: false, fs_read: false, fs_write: false, agent: false },
            is_trigger: false,
            secrets_ok: true,
        };
        &SPEC
    }

    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError> {
        let mut out = NodeOutput::default();
        out.set_port("main", Vec::new());
        for (i, item) in input.main().iter().enumerate() {
            ctx.check_cancel()?;
            let params = ctx.params(i)?;
            let spec = build_spec(&params).map_err(|e| NodeError::params(e).at_item(i))?;
            match ctx.http_request(&spec) {
                Ok(resp) => {
                    let headers: Map<String, Value> = resp
                        .headers
                        .into_iter()
                        .map(|(k, v)| (k, Value::String(v)))
                        .collect();
                    out.push(
                        "main",
                        Item::new(json!({
                            "status": resp.status,
                            "headers": headers,
                            "body": resp.body,
                            "url": resp.url,
                        })),
                    );
                }
                Err(err) if ctx.branch_errors() => {
                    out.push(
                        "error",
                        Item::new(json!({
                            "error": { "kind": err.kind, "message": err.message },
                            "item": item.json.clone(),
                        })),
                    );
                }
                Err(err) => return Err(err.at_item(i)),
            }
        }
        Ok(out)
    }
}

fn build_spec(params: &Value) -> Result<HttpRequestSpec, String> {
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("a URL is required")?
        .trim()
        .to_string();
    let to_pairs = |key: &str| -> Vec<(String, String)> {
        params
            .get(key)
            .and_then(|v| v.as_object())
            .map(|o| {
                o.iter()
                    .map(|(k, v)| {
                        let value = match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (k.clone(), value)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(HttpRequestSpec {
        method: params
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_string(),
        url,
        headers: to_pairs("headers"),
        query: to_pairs("query"),
        body_type: params
            .get("body_type")
            .and_then(|v| v.as_str())
            .unwrap_or("none")
            .to_string(),
        body: params.get("body").cloned().unwrap_or(Value::Null),
        timeout_secs: params
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS),
    })
}
