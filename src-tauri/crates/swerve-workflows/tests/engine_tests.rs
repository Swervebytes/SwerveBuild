//! End-to-end engine semantics, driven purely through the public API — this is
//! the "run a workflow from JSON with no canvas" gate from the design doc.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use swerve_workflows::engine::{run_workflow, CancelFlag, EngineConfig, TriggerFire};
use swerve_workflows::model::Workflow;
use swerve_workflows::runs::{NodeRunStatus, RunStatus};
use swerve_workflows::services::{AgentRequest, AgentResult, AgentRunner, EngineServices};
use swerve_workflows::validate::validate;

fn temp_runs_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("swf-tests-{tag}-{}", uuid()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))
}

fn workflow(nodes: Value, connections: Value) -> Workflow {
    workflow_with(nodes, connections, json!({}), json!({}))
}

fn workflow_with(nodes: Value, connections: Value, permissions: Value, settings_over: Value) -> Workflow {
    let mut settings = json!({ "capture": "full" });
    if let (Some(base), Some(over)) = (settings.as_object_mut(), settings_over.as_object()) {
        for (k, v) in over {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(json!({
        "id": "w-test",
        "name": "Test workflow",
        "nodes": nodes,
        "connections": connections,
        "permissions": permissions,
        "settings": settings,
    }))
    .unwrap()
}

fn node(id: &str, node_type: &str, name: &str, params: Value) -> Value {
    json!({ "id": id, "type": node_type, "name": name, "params": params })
}

fn manual_fire() -> TriggerFire {
    TriggerFire::manual("t1")
}

fn run(workflow: &Workflow, tag: &str) -> swerve_workflows::runs::RunRecord {
    let cfg = EngineConfig { runs_dir: temp_runs_dir(tag), services: EngineServices::default() };
    run_workflow(workflow, manual_fire(), &cfg, &CancelFlag::new())
}

/// Captured output items for a node+port from record.data.
fn data_items<'a>(record: &'a swerve_workflows::runs::RunRecord, node_id: &str, port: &str) -> Vec<&'a Value> {
    record
        .data
        .get(node_id)
        .and_then(|n| n.get(port))
        .and_then(|p| p.get("items"))
        .and_then(|i| i.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- happy path

#[test]
fn branching_flow_routes_and_merges() {
    // trigger → set(n=1,tag) → if(n eq 1) → true: set(route=yes) ; false: set(route=no) → merge
    let w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "transform.set", "Prep", json!({ "ops": [
                { "op": "set", "path": "n", "value": 1 },
                { "op": "set", "path": "tag", "value": "made by {{ $run.workflow.name }}" }
            ]})),
            node("n2", "flow.if", "Check", json!({ "combine": "and", "conditions": [
                { "left": "{{ $json.n }}", "op": "eq", "right": 1 }
            ]})),
            node("n3", "transform.set", "Yes", json!({ "ops": [ { "op": "set", "path": "route", "value": "yes" } ]})),
            node("n4", "transform.set", "No", json!({ "ops": [ { "op": "set", "path": "route", "value": "no" } ]})),
            node("n5", "flow.merge", "Join", json!({ "mode": "append" })),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
            { "from": "n2", "out": "true", "to": "n3", "in": "main" },
            { "from": "n2", "out": "false", "to": "n4", "in": "main" },
            { "from": "n3", "out": "main", "to": "n5", "in": "a" },
            { "from": "n4", "out": "main", "to": "n5", "in": "b" },
        ]),
    );
    let record = run(&w, "branch");
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);

    // Every reachable node executed exactly once.
    assert_eq!(record.nodes.len(), 6);
    assert!(record.nodes.iter().all(|n| n.status == NodeRunStatus::Success));

    // The untaken branch ran with zero items (empty in → empty out).
    let no_branch = record.nodes.iter().find(|n| n.node_id == "n4").unwrap();
    assert_eq!(no_branch.items_in, 0);
    assert_eq!(no_branch.items_out, 0);

    // Merge output: one item, routed yes, template resolved with typed values.
    let merged = data_items(&record, "n5", "main");
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["route"], json!("yes"));
    assert_eq!(merged[0]["n"], json!(1));
    assert_eq!(merged[0]["tag"], json!("made by Test workflow"));
}

#[test]
fn execution_order_is_deterministic() {
    // A diamond where both arms are ready simultaneously — lowest node id first.
    let w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("nb", "transform.set", "B", json!({ "ops": [] })),
            node("na", "transform.set", "A", json!({ "ops": [] })),
            node("nz", "flow.merge", "Z", json!({ "mode": "append" })),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "nb", "in": "main" },
            { "from": "t1", "out": "main", "to": "na", "in": "main" },
            { "from": "na", "out": "main", "to": "nz", "in": "a" },
            { "from": "nb", "out": "main", "to": "nz", "in": "b" },
        ]),
    );
    let first: Vec<String> = run(&w, "det1").nodes.iter().map(|n| n.node_id.clone()).collect();
    let second: Vec<String> = run(&w, "det2").nodes.iter().map(|n| n.node_id.clone()).collect();
    assert_eq!(first, second);
    assert_eq!(first, vec!["t1", "na", "nb", "nz"], "ready set must drain in id order");
}

#[test]
fn node_expression_reads_earlier_output() {
    let w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "transform.set", "Source", json!({ "ops": [ { "op": "set", "path": "x", "value": 41 } ]})),
            node("n2", "transform.set", "Reader", json!({ "ops": [
                { "op": "set", "path": "y", "value": "{{ $node(\"Source\").first().x + 1 }}" }
            ]})),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
        ]),
    );
    let record = run(&w, "nodefn");
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);
    assert_eq!(data_items(&record, "n2", "main")[0]["y"], json!(42));
}

// ---------------------------------------------------------------- error policy

fn failing_code_workflow(on_error: &str) -> Workflow {
    let mut w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "code.js", "Boom", json!({ "mode": "all_items", "code": "throw new Error('kaput');" })),
            node("n2", "transform.set", "After", json!({ "ops": [ { "op": "set", "path": "ok", "value": true } ]})),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
        ]),
        json!({ "code": true }),
        json!({}),
    );
    if let Some(n1) = w.nodes.iter_mut().find(|n| n.id == "n1") {
        n1.on_error = serde_json::from_value(json!(on_error)).unwrap();
    }
    w
}

#[test]
fn on_error_stop_fails_the_run_and_skips_downstream() {
    let record = run(&failing_code_workflow("stop"), "stop");
    assert_eq!(record.status, RunStatus::Error);
    let err = record.error.as_ref().unwrap();
    assert_eq!(err.node_id, "n1");
    assert!(err.error.message.contains("kaput"), "got: {}", err.error.message);
    let after = record.nodes.iter().find(|n| n.node_id == "n2").unwrap();
    assert_eq!(after.status, NodeRunStatus::Skipped);
}

#[test]
fn on_error_skip_continues_with_empty_output() {
    let record = run(&failing_code_workflow("skip"), "skip");
    assert_eq!(record.status, RunStatus::Success);
    let boom = record.nodes.iter().find(|n| n.node_id == "n1").unwrap();
    assert_eq!(boom.status, NodeRunStatus::Error);
    let after = record.nodes.iter().find(|n| n.node_id == "n2").unwrap();
    assert_eq!(after.status, NodeRunStatus::Success);
    assert_eq!(after.items_in, 0);
}

#[test]
fn on_error_branch_routes_an_error_item() {
    let mut w = failing_code_workflow("branch");
    // Rewire: error port → After.
    w.connections[1] = serde_json::from_value(
        json!({ "from": "n1", "out": "error", "to": "n2", "in": "main" }),
    )
    .unwrap();
    let record = run(&w, "branch-err");
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);
    let after = record.nodes.iter().find(|n| n.node_id == "n2").unwrap();
    assert_eq!(after.items_in, 1);
    let out = data_items(&record, "n2", "main");
    assert!(out[0]["error"]["message"].as_str().unwrap().contains("kaput"));
}

// ---------------------------------------------------------------- code node

#[test]
fn code_node_transforms_and_logs() {
    let w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "code.js", "Make", json!({ "mode": "all_items",
                "code": "console.log('hello from code'); return [{a: 1}, {a: 2}];" })),
            node("n2", "code.js", "Double", json!({ "mode": "per_item",
                "code": "return { a: item.a * 2, i: index };" })),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
        ]),
        json!({ "code": true }),
        json!({}),
    );
    let record = run(&w, "code");
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);
    let out = data_items(&record, "n2", "main");
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["a"], json!(2));
    assert_eq!(out[1]["a"], json!(4));
    assert_eq!(out[1]["i"], json!(1));
}

// ---------------------------------------------------------------- permissions

#[test]
fn validation_blocks_missing_permissions_per_node() {
    let w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "http.request", "Fetch", json!({ "url": "https://example.com" })),
            node("n2", "code.js", "Calc", json!({ "code": "return [];" })),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
        ]),
    );
    let v = validate(&w);
    assert!(!v.ok());
    let text = v.error_summary();
    assert!(text.contains("Fetch") && text.contains("network"), "got: {text}");
    assert!(text.contains("Calc") && text.contains("code"), "got: {text}");

    // Running it anyway fails cleanly with the same message.
    let record = run(&w, "perm");
    assert_eq!(record.status, RunStatus::Error);
}

#[test]
fn secret_helper_is_unavailable_outside_secret_capable_nodes() {
    let w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "transform.set", "Sneak", json!({ "ops": [
                { "op": "set", "path": "s", "value": "{{ $secret('github') }}" }
            ]})),
        ]),
        json!([ { "from": "t1", "out": "main", "to": "n1", "in": "main" } ]),
    );
    let record = run(&w, "secret");
    assert_eq!(record.status, RunStatus::Error);
    let msg = &record.error.as_ref().unwrap().error.message;
    assert!(msg.contains("$secret"), "got: {msg}");
}

#[test]
fn validation_rejects_cycles_duplicates_and_unknown_types() {
    let w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "transform.set", "Same", json!({})),
            node("n2", "transform.set", "Same", json!({})),
            node("n3", "no.such.type", "Ghost", json!({})),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
            { "from": "n2", "out": "main", "to": "n1", "in": "main" },
        ]),
    );
    let v = validate(&w);
    let text = v.error_summary();
    assert!(text.contains("cycle"), "got: {text}");
    assert!(text.contains("duplicate node name"), "got: {text}");
    assert!(text.contains("unknown node type"), "got: {text}");
}

// ---------------------------------------------------------------- files

#[test]
fn file_write_then_read_inside_grants() {
    let dir = temp_runs_dir("fs-data");
    let dir_str = dir.to_string_lossy().to_string();
    let w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "transform.set", "Compose", json!({ "ops": [
                { "op": "set", "path": "text", "value": "written by the workflow" }
            ]})),
            node("n2", "file.write", "Save", json!({
                "path": format!("{dir_str}/out.txt"),
                "content": "{{ $json.text }}",
                "mode": "overwrite"
            })),
            node("n3", "file.read", "Load", json!({ "path": format!("{dir_str}/out.txt") })),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
            { "from": "n2", "out": "main", "to": "n3", "in": "main" },
        ]),
        json!({ "fs": { "read": [dir_str], "write": [dir_str] } }),
        json!({}),
    );
    let record = run(&w, "fs");
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);
    let loaded = data_items(&record, "n3", "main");
    assert_eq!(loaded[0]["text"], json!("written by the workflow"));
}

// ---------------------------------------------------------------- agent seam

struct FakeAgent;

impl AgentRunner for FakeAgent {
    fn run(&self, req: AgentRequest, _cancel: &CancelFlag) -> Result<AgentResult, String> {
        Ok(AgentResult {
            text: format!("echo: {}", req.prompt),
            structured: None,
            stop_reason: Some("end".into()),
            session_id: None,
        })
    }
}

#[test]
fn agent_node_uses_the_injected_runner() {
    let w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "agent.run", "Ask", json!({
                "prompt": "summarize {{ $items.length }} items",
                "cwd": std::env::temp_dir().to_string_lossy(),
            })),
        ]),
        json!([ { "from": "t1", "out": "main", "to": "n1", "in": "main" } ]),
        json!({ "agent": true }),
        json!({}),
    );
    let cfg = EngineConfig {
        runs_dir: temp_runs_dir("agent"),
        services: EngineServices { agent: Some(Arc::new(FakeAgent)), ..Default::default() },
    };
    let record = run_workflow(&w, manual_fire(), &cfg, &CancelFlag::new());
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);
    assert_eq!(
        data_items(&record, "n1", "main")[0]["text"],
        json!("echo: summarize 1 items")
    );
}

// ---------------------------------------------------------------- cancellation

#[test]
fn cancel_mid_wait_stops_the_run() {
    let w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "util.wait", "Nap", json!({ "seconds": 30 })),
            node("n2", "transform.set", "Never", json!({ "ops": [] })),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n1", "in": "main" },
            { "from": "n1", "out": "main", "to": "n2", "in": "main" },
        ]),
    );
    let cfg = EngineConfig { runs_dir: temp_runs_dir("cancel"), services: EngineServices::default() };
    let cancel = Arc::new(CancelFlag::new());
    let canceller = Arc::clone(&cancel);
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        canceller.cancel();
    });
    let started = std::time::Instant::now();
    let record = run_workflow(&w, manual_fire(), &cfg, &cancel);
    t.join().unwrap();
    assert_eq!(record.status, RunStatus::Cancelled);
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "cancel must not wait out the nap");
    let never = record.nodes.iter().find(|n| n.node_id == "n2").unwrap();
    assert_eq!(never.status, NodeRunStatus::Skipped);
}

// ---------------------------------------------------------------- run records

#[test]
fn run_records_and_logs_land_on_disk_and_prune() {
    let runs_dir = temp_runs_dir("records");
    let w = workflow(
        json!([ node("t1", "trigger.manual", "Start", json!({})) ]),
        json!([]),
    );
    let cfg = EngineConfig { runs_dir: runs_dir.clone(), services: EngineServices::default() };
    for _ in 0..3 {
        let record = run_workflow(&w, manual_fire(), &cfg, &CancelFlag::new());
        assert_eq!(record.status, RunStatus::Success);
        assert!(runs_dir.join("w-test").join(format!("{}.json", record.id)).is_file());
        assert!(runs_dir.join("w-test").join(format!("{}.jsonl", record.id)).is_file());
    }
    let store = swerve_workflows::runs::RunStore::new(runs_dir);
    assert_eq!(store.list_records("w-test").len(), 3);
    store.prune("w-test", 1);
    assert_eq!(store.list_records("w-test").len(), 1);
}

// ---------------------------------------------------------------- live HTTP

use std::io::{Read as _, Write as _};
use std::net::TcpListener;

/// Tiny fixture server: /json → 200 json, /hop → 302 to /json.
fn spawn_http_fixture() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for _ in 0..8 {
            let Ok((mut stream, _)) = listener.accept() else { break };
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = if req.starts_with("GET /hop") {
                format!("HTTP/1.1 302 Found\r\nLocation: /json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            } else {
                let body = r#"{"ok":true,"from":"fixture"}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = stream.write_all(response.as_bytes());
            if req.starts_with("GET /done") {
                break;
            }
        }
    });
    (format!("127.0.0.1:{}", addr.port()), handle)
}

#[test]
fn http_node_follows_redirects_and_parses_json_with_private_optin() {
    let (host, _handle) = spawn_http_fixture();
    let w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "http.request", "Fetch", json!({ "url": format!("http://{host}/hop") })),
        ]),
        json!([ { "from": "t1", "out": "main", "to": "n1", "in": "main" } ]),
        json!({ "network": { "enabled": true, "hosts": ["127.0.0.1"], "private_ips": true } }),
        json!({}),
    );
    let record = run(&w, "http-ok");
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);
    let out = data_items(&record, "n1", "main");
    assert_eq!(out[0]["status"], json!(200));
    assert_eq!(out[0]["body"]["from"], json!("fixture"));
}

#[test]
fn http_node_blocks_private_targets_without_the_optin() {
    let (host, _handle) = spawn_http_fixture();
    let w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "http.request", "Fetch", json!({ "url": format!("http://{host}/json") })),
        ]),
        json!([ { "from": "t1", "out": "main", "to": "n1", "in": "main" } ]),
        json!({ "network": { "enabled": true, "hosts": [], "private_ips": false } }),
        json!({}),
    );
    let record = run(&w, "http-ssrf");
    assert_eq!(record.status, RunStatus::Error);
    let msg = &record.error.as_ref().unwrap().error.message;
    assert!(msg.contains("private"), "got: {msg}");
}

// ---------------------------------------------------------------- retry / cancel

#[test]
fn cancel_during_retry_backoff_reports_cancelled_not_error() {
    // A node that fails then sits in a long backoff must, when cancelled, finalize
    // the run as Cancelled — not as a spurious node error from the prior attempt.
    let mut w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "code.js", "Flaky", json!({ "mode": "all_items", "code": "throw new Error('nope');" })),
        ]),
        json!([ { "from": "t1", "out": "main", "to": "n1", "in": "main" } ]),
        json!({ "code": true }),
        json!({}),
    );
    if let Some(n1) = w.nodes.iter_mut().find(|n| n.id == "n1") {
        n1.retry = Some(serde_json::from_value(json!({ "attempts": 1, "backoff_secs": [30] })).unwrap());
    }
    let cfg = EngineConfig { runs_dir: temp_runs_dir("retry-cancel"), services: EngineServices::default() };
    let cancel = Arc::new(CancelFlag::new());
    let canceller = Arc::clone(&cancel);
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        canceller.cancel();
    });
    let started = std::time::Instant::now();
    let record = run_workflow(&w, manual_fire(), &cfg, &cancel);
    t.join().unwrap();
    assert_eq!(record.status, RunStatus::Cancelled, "error: {:?}", record.error);
    assert!(record.error.is_none(), "cancel must not record a node abort");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "must not wait out the 30s backoff");
}

#[test]
fn attempts_records_actual_count_not_the_maximum() {
    let mut w = workflow(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "transform.set", "Ok", json!({ "ops": [] })),
        ]),
        json!([ { "from": "t1", "out": "main", "to": "n1", "in": "main" } ]),
    );
    if let Some(n1) = w.nodes.iter_mut().find(|n| n.id == "n1") {
        n1.retry = Some(serde_json::from_value(json!({ "attempts": 3, "backoff_secs": [1] })).unwrap());
    }
    let record = run(&w, "attempts");
    let n1 = record.nodes.iter().find(|n| n.node_id == "n1").unwrap();
    assert_eq!(n1.attempts, 1, "succeeded first try → one attempt, not max");
}

#[test]
fn set_node_branch_keeps_successes_and_routes_the_failure() {
    // Per-item failure under branch policy routes that item to `error` while the
    // items that succeeded keep flowing on `main` (was: whole node failed).
    let mut w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n0", "code.js", "Seed", json!({ "mode": "all_items",
                "code": "return [{key:'a'}, {nope:1}, {key:'c'}];" })),
            node("n1", "transform.set", "Edit", json!({ "ops": [
                { "op": "set", "path": "{{ $json.key }}", "value": 1 }
            ]})),
        ]),
        json!([
            { "from": "t1", "out": "main", "to": "n0", "in": "main" },
            { "from": "n0", "out": "main", "to": "n1", "in": "main" },
        ]),
        json!({ "code": true }),
        json!({}),
    );
    if let Some(n1) = w.nodes.iter_mut().find(|n| n.id == "n1") {
        n1.on_error = serde_json::from_value(json!("branch")).unwrap();
    }
    let record = run(&w, "set-branch");
    assert_eq!(record.status, RunStatus::Success, "error: {:?}", record.error);
    assert_eq!(data_items(&record, "n1", "main").len(), 2, "two items succeed");
    let errs = data_items(&record, "n1", "error");
    assert_eq!(errs.len(), 1, "one item routed to error");
    assert!(errs[0]["error"]["message"].as_str().unwrap().contains("path"));
}

#[test]
fn http_node_enforces_the_host_allowlist() {
    let (host, _handle) = spawn_http_fixture();
    let w = workflow_with(
        json!([
            node("t1", "trigger.manual", "Start", json!({})),
            node("n1", "http.request", "Fetch", json!({ "url": format!("http://{host}/json") })),
        ]),
        json!([ { "from": "t1", "out": "main", "to": "n1", "in": "main" } ]),
        json!({ "network": { "enabled": true, "hosts": ["api.github.com"], "private_ips": true } }),
        json!({}),
    );
    let record = run(&w, "http-host");
    assert_eq!(record.status, RunStatus::Error);
    let msg = &record.error.as_ref().unwrap().error.message;
    assert!(msg.contains("allowed hosts"), "got: {msg}");
}
