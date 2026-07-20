//! Tauri adapter for the swerve-workflows engine: the on-disk workflow store,
//! the run manager (admission control mirroring `JobManager`), the trigger
//! scheduler, the grok-backed `AgentRunner`, the `$secret()` store, and the
//! event bridge to the webview.

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use swerve_workflows::engine::{run_workflow, CancelFlag, EngineConfig, TriggerFire};
use swerve_workflows::model::{is_safe_id, OverlapPolicy, Workflow};
use swerve_workflows::runs::{RunEvent, RunRecord, RunStatus, RunStore};
use swerve_workflows::schedule::{self, ScheduleSpec};
use swerve_workflows::services::{
    AgentRequest, AgentResult, AgentRunner, EngineServices, RunEvents, SecretStore,
};
use tauri::{AppHandle, Emitter, Manager};

const MAX_CONCURRENT_RUNS: usize = 2;

/// Serializes read-modify-write of workflow files (scheduler bookkeeping,
/// run-finish state updates, user saves). Single-process, like the other stores.
static WF_LOCK: Mutex<()> = Mutex::new(());

fn wf_guard() -> std::sync::MutexGuard<'static, ()> {
    WF_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Live grok child PIDs spawned by workflow Agent nodes. App-exit tree-kills
/// these synchronously — setting the run's cancel flag alone races the runner's
/// 400 ms poll against process teardown, orphaning `grok.exe` trees on Windows.
static ACTIVE_AGENT_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn register_agent_pid(pid: u32) {
    if let Ok(mut g) = ACTIVE_AGENT_PIDS.lock() {
        g.push(pid);
    }
}

/// Unregisters a pid on every runner return path via RAII.
struct PidGuard(u32);

impl Drop for PidGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = ACTIVE_AGENT_PIDS.lock() {
            g.retain(|p| *p != self.0);
        }
    }
}

// ------------------------------------------------------------------ store

pub struct WorkflowFiles;

impl WorkflowFiles {
    fn path(id: &str) -> PathBuf {
        crate::paths::workflows_dir().join(format!("{id}.json"))
    }

    pub fn list() -> Vec<Workflow> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(crate::paths::workflows_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                match fs::read_to_string(&path).ok().and_then(|raw| serde_json::from_str::<Workflow>(&raw).ok()) {
                    Some(w) => out.push(w),
                    None => {
                        if let Some(dest) = crate::paths::quarantine_corrupt(&path, &crate::store::Store::now()) {
                            eprintln!("workflow file failed to parse; quarantined to {}", dest.display());
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    pub fn get(id: &str) -> Option<Workflow> {
        if !is_safe_id(id) {
            return None;
        }
        let raw = fs::read_to_string(Self::path(id)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save(mut workflow: Workflow) -> Result<Workflow, String> {
        let now = crate::store::Store::now();
        if workflow.id.is_empty() {
            workflow.id = format!("w-{}", crate::store::Store::new_id());
            workflow.created_at = now.clone();
        }
        if !is_safe_id(&workflow.id) {
            return Err("invalid workflow id".into());
        }
        if workflow.created_at.is_empty() {
            workflow.created_at = now.clone();
        }
        workflow.updated_at = now;
        let raw = serde_json::to_string_pretty(&workflow).map_err(|e| e.to_string())?;
        crate::paths::write_atomic(&Self::path(&workflow.id), raw.as_bytes()).map_err(|e| e.to_string())?;
        Ok(workflow)
    }

    pub fn delete(id: &str) -> Result<(), String> {
        if !is_safe_id(id) {
            return Err("invalid workflow id".into());
        }
        let _ = fs::remove_file(Self::path(id));
        RunStore::new(crate::paths::workflow_runs_dir()).remove_all(id);
        Ok(())
    }

    /// Mutate one workflow under the store lock (bookkeeping updates).
    fn update<F: FnOnce(&mut Workflow)>(id: &str, mutate: F) {
        let _g = wf_guard();
        if let Some(mut w) = Self::get(id) {
            mutate(&mut w);
            let _ = Self::save(w);
        }
    }
}

// ------------------------------------------------------------------ services

/// `$secret()` backing: flat name→value JSON at `~/.swervebuild/secrets.json`.
/// v1 is plaintext-at-rest (same trust domain as the rest of the profile);
/// the trait seam exists so a keyring backend can replace it.
pub struct FileSecrets;

impl FileSecrets {
    fn load() -> HashMap<String, String> {
        fs::read_to_string(crate::paths::secrets_file())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn store(map: &HashMap<String, String>) -> Result<(), String> {
        let raw = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
        crate::paths::write_atomic(&crate::paths::secrets_file(), raw.as_bytes()).map_err(|e| e.to_string())
    }
}

impl SecretStore for FileSecrets {
    fn get(&self, name: &str) -> Option<String> {
        Self::load().get(name).cloned()
    }
    fn all(&self) -> HashMap<String, String> {
        Self::load()
    }
}

/// Engine events → webview events. One channel per event kind, mirroring the
/// `automation-*` family the frontend plumbing already speaks.
struct TauriRunEvents {
    app: AppHandle,
}

impl RunEvents for TauriRunEvents {
    fn emit(&self, event: &RunEvent) {
        let name = match event {
            RunEvent::RunStarted { .. } => "workflow-run-started",
            RunEvent::NodeStarted { .. } => "workflow-node-started",
            RunEvent::NodeFinished { .. } => "workflow-node-finished",
            RunEvent::Log { .. } => "workflow-run-log",
            RunEvent::RunFinished { .. } => "workflow-run-finished",
        };
        if let Ok(payload) = serde_json::to_value(event) {
            let _ = self.app.emit(name, payload);
        }
        // Failures while unfocused flash the taskbar — same rule as Automations.
        if let RunEvent::RunFinished { status, .. } = event {
            if matches!(status, RunStatus::Error | RunStatus::Timeout) {
                if let Some(win) = self.app.get_webview_window("main") {
                    if !win.is_focused().unwrap_or(true) {
                        let _ = win.request_user_attention(Some(tauri::UserAttentionType::Informational));
                    }
                }
            }
        }
    }
}

/// The workflow Agent node's runner: the same headless grok invocation
/// discipline as Automations (shadow-enforced args, hidden window, stdin null,
/// tree-kill), run synchronously with cancel polling.
pub struct GrokAgentRunner;

impl AgentRunner for GrokAgentRunner {
    fn run(&self, req: AgentRequest, cancel: &CancelFlag) -> Result<AgentResult, String> {
        let cwd = req.cwd.trim().to_string();
        if cwd.is_empty() || !std::path::Path::new(&cwd).is_dir() {
            return Err(format!("project folder not found: {cwd}"));
        }
        if let Some(model) = req.model.as_deref() {
            if model.starts_with(crate::grok_config::LOCAL_PREFIX) {
                return Err("local models are not supported in workflow agent nodes yet".into());
            }
        }
        let grok = crate::resolve_grok_executable().ok_or("Grok Build is not installed.")?;

        // Env context pack (Step 5) into headless `--rules`, same as automations.
        let env_pack = crate::env_context::format_pack(&crate::env_context::gather_for_automation(
            &cwd,
            req.model.as_deref(),
            "shadow (workflow agent node)",
            0,
            0,
        ));
        let exec = crate::jobs::Executor {
            prompt: String::new(),
            mode: crate::jobs::ExecMode::Shadow,
            tools: Vec::new(), // empty → the builder falls back to the full read-safe set
            deny: Vec::new(),
            rules: Some(format!("{env_pack}\n\n{}", crate::jobs::STANDING_RULES)),
            effort: req.effort.clone(),
            model: req.model.clone(),
            max_turns: req.max_turns,
            cwd: cwd.clone(),
            web_search: req.web_search,
            json_schema: req.json_schema.clone(),
            timeout_secs: req.timeout_secs,
            report_dir: None,
        };

        // Prompt via file — sidesteps Windows quoting entirely.
        let prompt_dir = crate::paths::workflow_runs_dir().join(".prompts");
        let _ = fs::create_dir_all(&prompt_dir);
        let prompt_path = prompt_dir.join(format!("{}.prompt.txt", crate::store::Store::new_id()));
        fs::write(&prompt_path, &req.prompt).map_err(|e| e.to_string())?;
        let args = crate::jobs::build_grok_args(&exec, &prompt_path.to_string_lossy());

        let mut child = crate::util::hidden_command(&grok)
            .args(&args)
            .env("GROK_DISABLE_AUTOUPDATER", "1")
            .envs(crate::providers::grok_endpoint_env())
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                let _ = fs::remove_file(&prompt_path);
                format!("failed to launch grok: {e}")
            })?;
        let pid = child.id();
        // Track the pid so app-exit can tree-kill it; the guard unregisters on
        // every return path.
        register_agent_pid(pid);
        let _pid_guard = PidGuard(pid);

        // Reader threads parse streaming-json (stdout) and drain stderr — a
        // chatty grok stderr would otherwise fill its pipe and block the child.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        thread::spawn(move || {
            if let Some(stderr) = stderr {
                for _line in BufReader::new(stderr).lines().map_while(Result::ok) {}
            }
        });
        let (tx, rx) = mpsc::channel::<AgentResult>();
        thread::spawn(move || {
            let mut result = AgentResult::default();
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    match v.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            result.text.push_str(v.get("text").and_then(|t| t.as_str()).unwrap_or_default());
                        }
                        Some("end") => {
                            result.stop_reason = v.get("stopReason").and_then(|s| s.as_str()).map(String::from);
                            result.session_id = v.get("sessionId").and_then(|s| s.as_str()).map(String::from);
                            result.structured = v.get("structuredOutput").cloned();
                        }
                        _ => {}
                    }
                }
            }
            let _ = tx.send(result);
        });

        let deadline = Instant::now() + Duration::from_secs(req.timeout_secs.max(10));
        let outcome = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    if cancel.is_set() {
                        crate::jobs::tree_kill(pid);
                        let _ = child.wait();
                        break Err("agent run was stopped".to_string());
                    }
                    if Instant::now() >= deadline {
                        crate::jobs::tree_kill(pid);
                        let _ = child.wait();
                        break Err(format!("agent run hit its {}s time limit", req.timeout_secs));
                    }
                    thread::sleep(Duration::from_millis(400));
                }
                Err(e) => {
                    crate::jobs::tree_kill(pid);
                    break Err(format!("waiting on grok: {e}"));
                }
            }
        };
        let _ = fs::remove_file(&prompt_path);
        let status = outcome?;
        let result = rx.recv_timeout(Duration::from_secs(6)).unwrap_or_default();
        if !status.success() {
            let tail = result.text.trim();
            return Err(if tail.is_empty() {
                format!("grok exited with {status}")
            } else {
                format!("grok exited with {status}: {tail}")
            });
        }
        Ok(AgentResult { text: result.text.trim().to_string(), ..result })
    }
}

// ------------------------------------------------------------------ manager

struct RunHandle {
    workflow_id: String,
    cancel: Arc<CancelFlag>,
}

struct QueuedFire {
    run_id: String,
    workflow: Workflow,
    fire: TriggerFire,
}

/// Admission control for workflow runs: pause, overlap, capacity + queue —
/// the `JobManager` pattern applied to the engine.
pub struct WorkflowManager {
    running: Mutex<HashMap<String, RunHandle>>,
    queue: Mutex<VecDeque<QueuedFire>>,
    paused: AtomicBool,
    pub wake_tx: Mutex<Option<mpsc::Sender<()>>>,
}

impl Default for WorkflowManager {
    fn default() -> Self {
        WorkflowManager {
            running: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            paused: AtomicBool::new(load_paused()),
            wake_tx: Mutex::new(None),
        }
    }
}

fn paused_file() -> PathBuf {
    crate::paths::workflows_dir().join(".paused")
}

fn load_paused() -> bool {
    paused_file().is_file()
}

impl WorkflowManager {
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        if paused {
            let _ = fs::create_dir_all(crate::paths::workflows_dir());
            let _ = fs::write(paused_file(), b"1");
        } else {
            let _ = fs::remove_file(paused_file());
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    fn services(app: &AppHandle) -> EngineServices {
        EngineServices {
            agent: Some(Arc::new(GrokAgentRunner)),
            secrets: Arc::new(FileSecrets),
            events: Arc::new(TauriRunEvents { app: app.clone() }),
        }
    }

    /// The single seam every trigger funnels through. The overlap + capacity
    /// decision and the slot reservation happen under one `running` lock so two
    /// concurrent triggers (the scheduler and a manual Run) can't both slip past
    /// the checks and double-spawn or exceed the cap.
    pub fn start_run(
        self: &Arc<Self>,
        app: AppHandle,
        workflow: Workflow,
        mut fire: TriggerFire,
    ) -> Result<String, String> {
        if self.is_paused() && fire.reason != "manual" {
            return Err("Workflows are paused".into());
        }
        let run_id = format!("r-{}", crate::store::Store::new_id());
        fire.run_id = Some(run_id.clone());

        enum Admit {
            Existing(String),
            Queue,
            Go(Arc<CancelFlag>),
        }
        let decision = {
            let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
            let existing = running
                .iter()
                .find(|(_, h)| h.workflow_id == workflow.id)
                .map(|(id, _)| id.clone());
            let reserve = |running: &mut HashMap<String, RunHandle>| {
                let cancel = Arc::new(CancelFlag::new());
                running.insert(
                    run_id.clone(),
                    RunHandle { workflow_id: workflow.id.clone(), cancel: Arc::clone(&cancel) },
                );
                cancel
            };
            match (existing, workflow.settings.overlap) {
                (Some(id), OverlapPolicy::Skip) => Admit::Existing(id),
                (Some(id), OverlapPolicy::Replace) => {
                    if let Some(h) = running.get(&id) {
                        h.cancel.cancel();
                    }
                    // The cancelled run still holds its slot until it exits, so a
                    // replacement at capacity queues and drains when it frees.
                    if running.len() >= MAX_CONCURRENT_RUNS {
                        Admit::Queue
                    } else {
                        Admit::Go(reserve(&mut running))
                    }
                }
                (None, _) if running.len() >= MAX_CONCURRENT_RUNS => Admit::Queue,
                (None, _) => Admit::Go(reserve(&mut running)),
            }
        };

        match decision {
            Admit::Existing(id) => Ok(id),
            Admit::Queue => {
                let store = RunStore::new(crate::paths::workflow_runs_dir());
                let _ = store.write_record(&RunRecord {
                    id: run_id.clone(),
                    workflow_id: workflow.id.clone(),
                    workflow_name: workflow.name.clone(),
                    trigger: swerve_workflows::runs::TriggerInfo {
                        kind: fire.kind.clone(),
                        reason: fire.reason.clone(),
                        node_id: fire.node_id.clone(),
                    },
                    status: RunStatus::Queued,
                    started_at: crate::store::Store::now(),
                    finished_at: None,
                    error: None,
                    nodes: Vec::new(),
                    data: serde_json::Map::new(),
                    seen: false,
                });
                if let Ok(mut q) = self.queue.lock() {
                    q.push_back(QueuedFire { run_id: run_id.clone(), workflow, fire });
                }
                self.drain_queue(&app);
                Ok(run_id)
            }
            Admit::Go(cancel) => {
                self.spawn_reserved(app, workflow, fire, run_id.clone(), cancel);
                Ok(run_id)
            }
        }
    }

    /// Spawn a run whose slot is already reserved in `running` under key `run_id`.
    fn spawn_reserved(
        self: &Arc<Self>,
        app: AppHandle,
        workflow: Workflow,
        fire: TriggerFire,
        run_id: String,
        cancel: Arc<CancelFlag>,
    ) {
        let manager = Arc::clone(self);
        thread::spawn(move || {
            let cfg = EngineConfig {
                runs_dir: crate::paths::workflow_runs_dir(),
                services: Self::services(&app),
            };
            let record = run_workflow(&workflow, fire, &cfg, &cancel);
            WorkflowFiles::update(&workflow.id, |w| {
                w.state.last_run_id = Some(record.id.clone());
                w.state.last_status = Some(status_str(record.status).to_string());
            });
            // Remove by the reserved key, which always frees the slot even if a
            // future change made the engine's run id diverge from the reservation.
            if let Ok(mut g) = manager.running.lock() {
                g.remove(&run_id);
            }
            manager.drain_queue(&app);
        });
    }

    fn drain_queue(self: &Arc<Self>, app: &AppHandle) {
        loop {
            // Reserve the slot and pop atomically so two concurrent drains (from
            // two runs finishing at once) can't both spawn past the cap.
            let next = {
                let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
                if running.len() >= MAX_CONCURRENT_RUNS {
                    return;
                }
                let popped = self
                    .queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop_front();
                match popped {
                    Some(q) => {
                        let cancel = Arc::new(CancelFlag::new());
                        running.insert(
                            q.run_id.clone(),
                            RunHandle { workflow_id: q.workflow.id.clone(), cancel: Arc::clone(&cancel) },
                        );
                        Some((q, cancel))
                    }
                    None => None,
                }
            };
            match next {
                Some((q, cancel)) => {
                    self.spawn_reserved(app.clone(), q.workflow, q.fire, q.run_id, cancel)
                }
                None => return,
            }
        }
    }

    pub fn cancel_run(&self, run_id: &str) -> Result<(), String> {
        if let Ok(g) = self.running.lock() {
            if let Some(handle) = g.get(run_id) {
                handle.cancel.cancel();
                return Ok(());
            }
        }
        // Not running — maybe queued.
        let removed = self.queue.lock().ok().and_then(|mut q| {
            let pos = q.iter().position(|x| x.run_id == run_id)?;
            q.remove(pos)
        });
        if let Some(q) = removed {
            let store = RunStore::new(crate::paths::workflow_runs_dir());
            if let Some(mut rec) = store.read_record(&q.workflow.id, run_id) {
                rec.status = RunStatus::Cancelled;
                rec.finished_at = Some(crate::store::Store::now());
                let _ = store.write_record(&rec);
            }
        }
        Ok(())
    }

    /// Flag every running workflow AND tree-kill any in-flight Agent grok trees —
    /// called on app exit. The run threads are detached and would not observe the
    /// cancel flag before the process tears down, so the kill must be synchronous.
    pub fn cancel_all(&self) {
        if let Ok(g) = self.running.lock() {
            for handle in g.values() {
                handle.cancel.cancel();
            }
        }
        if let Ok(pids) = ACTIVE_AGENT_PIDS.lock() {
            for pid in pids.iter() {
                crate::jobs::tree_kill(*pid);
            }
        }
    }

    pub fn wake(&self) {
        if let Ok(w) = self.wake_tx.lock() {
            if let Some(tx) = w.as_ref() {
                let _ = tx.send(());
            }
        }
    }
}

fn status_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Success => "success",
        RunStatus::Error => "error",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Timeout => "timeout",
    }
}

// ------------------------------------------------------------------ scheduler

fn trigger_state_u64(w: &Workflow, node_id: &str, key: &str) -> Option<u64> {
    w.state.trigger.get(node_id)?.get(key)?.as_u64()
}

fn trigger_state_str<'a>(w: &'a Workflow, node_id: &str, key: &str) -> Option<&'a str> {
    w.state.trigger.get(node_id)?.get(key)?.as_str()
}

fn set_trigger_state(w: &mut Workflow, node_id: &str, key: &str, value: Value) {
    let entry = w
        .state
        .trigger
        .entry(node_id.to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(map) = entry.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

struct Fire {
    workflow_id: String,
    node_id: String,
    kind: String,
    reason: String,
    payload: Value,
    /// (key, value) bookkeeping to persist on the trigger node's state.
    bookkeeping: Vec<(String, Value)>,
}

/// The workflow trigger scheduler — its own thread, fully separate from the
/// Automations scheduler. 30s tick, early wake on config change, wall-clock
/// math that survives sleep/hibernate.
pub fn spawn_scheduler(app: AppHandle, manager: Arc<WorkflowManager>) {
    let (tx, rx) = mpsc::channel::<()>();
    if let Ok(mut w) = manager.wake_tx.lock() {
        *w = Some(tx);
    }
    thread::spawn(move || {
        // One-tick settle buffer for file triggers, keyed workflow_id/node_id.
        let mut file_pending: HashMap<String, String> = HashMap::new();
        loop {
            let now = schedule::now_secs();
            let mut next_wait = 30u64;
            let mut fires: Vec<Fire> = Vec::new();

            if !manager.is_paused() {
                for w in WorkflowFiles::list() {
                    if !w.enabled {
                        continue;
                    }
                    for node in &w.nodes {
                        if node.disabled {
                            continue;
                        }
                        let params = &node.params;
                        match node.node_type.as_str() {
                            "trigger.schedule" => {
                                let Ok(spec) = serde_json::from_value::<ScheduleSpec>(params.clone()) else {
                                    continue;
                                };
                                let last = trigger_state_u64(&w, &node.id, "last_fired_at");
                                if spec.every == "interval" {
                                    let iv = spec.interval_minutes.max(15) * 60;
                                    match last {
                                        Some(t) if now >= t + iv => fires.push(schedule_fire(&w, &node.id, "schedule", now)),
                                        Some(t) => next_wait = next_wait.min((t + iv).saturating_sub(now)),
                                        None => fires.push(schedule_fire(&w, &node.id, "schedule", now)),
                                    }
                                } else if let Some(occ) = schedule::most_recent_occurrence(&spec, now) {
                                    // Never-fired rules baseline off workflow creation so a
                                    // new daily rule doesn't "catch up" on a pre-creation slot.
                                    let baseline = last.unwrap_or_else(|| w.created_at.parse::<u64>().unwrap_or(0));
                                    if baseline < occ {
                                        fires.push(schedule_fire(&w, &node.id, "schedule-catchup", now));
                                    }
                                }
                            }
                            "trigger.git" => {
                                let since = trigger_state_u64(&w, &node.id, "last_fired_at")
                                    .map(|t| now.saturating_sub(t))
                                    .unwrap_or(u64::MAX);
                                if since < w.settings.min_interval_secs.max(60) {
                                    continue;
                                }
                                let cwd = params.get("cwd").and_then(|v| v.as_str()).unwrap_or_default();
                                let branch = params.get("branch").and_then(|v| v.as_str()).filter(|b| !b.is_empty());
                                let Some(head) = schedule::git_head(cwd, branch) else { continue };
                                match trigger_state_str(&w, &node.id, "last_seen_commit") {
                                    Some(prev) if prev != head => {
                                        let prev = prev.to_string();
                                        fires.push(Fire {
                                            workflow_id: w.id.clone(),
                                            node_id: node.id.clone(),
                                            kind: "git".into(),
                                            reason: "git".into(),
                                            payload: json!({ "trigger": {
                                                "kind": "git", "fired_at": now,
                                                "commit": head, "prev": prev, "branch": branch,
                                            }}),
                                            bookkeeping: vec![
                                                ("last_seen_commit".into(), json!(head)),
                                                ("last_fired_at".into(), json!(now)),
                                            ],
                                        });
                                    }
                                    Some(_) => {}
                                    None => {
                                        // First observation: baseline silently.
                                        WorkflowFiles::update(&w.id, |wf| {
                                            set_trigger_state(wf, &node.id, "last_seen_commit", json!(head));
                                        });
                                    }
                                }
                            }
                            "trigger.file" => {
                                let path = params.get("path").and_then(|v| v.as_str()).unwrap_or_default();
                                if path.is_empty() {
                                    continue;
                                }
                                let glob = params.get("glob").and_then(|v| v.as_str()).filter(|g| !g.is_empty());
                                let snap = schedule::file_snapshot(path, glob);
                                let key = format!("{}/{}", w.id, node.id);
                                if trigger_state_str(&w, &node.id, "snapshot") != Some(snap.as_str()) {
                                    if file_pending.get(&key).map(|s| s.as_str()) == Some(snap.as_str()) {
                                        file_pending.remove(&key);
                                        fires.push(Fire {
                                            workflow_id: w.id.clone(),
                                            node_id: node.id.clone(),
                                            kind: "file".into(),
                                            reason: "file".into(),
                                            payload: json!({ "trigger": {
                                                "kind": "file", "fired_at": now, "path": path, "glob": glob,
                                            }}),
                                            bookkeeping: vec![
                                                ("snapshot".into(), json!(snap)),
                                                ("last_fired_at".into(), json!(now)),
                                            ],
                                        });
                                    } else {
                                        file_pending.insert(key, snap);
                                        next_wait = next_wait.min(3);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            for fire in fires {
                {
                    let node_id = fire.node_id.clone();
                    let bookkeeping = fire.bookkeeping.clone();
                    WorkflowFiles::update(&fire.workflow_id, |wf| {
                        wf.state.last_fired_at = Some(now);
                        for (key, value) in &bookkeeping {
                            set_trigger_state(wf, &node_id, key, value.clone());
                        }
                    });
                }
                if let Some(workflow) = WorkflowFiles::get(&fire.workflow_id) {
                    let _ = manager.start_run(
                        app.clone(),
                        workflow,
                        TriggerFire {
                            node_id: fire.node_id,
                            kind: fire.kind,
                            reason: fire.reason,
                            payload: fire.payload,
                            run_id: None,
                        },
                    );
                }
            }

            let _ = rx.recv_timeout(Duration::from_secs(next_wait.clamp(2, 30)));
        }
    });
}

fn schedule_fire(w: &Workflow, node_id: &str, reason: &str, now: u64) -> Fire {
    Fire {
        workflow_id: w.id.clone(),
        node_id: node_id.to_string(),
        kind: "schedule".into(),
        reason: reason.into(),
        payload: json!({ "trigger": { "kind": "schedule", "reason": reason, "fired_at": now } }),
        bookkeeping: vec![("last_fired_at".into(), json!(now))],
    }
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn workflows_list() -> Vec<Workflow> {
    WorkflowFiles::list()
}

#[tauri::command]
pub fn workflow_get(id: String) -> Option<Workflow> {
    WorkflowFiles::get(&id)
}

#[tauri::command]
pub fn workflow_save(
    mut workflow: Workflow,
    manager: tauri::State<'_, Arc<WorkflowManager>>,
) -> Result<Workflow, String> {
    let _g = wf_guard();
    // `state` — last_fired_at plus the per-trigger bookkeeping (git last-seen
    // commit, file snapshot) — is scheduler-owned. The editor loads it once at
    // page open and it goes stale as the scheduler fires; writing the editor's
    // copy back would reset an interval schedule (making it re-fire) and drop a
    // git/file trigger's baseline. Always keep the on-disk state on save.
    if !workflow.id.is_empty() {
        if let Some(existing) = WorkflowFiles::get(&workflow.id) {
            workflow.state = existing.state;
            if workflow.created_at.is_empty() {
                workflow.created_at = existing.created_at;
            }
        }
    }
    let saved = WorkflowFiles::save(workflow)?;
    manager.wake();
    Ok(saved)
}

#[tauri::command]
pub fn workflow_delete(id: String) -> Result<(), String> {
    let _g = wf_guard();
    WorkflowFiles::delete(&id)
}

#[tauri::command]
pub fn workflow_validate(workflow: Workflow) -> Value {
    serde_json::to_value(swerve_workflows::validate::validate(&workflow)).unwrap_or(Value::Null)
}

#[tauri::command]
pub fn workflow_node_catalog() -> Value {
    swerve_workflows::nodes::catalog_json()
}

#[tauri::command]
pub fn workflow_run_now(
    app: AppHandle,
    manager: tauri::State<'_, Arc<WorkflowManager>>,
    id: String,
    trigger_node_id: Option<String>,
) -> Result<String, String> {
    let workflow = WorkflowFiles::get(&id).ok_or("workflow not found")?;
    let node_id = match trigger_node_id {
        Some(id) => id,
        None => {
            // Prefer a manual trigger; fall back to any trigger node.
            let manual = workflow.nodes.iter().find(|n| n.node_type == "trigger.manual" && !n.disabled);
            let any = workflow.nodes.iter().find(|n| {
                swerve_workflows::nodes::get(&n.node_type)
                    .map(|x| x.spec().is_trigger)
                    .unwrap_or(false)
                    && !n.disabled
            });
            manual.or(any).map(|n| n.id.clone()).ok_or("this workflow has no trigger node")?
        }
    };
    manager.start_run(app, workflow, TriggerFire::manual(node_id))
}

#[tauri::command]
pub fn workflow_cancel_run(
    manager: tauri::State<'_, Arc<WorkflowManager>>,
    run_id: String,
) -> Result<(), String> {
    manager.cancel_run(&run_id)
}

/// Run list for the sidebar — records without the (potentially large) data map.
#[tauri::command]
pub fn workflow_runs(workflow_id: String) -> Vec<RunRecord> {
    let store = RunStore::new(crate::paths::workflow_runs_dir());
    store
        .list_records(&workflow_id)
        .into_iter()
        .map(|mut rec| {
            rec.data = serde_json::Map::new();
            rec
        })
        .collect()
}

/// Full record including captured per-node data (for the run inspector).
#[tauri::command]
pub fn workflow_run_detail(workflow_id: String, run_id: String) -> Option<RunRecord> {
    RunStore::new(crate::paths::workflow_runs_dir()).read_record(&workflow_id, &run_id)
}

#[tauri::command]
pub fn workflows_set_paused(manager: tauri::State<'_, Arc<WorkflowManager>>, paused: bool) {
    manager.set_paused(paused);
    manager.wake();
}

#[tauri::command]
pub fn workflows_get_paused(manager: tauri::State<'_, Arc<WorkflowManager>>) -> bool {
    manager.is_paused()
}

#[tauri::command]
pub fn workflow_secret_names() -> Vec<String> {
    let mut names: Vec<String> = FileSecrets::load().into_keys().collect();
    names.sort();
    names
}

#[tauri::command]
pub fn workflow_secret_set(name: String, value: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("secret name is required".into());
    }
    let mut map = FileSecrets::load();
    map.insert(name, value);
    FileSecrets::store(&map)
}

#[tauri::command]
pub fn workflow_secret_delete(name: String) -> Result<(), String> {
    let mut map = FileSecrets::load();
    map.remove(&name);
    FileSecrets::store(&map)
}
