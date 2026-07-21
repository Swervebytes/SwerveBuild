use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

#[derive(Deserialize)]
struct RpcRequest {
    id: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
}

#[derive(Serialize)]
struct ToolDef {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

// Tool names are intentionally UN-prefixed. grok namespaces every MCP tool as
// `<server>__<tool>`, and this server is registered as "swervebuild", so a name
// like "swervebuild_list_projects" surfaced to the model as the double-prefixed
// "swervebuild__swervebuild_list_projects" — which the model fumbles (it reaches
// for "swervebuild__list_projects" first and gets "tool not found"). Short names
// give the clean "swervebuild__list_projects". call_tool still accepts the old
// prefixed names (and the legacy swervegrok_ ones) as aliases.
fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_projects".into(),
            description: "List all Swerve Build projects (folders) with paths and chat counts.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "list_chats".into(),
            description: "List chats for a project. Optional project_id; omit for all projects.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "description": "Swerve Build project id" }
                }
            }),
        },
        ToolDef {
            name: "get_app_status".into(),
            description: "Get Swerve Build app status: projects, chats, data file path.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "get_chat_summary".into(),
            description: "Get summary for a chat: title, message count, session id, last updated.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "chat_id": { "type": "string" }
                },
                "required": ["chat_id"]
            }),
        },
        ToolDef {
            name: "list_automations".into(),
            description: "List Swerve Build automations (triggered agents): id, name, whether enabled, trigger, execution mode, and last run status.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "list_automation_runs".into(),
            description: "List recent runs for an automation: run id, status, what triggered it, timestamps, and final output.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "automation_id": { "type": "string", "description": "Swerve Build automation id" },
                    "limit": { "type": "number", "description": "Max runs to return (default 10)" }
                },
                "required": ["automation_id"]
            }),
        },
        // --- App UI drive (Step 6). Require Settings grant. CDP interaction TBD. ---
        ToolDef {
            name: "app_ui_state".into(),
            description: "Read SwerveBuild UI control status: human grant, current route/title (published by the app), and which drive tools are implemented. Requires Settings → Agent UI control.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "app_ui_snapshot".into(),
            description: "Size-capped text digest of the visible SwerveBuild UI from the last published frontend state (route/title). Not a full DOM dump yet. Requires grant.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "app_ui_screenshot".into(),
            description: "Capture the main SwerveBuild WebView as PNG via CDP. Returns artifact id + path under ~/.swervebuild/app_ui_artifacts/. Requires Settings → Agent UI control grant and a running S08+ app with CDP.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "app_ui_click".into(),
            description: "Click a control in the SwerveBuild UI via CDP. Pass a CSS selector or bare data-testid (e.g. app-ui-grant-on). Requires grant + running app with CDP.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector or bare data-testid token" }
                },
                "required": ["selector"]
            }),
        },
        ToolDef {
            name: "app_ui_type".into(),
            description: "Fill a SwerveBuild UI field via CDP: input, textarea, or contenteditable. Pass a CSS selector or bare data-testid. Fires real input/change events so bindings update. Requires grant + running app with CDP.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector or bare data-testid token" },
                    "text": { "type": "string", "description": "Text to set (max 4000 chars)" },
                    "clear": { "type": "boolean", "description": "Replace existing value (default true); false appends" }
                },
                "required": ["selector", "text"]
            }),
        },
        ToolDef {
            name: "app_ui_press".into(),
            description: "Send a trusted key press to the focused SwerveBuild element via CDP (focus a field first with app_ui_click/app_ui_type). Named keys: Enter, Tab, Escape, Backspace, Delete, ArrowUp/Down/Left/Right, Home, End, PageUp, PageDown, Space — or one printable character. Chords: Ctrl+Enter, Shift+Tab, … Requires grant + running app with CDP.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "e.g. Enter, Escape, Ctrl+Enter, a" }
                },
                "required": ["key"]
            }),
        },
        ToolDef {
            name: "app_ui_wait".into(),
            description: "Wait for a SwerveBuild UI condition via CDP polling. Condition forms: CSS selector or bare data-testid (present), !selector (absent), route:/path (SPA route), text:needle (visible text). Timeout capped at 15000 ms (default 5000). Timeout returns matched:false — not an error. Requires grant + running app with CDP.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "condition": { "type": "string", "description": "selector | !selector | route:/path | text:needle" },
                    "timeout_ms": { "type": "number", "description": "Max wait in ms (default 5000, cap 15000)" }
                },
                "required": ["condition"]
            }),
        },
        // --- Terminal (Step 6). One-shot command runner. Require Settings grant. ---
        ToolDef {
            name: "term_state".into(),
            description: "Report the terminal tool status: human grant, the resolved open-project folder commands run in, and defaults (shell, timeout, output cap). Read-only; no grant required.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "term_run".into(),
            description: "Run ONE shell command to completion inside the open SwerveBuild project and return captured output. Windows PowerShell by default (shell:\"cmd\" for cmd.exe). cwd is confined to the project folder (junction-safe); output is size-capped (truncated flagged); a timeout tree-kills the process. Non-zero exit and timeout are normal results (ok:false), not tool errors. Requires Settings → Agent terminal grant (off by default); never enabled for shadow automations.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command line to run (max 8000 chars)" },
                    "cwd": { "type": "string", "description": "Optional sub-folder of the project to run in; must stay inside the project" },
                    "project_id": { "type": "string", "description": "Optional Swerve project id; defaults to the most-recently-opened project" },
                    "timeout_secs": { "type": "number", "description": "Max seconds before the process is killed (default 30, cap 120)" },
                    "shell": { "type": "string", "description": "powershell (default) or cmd" }
                },
                "required": ["command"]
            }),
        },
    ]
}

fn data_path() -> PathBuf {
    swerve_build_lib::paths::data_file()
}

/// Reject ids that could escape the runs directory when joined into a path. This
/// server is reachable by the chat agent (it's wired in as an MCP server), so an
/// `automation_id` like `..\..\..\Users\me\.grok` must never reach `run_dir`.
/// Real ids are UUIDs or `a-<uuid>` — ASCII alnum, `-`, `_`.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn load_json(path: PathBuf) -> Value {
    if !path.exists() {
        return json!({});
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}))
}

fn load_store() -> Value {
    let path = data_path();
    if !path.exists() {
        return json!({ "projects": [], "chats": [] });
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({ "projects": [], "chats": [] }))
}

fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    let store = load_store();
    match name {
        "list_projects" | "swervebuild_list_projects" | "swervegrok_list_projects" => {
            let projects = store.get("projects").cloned().unwrap_or_else(|| json!([]));
            let chats = store.get("chats").cloned().unwrap_or_else(|| json!([]));
            let mut out = Vec::new();
            if let Some(arr) = projects.as_array() {
                for project in arr {
                    let pid = project.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let chat_count = chats
                        .as_array()
                        .map(|list| {
                            list.iter()
                                .filter(|c| c.get("project_id").and_then(|v| v.as_str()) == Some(pid))
                                .count()
                        })
                        .unwrap_or(0);
                    out.push(json!({
                        "id": project.get("id"),
                        "name": project.get("name"),
                        "path": project.get("path"),
                        "chat_count": chat_count,
                        "last_opened_at": project.get("last_opened_at"),
                    }));
                }
            }
            Ok(json!({ "projects": out }))
        }
        "list_chats" | "swervebuild_list_chats" | "swervegrok_list_chats" => {
            let project_id = args.get("project_id").and_then(|v| v.as_str());
            let chats = store.get("chats").cloned().unwrap_or_else(|| json!([]));
            let filtered: Vec<Value> = chats
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter(|chat| {
                    project_id.map_or(true, |pid| {
                        chat.get("project_id").and_then(|v| v.as_str()) == Some(pid)
                    })
                })
                .map(|chat| {
                    json!({
                        "id": chat.get("id"),
                        "project_id": chat.get("project_id"),
                        "title": chat.get("title"),
                        "message_count": chat.get("messages").and_then(|m| m.as_array()).map(|a| a.len()).unwrap_or(0),
                        "grok_session_id": chat.get("grok_session_id"),
                        "updated_at": chat.get("updated_at"),
                    })
                })
                .collect();
            Ok(json!({ "chats": filtered }))
        }
        "get_app_status" | "swervebuild_get_app_status" | "swervegrok_get_app_status" => {
            let autos = load_json(swerve_build_lib::paths::automations_file());
            Ok(json!({
                "data_path": data_path().display().to_string(),
                "project_count": store.get("projects").and_then(|p| p.as_array()).map(|a| a.len()).unwrap_or(0),
                "chat_count": store.get("chats").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0),
                "automation_count": autos.get("automations").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0),
                "automations_paused": autos.get("paused").and_then(|v| v.as_bool()).unwrap_or(false),
            }))
        }
        "list_automations" | "swervebuild_list_automations" | "swervegrok_list_automations" => {
            let autos = load_json(swerve_build_lib::paths::automations_file());
            let paused = autos.get("paused").and_then(|v| v.as_bool()).unwrap_or(false);
            let list = autos.get("automations").and_then(|a| a.as_array()).cloned().unwrap_or_default();
            let out: Vec<Value> = list
                .iter()
                .map(|a| {
                    json!({
                        "id": a.get("id"),
                        "name": a.get("name"),
                        "enabled": a.get("enabled"),
                        "project_id": a.get("project_id"),
                        "trigger": a.get("trigger"),
                        "mode": a.get("executor").and_then(|e| e.get("mode")),
                        "last_status": a.get("state").and_then(|s| s.get("last_status")),
                        "last_fired_at": a.get("state").and_then(|s| s.get("last_fired_at")),
                    })
                })
                .collect();
            Ok(json!({ "paused": paused, "automations": out }))
        }
        "list_automation_runs" | "swervebuild_list_automation_runs" | "swervegrok_list_automation_runs" => {
            let automation_id = args
                .get("automation_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "automation_id required".to_string())?;
            if !is_safe_id(automation_id) {
                return Err("invalid automation_id".to_string());
            }
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let dir = swerve_build_lib::paths::run_dir(automation_id);
            let mut runs: Vec<Value> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|x| x.to_str()) != Some("json") {
                        continue; // skip .jsonl transcripts and .prompt.txt
                    }
                    if let Some(v) = std::fs::read_to_string(&p)
                        .ok()
                        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                    {
                        runs.push(json!({
                            "run_id": v.get("id"),
                            "status": v.get("status"),
                            "trigger_reason": v.get("trigger_reason"),
                            "started_at": v.get("started_at"),
                            "finished_at": v.get("finished_at"),
                            "final_text": v.get("final_text"),
                        }));
                    }
                }
            }
            runs.sort_by(|a, b| {
                let sa = a.get("started_at").and_then(|s| s.as_str()).unwrap_or("");
                let sb = b.get("started_at").and_then(|s| s.as_str()).unwrap_or("");
                sb.cmp(sa) // newest first
            });
            runs.truncate(limit);
            Ok(json!({ "runs": runs }))
        }
        "get_chat_summary" | "swervebuild_get_chat_summary" | "swervegrok_get_chat_summary" => {
            let chat_id = args
                .get("chat_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "chat_id required".to_string())?;
            let chat = store
                .get("chats")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.iter().find(|c| c.get("id").and_then(|v| v.as_str()) == Some(chat_id)))
                .ok_or_else(|| "chat not found".to_string())?;
            Ok(json!({
                "id": chat.get("id"),
                "title": chat.get("title"),
                "project_id": chat.get("project_id"),
                "message_count": chat.get("messages").and_then(|m| m.as_array()).map(|a| a.len()).unwrap_or(0),
                "grok_session_id": chat.get("grok_session_id"),
                "updated_at": chat.get("updated_at"),
            }))
        }
        "app_ui_state" | "swervebuild_app_ui_state" => {
            swerve_build_lib::app_ui::require_grant()?;
            Ok(swerve_build_lib::app_ui::state_report())
        }
        "app_ui_snapshot" | "swervebuild_app_ui_snapshot" => {
            swerve_build_lib::app_ui::require_grant()?;
            let report = swerve_build_lib::app_ui::state_report();
            let route = report
                .get("route")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let title = report
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let modal = report
                .get("permissionModalOpen")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let drive_ready = report
                .get("driveReady")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let digest = format!(
                "SwerveBuild UI digest (published state, not full DOM)\nroute: {route}\ntitle: {title}\npermission_modal_open: {modal}\ndrive_ready: {drive_ready}"
            );
            // Soft cap — keep under agent context budgets.
            let capped: String = digest.chars().take(4_000).collect();
            Ok(json!({ "digest": capped, "chars": capped.chars().count() }))
        }
        "app_ui_screenshot" | "swervebuild_app_ui_screenshot" => {
            swerve_build_lib::app_ui::screenshot()
        }
        "app_ui_click" | "swervebuild_app_ui_click" => {
            let selector = args
                .get("selector")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "selector required".to_string())?;
            swerve_build_lib::app_ui::click(selector)
        }
        "app_ui_type" | "swervebuild_app_ui_type" => {
            let selector = args
                .get("selector")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "selector required".to_string())?;
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "text required".to_string())?;
            let clear = args.get("clear").and_then(|v| v.as_bool()).unwrap_or(true);
            swerve_build_lib::app_ui::type_text(selector, text, clear)
        }
        "app_ui_press" | "swervebuild_app_ui_press" => {
            let key = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "key required".to_string())?;
            swerve_build_lib::app_ui::press_key(key)
        }
        "app_ui_wait" | "swervebuild_app_ui_wait" => {
            let condition = args
                .get("condition")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "condition required".to_string())?;
            let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64());
            swerve_build_lib::app_ui::wait_for(condition, timeout_ms)
        }
        "term_state" | "swervebuild_term_state" => Ok(swerve_build_lib::terminal::state_report()),
        "term_run" | "swervebuild_term_run" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "command required".to_string())?;
            let cwd = args.get("cwd").and_then(|v| v.as_str());
            let project_id = args.get("project_id").and_then(|v| v.as_str());
            let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64());
            let shell = args.get("shell").and_then(|v| v.as_str());
            swerve_build_lib::terminal::run_command(command, project_id, cwd, timeout_secs, shell)
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

/// A proper JSON-RPC error object. Callers previously got either a `result`
/// containing an "error" key (non-standard, so clients couldn't detect failure)
/// or no reply at all.
fn error_response(id: &Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn handle(request: &RpcRequest) -> Option<Value> {
    let id = request.id.clone()?;
    let Some(method) = request.method.as_deref() else {
        return Some(error_response(&id, -32600, "Invalid Request: missing method".into()));
    };

    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "swervebuild-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
        "notifications/initialized" => return None,
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            // Every request carrying an id MUST get a reply. The old `?` returned
            // None on malformed params, sending nothing at all — a strict client
            // then waits on that id forever.
            let Some(params) = request.params.as_ref() else {
                return Some(error_response(&id, -32602, "Invalid params: missing params".into()));
            };
            let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
                return Some(error_response(
                    &id,
                    -32602,
                    "Invalid params: missing tool name".into(),
                ));
            };
            let empty_args = json!({});
            let args = params.get("arguments").unwrap_or(&empty_args);
            match call_tool(name, args) {
                Ok(data) => json!({
                    "content": [{ "type": "text", "text": data.to_string() }],
                    "isError": false
                }),
                Err(error) => json!({
                    "content": [{ "type": "text", "text": error }],
                    "isError": true
                }),
            }
        }
        other => {
            // JSON-RPC "method not found" — not a success result that happens to
            // contain an "error" key, which clients have no way to detect.
            return Some(error_response(
                &id,
                -32601,
                format!("Method not found: {other}"),
            ));
        }
    };

    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines().map_while(Result::ok) {
        // Strip any BOM a host shell's pipe machinery may prepend (Windows
        // PowerShell 5.1 injects a UTF-8 preamble into redirected stdin, which
        // would otherwise make the first request unparseable) before parsing.
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<RpcRequest>(line) else {
            continue;
        };
        if let Some(response) = handle(&request) {
            let _ = writeln!(stdout, "{}", response);
            let _ = stdout.flush();
        }
    }
}