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

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "swervebuild_list_projects".into(),
            description: "List all Swerve Build projects (folders) with paths and chat counts.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "swervebuild_list_chats".into(),
            description: "List chats for a project. Optional project_id; omit for all projects.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "description": "Swerve Build project id" }
                }
            }),
        },
        ToolDef {
            name: "swervebuild_get_app_status".into(),
            description: "Get Swerve Build app status: projects, chats, data file path.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "swervebuild_get_chat_summary".into(),
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
            name: "swervebuild_list_automations".into(),
            description: "List Swerve Build automations (triggered agents): id, name, whether enabled, trigger, execution mode, and last run status.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "swervebuild_list_automation_runs".into(),
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
        "swervebuild_list_projects" | "swervegrok_list_projects" => {
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
        "swervebuild_list_chats" | "swervegrok_list_chats" => {
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
        "swervebuild_get_app_status" | "swervegrok_get_app_status" => {
            let autos = load_json(swerve_build_lib::paths::automations_file());
            Ok(json!({
                "data_path": data_path().display().to_string(),
                "project_count": store.get("projects").and_then(|p| p.as_array()).map(|a| a.len()).unwrap_or(0),
                "chat_count": store.get("chats").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0),
                "automation_count": autos.get("automations").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0),
                "automations_paused": autos.get("paused").and_then(|v| v.as_bool()).unwrap_or(false),
            }))
        }
        "swervebuild_list_automations" | "swervegrok_list_automations" => {
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
        "swervebuild_list_automation_runs" | "swervegrok_list_automation_runs" => {
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
        "swervebuild_get_chat_summary" | "swervegrok_get_chat_summary" => {
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
        _ => Err(format!("Unknown tool: {name}")),
    }
}

fn handle(request: &RpcRequest) -> Option<Value> {
    let id = request.id.clone()?;
    let method = request.method.as_deref()?;

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
            let params = request.params.as_ref()?;
            let name = params.get("name")?.as_str()?;
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
        _ => json!({ "error": format!("unsupported method: {method}") }),
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