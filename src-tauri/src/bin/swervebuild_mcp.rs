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
    ]
}

fn data_path() -> PathBuf {
    swerve_build_lib::paths::data_file()
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
        "swervebuild_get_app_status" | "swervegrok_get_app_status" => Ok(json!({
            "data_path": data_path().display().to_string(),
            "project_count": store.get("projects").and_then(|p| p.as_array()).map(|a| a.len()).unwrap_or(0),
            "chat_count": store.get("chats").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0),
        })),
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
                "version": "0.1.0"
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
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<RpcRequest>(&line) else {
            continue;
        };
        if let Some(response) = handle(&request) {
            let _ = writeln!(stdout, "{}", response);
            let _ = stdout.flush();
        }
    }
}