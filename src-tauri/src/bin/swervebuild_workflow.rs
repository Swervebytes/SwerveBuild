//! Headless workflow runner — the engine-first gate from the design doc:
//! execute a workflow JSON file with zero UI.
//!
//!   swervebuild-workflow run <file.json> [--trigger <node_id>]
//!   swervebuild-workflow validate <file.json>
//!
//! Runs use the app's real data dir for run records and secrets, and the real
//! grok agent runner, so a CLI run is a faithful rehearsal of an in-app run.
//! Exception: `local:` models need the app's managed llama-server and are
//! refused here with a clear error.

use std::sync::Arc;
use swerve_workflows::engine::{run_workflow, CancelFlag, EngineConfig, TriggerFire};
use swerve_workflows::model::Workflow;
use swerve_workflows::runs::RunEvent;
use swerve_workflows::services::{EngineServices, RunEvents};

struct PrintEvents;

impl RunEvents for PrintEvents {
    fn emit(&self, event: &RunEvent) {
        match event {
            RunEvent::RunStarted { run_id, .. } => println!("run {run_id} started"),
            RunEvent::NodeStarted { name, .. } => println!("  > {name}"),
            RunEvent::NodeFinished { name, status, items_in, items_out, duration_ms, error, .. } => {
                match error {
                    Some(e) => println!("  x {name} ({items_in} in) failed after {duration_ms}ms: {e}"),
                    None => println!("  = {name}: {items_in} in, {items_out} out, {duration_ms}ms ({status:?})"),
                }
            }
            RunEvent::Log { message, .. } => println!("    log: {message}"),
            RunEvent::RunFinished { status, error, .. } => match error {
                Some(e) => println!("run finished: {status:?} — {e}"),
                None => println!("run finished: {status:?}"),
            },
        }
    }
}

fn load(path: &str) -> Result<Workflow, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("{path} does not parse as a workflow: {e}"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
        Some("validate") => cmd_validate(&args[1..]),
        _ => {
            eprintln!("usage: swervebuild-workflow run <file.json> [--trigger <node_id>]");
            eprintln!("       swervebuild-workflow validate <file.json>");
            2
        }
    };
    std::process::exit(code);
}

fn cmd_validate(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("validate needs a file path");
        return 2;
    };
    let workflow = match load(path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let v = swerve_workflows::validate::validate(&workflow);
    for warning in &v.warnings {
        println!("warning: {}", warning.message);
    }
    for error in &v.errors {
        println!("error: {}", error.message);
    }
    if v.ok() {
        println!("ok: {} nodes, {} connections", workflow.nodes.len(), workflow.connections.len());
        0
    } else {
        1
    }
}

fn cmd_run(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("run needs a file path");
        return 2;
    };
    let workflow = match load(path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let trigger_node_id = args
        .iter()
        .position(|a| a == "--trigger")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| {
            workflow
                .nodes
                .iter()
                .find(|n| {
                    swerve_workflows::nodes::get(&n.node_type)
                        .map(|x| x.spec().is_trigger)
                        .unwrap_or(false)
                })
                .map(|n| n.id.clone())
        });
    let Some(node_id) = trigger_node_id else {
        eprintln!("the workflow has no trigger node");
        return 1;
    };

    let cfg = EngineConfig {
        runs_dir: swerve_build_lib::paths::workflow_runs_dir(),
        services: EngineServices {
            agent: Some(Arc::new(swerve_build_lib::workflows_tauri::GrokAgentRunner::headless())),
            secrets: Arc::new(swerve_build_lib::workflows_tauri::FileSecrets),
            events: Arc::new(PrintEvents),
        },
    };
    let record = run_workflow(&workflow, TriggerFire::manual(node_id), &cfg, &CancelFlag::new());
    println!(
        "record: {}",
        swerve_build_lib::paths::workflow_runs_dir()
            .join(&record.workflow_id)
            .join(format!("{}.json", record.id))
            .display()
    );
    match record.status {
        swerve_workflows::runs::RunStatus::Success => 0,
        _ => 1,
    }
}
