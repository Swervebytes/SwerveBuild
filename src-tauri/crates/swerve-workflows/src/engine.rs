//! The executor: deterministic ready-queue scheduling over a validated DAG.
//! One run = one call to [`run_workflow`] on one thread; nodes execute
//! sequentially in a stable order (among ready nodes, lowest id first).

use crate::error::{ErrorKind, NodeError};
use crate::expr::{ExprEngine, ExprScope};
use crate::items::{Item, NodeInput, NodeOutput};
use crate::model::{NodeDef, OnError, Permissions, Workflow};
use crate::nodes;
use crate::permissions::{self, HttpRequestSpec, HttpResponseData};
use crate::runs::{
    CapturedPort, LogLevel, NodeRunStatus, NodeRunSummary, RunErrorInfo, RunEvent, RunRecord, RunStatus,
    RunStore, TriggerInfo,
};
use crate::services::{AgentRunner, EngineServices};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const SAMPLE_ITEMS_PER_PORT: usize = 20;
const SAMPLE_BYTES_PER_NODE: usize = 256 * 1024;

// --------------------------------------------------------------- cancellation

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    None,
    Cancelled,
    Timeout,
}

/// Cooperative stop flag shared between the run thread and its controller.
/// The FIRST reason wins (a cancel arriving after a timeout stays a timeout).
#[derive(Default)]
pub struct CancelFlag(AtomicU8);

impl CancelFlag {
    pub fn new() -> Self {
        CancelFlag(AtomicU8::new(0))
    }
    pub fn cancel(&self) {
        let _ = self.0.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
    }
    pub fn timeout(&self) {
        let _ = self.0.compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
    }
    pub fn reason(&self) -> CancelReason {
        match self.0.load(Ordering::SeqCst) {
            1 => CancelReason::Cancelled,
            2 => CancelReason::Timeout,
            _ => CancelReason::None,
        }
    }
    pub fn is_set(&self) -> bool {
        self.reason() != CancelReason::None
    }
}

// --------------------------------------------------------------- run inputs

/// Why (and from which trigger node) a run started. Built by the scheduler,
/// the manual Run button, or the CLI.
#[derive(Debug, Clone, Default)]
pub struct TriggerFire {
    pub node_id: String,
    pub kind: String,
    pub reason: String,
    /// Seed item json; Null → the engine synthesizes `{"trigger": {…}}`.
    pub payload: Value,
    /// Caller-assigned run id (so a manager can track/cancel a queued run
    /// before the engine thread starts). None → the engine generates one.
    pub run_id: Option<String>,
}

impl TriggerFire {
    pub fn manual(node_id: impl Into<String>) -> Self {
        TriggerFire {
            node_id: node_id.into(),
            kind: "manual".into(),
            reason: "manual".into(),
            payload: Value::Null,
            run_id: None,
        }
    }
}

pub struct EngineConfig {
    pub runs_dir: PathBuf,
    pub services: EngineServices,
}

#[derive(Debug, Clone)]
pub struct RunInfo {
    pub run_id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub trigger: TriggerInfo,
    pub started_at: String,
}

// --------------------------------------------------------------- node context

/// Everything a node may touch. Capability enters ONLY through these methods —
/// each one checks the workflow's permission grants before acting.
pub struct NodeCtx<'r> {
    pub node: &'r NodeDef,
    run: &'r RunInfo,
    cancel: &'r CancelFlag,
    deadline: Instant,
    permissions: &'r Permissions,
    services: &'r EngineServices,
    expr_engine: &'r ExprEngine,
    scope: ExprScope<'r>,
    primary_json: Vec<Value>,
    store: &'r RunStore,
    fire_payload: Option<Value>,
    wants_node_fn: bool,
    wants_secret_fn: bool,
    node_outputs_json: &'r BTreeMap<String, Value>,
}

impl<'r> NodeCtx<'r> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        node: &'r NodeDef,
        run: &'r RunInfo,
        cancel: &'r CancelFlag,
        deadline: Instant,
        permissions: &'r Permissions,
        services: &'r EngineServices,
        expr_engine: &'r ExprEngine,
        store: &'r RunStore,
        input: &NodeInput,
        fire_payload: Option<Value>,
        node_outputs_json: &'r BTreeMap<String, Value>,
    ) -> Result<Self, NodeError> {
        let scope = expr_engine.scope().map_err(NodeError::internal)?;
        let primary_json: Vec<Value> = input.main().iter().map(|i| i.json.clone()).collect();
        let params_text = node.params.to_string();
        let secrets_ok = nodes::get(&node.node_type).map(|n| n.spec().secrets_ok).unwrap_or(false);
        let ctx = NodeCtx {
            node,
            run,
            cancel,
            deadline,
            permissions,
            services,
            expr_engine,
            scope,
            primary_json,
            store,
            fire_payload,
            wants_node_fn: params_text.contains("$node("),
            wants_secret_fn: secrets_ok && params_text.contains("$secret("),
            node_outputs_json,
        };
        ctx.prepare_scope(&ctx.scope)?;
        Ok(ctx)
    }

    /// Inject the data globals into a scope (the shared one, or a fresh Code one).
    fn prepare_scope(&self, scope: &ExprScope<'_>) -> Result<(), NodeError> {
        let as_internal = NodeError::internal;
        scope
            .set_global(
                "$run",
                &json!({
                    "id": self.run.run_id,
                    "workflow": { "id": self.run.workflow_id, "name": self.run.workflow_name },
                    "trigger": { "kind": self.run.trigger.kind, "reason": self.run.trigger.reason },
                    "started_at": self.run.started_at,
                }),
            )
            .map_err(as_internal)?;
        scope
            .set_global("$now", &json!(crate::schedule::now_secs()))
            .map_err(as_internal)?;
        scope
            .set_global("$items", &Value::Array(self.primary_json.clone()))
            .map_err(as_internal)?;
        if self.wants_node_fn {
            let map = Value::Object(self.node_outputs_json.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
            scope.set_global("__nodes", &map).map_err(as_internal)?;
            scope
                .set_prelude(
                    "globalThis.$node = function(name) {\n\
                     var d = __nodes[name];\n\
                     if (!d) { throw new Error('no executed node named ' + name); }\n\
                     return {\n\
                       all: function(port) { return d[port || 'main'] || []; },\n\
                       first: function(port) { var a = d[port || 'main']; return (a && a.length) ? a[0] : null; }\n\
                     };\n};",
                )
                .map_err(as_internal)?;
        }
        if self.wants_secret_fn {
            let secrets: Map<String, Value> = self
                .services
                .secrets
                .all()
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            scope.set_global("__secrets", &Value::Object(secrets)).map_err(as_internal)?;
            scope
                .set_prelude(
                    "globalThis.$secret = function(name) {\n\
                     if (!(name in __secrets)) { throw new Error('no secret named ' + name + ' — add it in Settings'); }\n\
                     return __secrets[name];\n};",
                )
                .map_err(as_internal)?;
        }
        Ok(())
    }

    // --- parameters -------------------------------------------------------------

    pub fn params_raw(&self) -> &Value {
        &self.node.params
    }

    /// Params with templates resolved against item `i` of the main input.
    pub fn params(&mut self, i: usize) -> Result<Value, NodeError> {
        let item = self.primary_json.get(i).cloned().unwrap_or(Value::Null);
        self.scope
            .set_global("$json", &item)
            .and_then(|_| self.scope.set_global("$index", &json!(i)))
            .map_err(NodeError::internal)?;
        crate::expr::resolve_params(&self.scope, &self.node.params).map_err(NodeError::expression)
    }

    /// Params resolved with no item in scope ($json/$index unavailable).
    pub fn params_node(&mut self) -> Result<Value, NodeError> {
        crate::expr::resolve_params(&self.scope, &self.node.params).map_err(NodeError::expression)
    }

    // --- control ----------------------------------------------------------------

    pub fn cancelled(&self) -> bool {
        self.cancel.is_set()
    }

    pub fn cancel_flag(&self) -> &CancelFlag {
        self.cancel
    }

    /// Cooperative stop + run-deadline check. Long-running nodes call this
    /// between items and inside internal waits.
    pub fn check_cancel(&self) -> Result<(), NodeError> {
        if Instant::now() >= self.deadline {
            self.cancel.timeout();
        }
        match self.cancel.reason() {
            CancelReason::None => Ok(()),
            CancelReason::Cancelled => Err(NodeError::new(ErrorKind::Cancelled, "run was stopped")),
            CancelReason::Timeout => Err(NodeError::new(ErrorKind::Timeout, "run hit its time limit")),
        }
    }

    pub fn log(&mut self, level: LogLevel, message: &str) {
        let event = RunEvent::Log {
            workflow_id: self.run.workflow_id.clone(),
            run_id: self.run.run_id.clone(),
            node_id: self.node.id.clone(),
            level,
            message: message.to_string(),
        };
        self.store.append_event(&self.run.workflow_id, &self.run.run_id, &event);
        self.services.events.emit(&event);
    }

    /// Should per-item failures route to the virtual `error` port?
    pub fn branch_errors(&self) -> bool {
        self.node.on_error == OnError::Branch
    }

    // --- capability services ------------------------------------------------------

    pub fn http_request(&mut self, spec: &HttpRequestSpec) -> Result<HttpResponseData, NodeError> {
        permissions::http_request(&self.permissions.network, spec)
    }

    pub fn fs_read(&mut self, path: &str) -> Result<Vec<u8>, NodeError> {
        permissions::fs_read(&self.permissions.fs, path)
    }

    pub fn fs_write(&mut self, path: &str, bytes: &[u8], append: bool) -> Result<(), NodeError> {
        permissions::fs_write(&self.permissions.fs, path, bytes, append)
    }

    pub fn agent(&self) -> Result<&dyn AgentRunner, NodeError> {
        if !self.permissions.agent {
            return Err(NodeError::permission(
                "this workflow has no agent permission; enable it in Permissions",
            ));
        }
        self.services
            .agent
            .as_deref()
            .ok_or_else(|| NodeError::new(ErrorKind::Agent, "no agent runner is available here"))
    }

    /// A fresh sandboxed context for the Code node (code permission required).
    pub fn code_scope(&mut self) -> Result<ExprScope<'r>, NodeError> {
        if !self.permissions.code {
            return Err(NodeError::permission(
                "this workflow has no code permission; enable it in Permissions",
            ));
        }
        let scope = self.expr_engine.scope().map_err(NodeError::internal)?;
        self.prepare_scope(&scope)?;
        Ok(scope)
    }

    /// Regex support for the IF node's `matches` operator. Uses the Rust `regex`
    /// crate (finite-automaton, linear time) rather than QuickJS `RegExp`: a
    /// native `RegExp.test` call does not poll the interrupt watchdog, so a
    /// catastrophic-backtracking pattern could otherwise wedge the run thread
    /// unbounded. Rust regex cannot backtrack, and the size limit caps compile
    /// cost. Syntax is Rust-flavored (no lookaround/backrefs) — acceptable for a
    /// "does this match" convenience.
    pub fn regex_test(&mut self, pattern: &str, value: &str) -> Result<bool, String> {
        let re = regex::RegexBuilder::new(pattern)
            .size_limit(1 << 20)
            .dfa_size_limit(1 << 20)
            .build()
            .map_err(|e| format!("invalid regex: {e}"))?;
        Ok(re.is_match(value))
    }

    /// The seed payload — only meaningful while the fired trigger node runs.
    pub fn fire_payload(&self) -> Value {
        self.fire_payload.clone().unwrap_or(Value::Null)
    }
}

// --------------------------------------------------------------- the executor

/// Execute one workflow run to completion. Blocking; call it on a worker
/// thread. Writes the run record progressively and returns the final one.
pub fn run_workflow(
    workflow: &Workflow,
    fire: TriggerFire,
    cfg: &EngineConfig,
    cancel: &CancelFlag,
) -> RunRecord {
    let store = RunStore::new(cfg.runs_dir.clone());
    let run_id = fire
        .run_id
        .clone()
        .filter(|id| crate::model::is_safe_id(id))
        .unwrap_or_else(|| format!("r-{}", uuid::Uuid::new_v4()));
    let started_at = crate::runs::now_epoch();
    let trigger = TriggerInfo {
        kind: fire.kind.clone(),
        reason: fire.reason.clone(),
        node_id: fire.node_id.clone(),
    };
    let mut record = RunRecord {
        id: run_id.clone(),
        workflow_id: workflow.id.clone(),
        workflow_name: workflow.name.clone(),
        trigger: trigger.clone(),
        status: RunStatus::Running,
        started_at: started_at.clone(),
        finished_at: None,
        error: None,
        nodes: Vec::new(),
        data: Map::new(),
        seen: false,
    };

    let emit = |event: RunEvent| {
        store.append_event(&workflow.id, &run_id, &event);
        cfg.services.events.emit(&event);
    };

    let fail = |mut record: RunRecord, node: Option<&NodeDef>, error: NodeError, store: &RunStore| {
        record.status = RunStatus::Error;
        record.finished_at = Some(crate::runs::now_epoch());
        record.error = Some(RunErrorInfo {
            node_id: node.map(|n| n.id.clone()).unwrap_or_default(),
            node_name: node.map(|n| n.name.clone()).unwrap_or_default(),
            error,
        });
        let _ = store.write_record(&record);
        record
    };

    if !crate::model::is_safe_id(&workflow.id) {
        let err = NodeError::params("workflow has no valid id — save it first");
        let record = fail(record, None, err.clone(), &store);
        emit(RunEvent::RunFinished {
            workflow_id: workflow.id.clone(),
            run_id: run_id.clone(),
            status: RunStatus::Error,
            error: Some(err.message),
        });
        return record;
    }

    let _ = store.write_record(&record);
    emit(RunEvent::RunStarted { workflow_id: workflow.id.clone(), run_id: run_id.clone() });

    // --- validation -------------------------------------------------------------
    let validation = crate::validate::validate(workflow);
    if !validation.ok() {
        let err = NodeError::params(format!("workflow is not valid: {}", validation.error_summary()));
        let record = fail(record, None, err.clone(), &store);
        emit(RunEvent::RunFinished {
            workflow_id: workflow.id.clone(),
            run_id: run_id.clone(),
            status: RunStatus::Error,
            error: Some(err.message),
        });
        return record;
    }

    // --- fired trigger ------------------------------------------------------------
    let fire_node = match workflow.node(&fire.node_id) {
        Some(n) if nodes::get(&n.node_type).map(|x| x.spec().is_trigger).unwrap_or(false) => n,
        _ => {
            let err = NodeError::params(format!("{} is not a trigger node in this workflow", fire.node_id));
            let record = fail(record, None, err.clone(), &store);
            emit(RunEvent::RunFinished {
                workflow_id: workflow.id.clone(),
                run_id: run_id.clone(),
                status: RunStatus::Error,
                error: Some(err.message),
            });
            return record;
        }
    };
    let fire_payload = if fire.payload.is_null() {
        json!({ "trigger": { "kind": fire.kind, "reason": fire.reason, "fired_at": started_at } })
    } else {
        fire.payload.clone()
    };

    let expr_engine = match ExprEngine::new() {
        Ok(e) => e,
        Err(e) => {
            let err = NodeError::internal(e);
            let record = fail(record, None, err.clone(), &store);
            emit(RunEvent::RunFinished {
                workflow_id: workflow.id.clone(),
                run_id: run_id.clone(),
                status: RunStatus::Error,
                error: Some(err.message),
            });
            return record;
        }
    };

    // --- graph prep ---------------------------------------------------------------
    // Reachable = directed closure from the fired trigger. Edges from nodes
    // outside this set never deliver and are excluded from readiness.
    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    reachable.insert(fire_node.id.as_str());
    let mut frontier: VecDeque<&str> = VecDeque::from([fire_node.id.as_str()]);
    while let Some(id) = frontier.pop_front() {
        for c in workflow.connections.iter().filter(|c| c.from == id) {
            if reachable.insert(c.to.as_str()) {
                frontier.push_back(c.to.as_str());
            }
        }
    }

    let live_edge = |idx: usize| -> bool {
        let c = &workflow.connections[idx];
        reachable.contains(c.from.as_str()) && reachable.contains(c.to.as_str())
    };
    let mut pending: HashMap<&str, usize> = HashMap::new();
    for id in &reachable {
        pending.insert(id, 0);
    }
    for (idx, c) in workflow.connections.iter().enumerate() {
        if live_edge(idx) {
            *pending.get_mut(c.to.as_str()).unwrap() += 1;
        }
    }
    let mut edge_items: Vec<Option<Vec<Item>>> = vec![None; workflow.connections.len()];
    let mut ready: BTreeSet<&str> = pending.iter().filter(|(_, d)| **d == 0).map(|(id, _)| *id).collect();
    // Only the fired trigger seeds execution (other zero-input nodes reachable
    // from it would be triggers, which can't be reached by edges).
    ready.retain(|id| *id == fire_node.id.as_str());

    let run_info = RunInfo {
        run_id: run_id.clone(),
        workflow_id: workflow.id.clone(),
        workflow_name: workflow.name.clone(),
        trigger: trigger.clone(),
        started_at: started_at.clone(),
    };
    let deadline = Instant::now() + Duration::from_secs(workflow.settings.timeout_secs.max(10));
    let mut node_outputs_json: BTreeMap<String, Value> = BTreeMap::new();
    let mut abort: Option<(String, String, NodeError)> = None;

    while let Some(node_id) = ready.pop_first() {
        if Instant::now() >= deadline {
            cancel.timeout();
        }
        if cancel.is_set() {
            break;
        }
        let node = workflow.node(node_id).expect("reachable node exists");

        // Assemble input: per-port concatenation in connections order.
        let mut input = NodeInput::default();
        for (idx, c) in workflow.connections.iter().enumerate() {
            if c.to == node_id && live_edge(idx) {
                let items = edge_items[idx].take().unwrap_or_default();
                if let Some(slot) = input.ports.iter_mut().find(|(p, _)| *p == c.to_port) {
                    slot.1.extend(items);
                } else {
                    input.ports.push((c.to_port.clone(), items));
                }
            }
        }
        let items_in = input.total();

        emit(RunEvent::NodeStarted {
            workflow_id: workflow.id.clone(),
            run_id: run_id.clone(),
            node_id: node.id.clone(),
            name: node.name.clone(),
        });
        let t0 = Instant::now();
        let payload = if node.id == fire_node.id { Some(fire_payload.clone()) } else { None };

        let (result, attempts): (Result<NodeOutput, NodeError>, u32) = if node.disabled {
            (Ok(passthrough(node, &input)), 1)
        } else {
            execute_with_retry(
                node,
                &run_info,
                cancel,
                deadline,
                &workflow.permissions,
                &cfg.services,
                &expr_engine,
                &store,
                &input,
                payload,
                &node_outputs_json,
            )
        };
        let duration_ms = t0.elapsed().as_millis() as u64;

        // A node-level error under `branch` policy becomes one error item.
        let result = match result {
            Err(err) if node.on_error == OnError::Branch && !matches!(err.kind, ErrorKind::Cancelled | ErrorKind::Timeout) => {
                let mut out = NodeOutput::default();
                out.push(
                    "error",
                    Item::new(json!({ "error": { "kind": err.kind, "message": err.message }, "item": Value::Null })),
                );
                Ok(out)
            }
            other => other,
        };

        match result {
            Ok(output) => {
                if let Some((port, len)) = output
                    .ports
                    .iter()
                    .find(|(_, items)| items.len() > workflow.settings.max_items_per_port)
                    .map(|(p, items)| (p.clone(), items.len()))
                {
                    let err = NodeError::new(
                        ErrorKind::Data,
                        format!("{} emitted {len} items on {port} — over the {} item limit", node.name, workflow.settings.max_items_per_port),
                    );
                    record.nodes.push(NodeRunSummary {
                        node_id: node.id.clone(),
                        name: node.name.clone(),
                        status: NodeRunStatus::Error,
                        items_in,
                        items_out: 0,
                        duration_ms,
                        attempts,
                        error: Some(err.clone()),
                        warning: None,
                    });
                    emit(RunEvent::NodeFinished {
                        workflow_id: workflow.id.clone(),
                        run_id: run_id.clone(),
                        node_id: node.id.clone(),
                        name: node.name.clone(),
                        status: NodeRunStatus::Error,
                        items_in,
                        items_out: 0,
                        duration_ms,
                        error: Some(err.message.clone()),
                    });
                    abort = Some((node.id.clone(), node.name.clone(), err));
                    break;
                }

                capture_output(&mut record, &workflow.settings.capture, &node.id, &output);
                let ports_json: Map<String, Value> = output
                    .ports
                    .iter()
                    .map(|(p, items)| {
                        (p.clone(), Value::Array(items.iter().map(|i| i.json.clone()).collect()))
                    })
                    .collect();
                node_outputs_json.insert(node.name.clone(), Value::Object(ports_json));

                let items_out = output.total();
                record.nodes.push(NodeRunSummary {
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    status: NodeRunStatus::Success,
                    items_in,
                    items_out,
                    duration_ms,
                    attempts,
                    error: None,
                    warning: None,
                });
                emit(RunEvent::NodeFinished {
                    workflow_id: workflow.id.clone(),
                    run_id: run_id.clone(),
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    status: NodeRunStatus::Success,
                    items_in,
                    items_out,
                    duration_ms,
                    error: None,
                });

                // Deliver along every live out-edge; newly satisfied nodes become ready.
                for (idx, c) in workflow.connections.iter().enumerate() {
                    if c.from == node.id && live_edge(idx) {
                        edge_items[idx] = Some(output.port(&c.from_port).to_vec());
                        let d = pending.get_mut(c.to.as_str()).unwrap();
                        *d -= 1;
                        if *d == 0 {
                            ready.insert(c.to.as_str());
                        }
                    }
                }
            }
            Err(err) => {
                let stopping = node.on_error == OnError::Stop
                    || matches!(err.kind, ErrorKind::Cancelled | ErrorKind::Timeout);
                let node_status = if matches!(err.kind, ErrorKind::Cancelled) {
                    NodeRunStatus::Cancelled
                } else {
                    NodeRunStatus::Error
                };
                record.nodes.push(NodeRunSummary {
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    status: node_status,
                    items_in,
                    items_out: 0,
                    duration_ms,
                    attempts,
                    error: Some(err.clone()),
                    warning: None,
                });
                emit(RunEvent::NodeFinished {
                    workflow_id: workflow.id.clone(),
                    run_id: run_id.clone(),
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    status: node_status,
                    items_in,
                    items_out: 0,
                    duration_ms,
                    error: Some(err.to_string()),
                });
                if stopping {
                    // A cancel/timeout is not a node failure — the final status
                    // comes from the cancel flag, not from an abort record.
                    if !matches!(err.kind, ErrorKind::Cancelled | ErrorKind::Timeout) {
                        abort = Some((node.id.clone(), node.name.clone(), err));
                    }
                    break;
                }
                // on_error: skip — deliver empty everywhere and continue.
                for (idx, c) in workflow.connections.iter().enumerate() {
                    if c.from == node.id && live_edge(idx) {
                        edge_items[idx] = Some(Vec::new());
                        let d = pending.get_mut(c.to.as_str()).unwrap();
                        *d -= 1;
                        if *d == 0 {
                            ready.insert(c.to.as_str());
                        }
                    }
                }
            }
        }
        let _ = store.write_record(&record);
    }

    // Every node that didn't execute → skipped. Covers both reachable nodes cut
    // off by an abort/cancel and nodes in other triggers' subgraphs that this
    // firing never reaches (design §7.3.1).
    for node in &workflow.nodes {
        if !record.nodes.iter().any(|n| n.node_id == node.id) {
            record.nodes.push(NodeRunSummary {
                node_id: node.id.clone(),
                name: node.name.clone(),
                status: NodeRunStatus::Skipped,
                items_in: 0,
                items_out: 0,
                duration_ms: 0,
                attempts: 0,
                error: None,
                warning: None,
            });
        }
    }

    record.status = if let Some((node_id, node_name, err)) = abort {
        record.error = Some(RunErrorInfo { node_id, node_name, error: err });
        RunStatus::Error
    } else {
        match cancel.reason() {
            CancelReason::Cancelled => RunStatus::Cancelled,
            CancelReason::Timeout => RunStatus::Timeout,
            CancelReason::None => RunStatus::Success,
        }
    };
    record.finished_at = Some(crate::runs::now_epoch());
    let _ = store.write_record(&record);
    emit(RunEvent::RunFinished {
        workflow_id: workflow.id.clone(),
        run_id: run_id.clone(),
        status: record.status,
        error: record.error.as_ref().map(|e| e.error.message.clone()),
    });
    store.prune(&workflow.id, workflow.settings.keep_runs);
    record
}

/// Run a node with its retry policy. Returns the result plus the number of
/// attempts actually made (so the run record reports the real count, not the max).
#[allow(clippy::too_many_arguments)]
fn execute_with_retry(
    node: &NodeDef,
    run_info: &RunInfo,
    cancel: &CancelFlag,
    deadline: Instant,
    permissions: &Permissions,
    services: &EngineServices,
    expr_engine: &ExprEngine,
    store: &RunStore,
    input: &NodeInput,
    fire_payload: Option<Value>,
    node_outputs_json: &BTreeMap<String, Value>,
) -> (Result<NodeOutput, NodeError>, u32) {
    let node_impl = match nodes::get(&node.node_type) {
        Some(n) => n,
        None => {
            return (
                Err(NodeError::params(format!("unknown node type {}", node.node_type))),
                1,
            )
        }
    };
    let max_attempts = 1 + node.retry.as_ref().map(|r| r.attempts).unwrap_or(0);
    let backoff = node.retry.as_ref().map(|r| r.backoff_secs.clone()).unwrap_or_default();
    let mut last_err: Option<NodeError> = None;

    for attempt in 0..max_attempts {
        if attempt > 0 {
            let wait = backoff.get(attempt as usize - 1).copied().unwrap_or(5);
            let until = Instant::now() + Duration::from_secs(wait);
            while Instant::now() < until {
                // A stop or the run deadline during a backoff must surface AS a
                // cancel/timeout — not the prior attempt's error — so the run
                // finalizes with the right status instead of a spurious abort.
                if Instant::now() >= deadline {
                    cancel.timeout();
                }
                if cancel.is_set() {
                    let (kind, msg) = match cancel.reason() {
                        CancelReason::Timeout => (ErrorKind::Timeout, "run hit its time limit"),
                        _ => (ErrorKind::Cancelled, "run was stopped"),
                    };
                    return (Err(NodeError::new(kind, msg)), attempt);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        let mut ctx = match NodeCtx::new(
            node,
            run_info,
            cancel,
            deadline,
            permissions,
            services,
            expr_engine,
            store,
            input,
            fire_payload.clone(),
            node_outputs_json,
        ) {
            Ok(ctx) => ctx,
            Err(e) => return (Err(e), attempt + 1),
        };
        match node_impl.run(&mut ctx, input.clone()) {
            Ok(output) => return (Ok(output), attempt + 1),
            Err(err) => {
                if matches!(err.kind, ErrorKind::Cancelled | ErrorKind::Timeout) {
                    return (Err(err), attempt + 1);
                }
                last_err = Some(err);
            }
        }
    }
    (
        Err(last_err.unwrap_or_else(|| NodeError::internal("node produced no result"))),
        max_attempts,
    )
}

/// A disabled node forwards its first input port to its first declared output.
fn passthrough(node: &NodeDef, input: &NodeInput) -> NodeOutput {
    let out_port = nodes::get(&node.node_type)
        .and_then(|n| n.spec().outputs.first())
        .map(|p| p.name)
        .unwrap_or("main");
    let items = input
        .ports
        .first()
        .map(|(_, items)| items.clone())
        .unwrap_or_default();
    let mut out = NodeOutput::default();
    out.set_port(out_port, items);
    out
}

fn capture_output(record: &mut RunRecord, capture: &str, node_id: &str, output: &NodeOutput) {
    if capture == "none" {
        return;
    }
    let sample = capture != "full";
    let mut budget = SAMPLE_BYTES_PER_NODE;
    let mut ports = Map::new();
    for (port, items) in &output.ports {
        let mut kept: Vec<Value> = Vec::new();
        let mut truncated = false;
        for (i, item) in items.iter().enumerate() {
            if sample && (i >= SAMPLE_ITEMS_PER_PORT || budget == 0) {
                truncated = true;
                break;
            }
            if sample {
                let size = item.json.to_string().len();
                if size > budget {
                    truncated = true;
                    break;
                }
                budget -= size;
            }
            kept.push(item.json.clone());
        }
        let captured = CapturedPort { items: kept, total: items.len(), truncated };
        ports.insert(port.clone(), serde_json::to_value(captured).unwrap_or(Value::Null));
    }
    record.data.insert(node_id.to_string(), Value::Object(ports));
}
