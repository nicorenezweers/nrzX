//! MCP-Server von nrzX für die Agenten (`POST /mcp`, Streamable HTTP, JSON-RPC 2.0,
//! Protokoll `2025-06-18`). Anmeldung mit dem Token des Laufs (`Authorization: Bearer …`,
//! in `run_tokens`, gültig solange der Lauf läuft). Antworten kommen als einzelnes JSON
//! (kein SSE-Strom), Benachrichtigungen mit 202.
//!
//! Werkzeuge: `spawn_subagents`, `wait_for_subagents`, `subagent_status`, `stop_subagent`,
//! `open_preview`, `notify_user`, `ask_user` (nicht im FreeStyle-Modus und für Subagenten),
//! und – für CLIs ohne eigene Werkzeugsuche – `nrzgit_find`/`nrzgit_call` (siehe `gitgateway`);
//! in Gesprächen mit einem Bot `remember`/`rewrite_memory` (siehe `bots`); in Projekten mit Team
//! `team_status`/`team_send`/`team_read` (siehe `crate::team`).

use super::runtime::LocalPort;
use super::subagents;
use super::threads;
use crate::auth::session::hash_token;
use crate::error::AppResult;
use crate::{hub, AppState};
use nrzx_shared::agents::{SubagentTask, SystemData};
use nrzx_shared::{AgentKind, HubEvent};
use rocket::http::{ContentType, Header, Status};
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::{self, Responder, Response};
use rocket::{delete, get, post, routes, Route, State};
use serde_json::{json, Value};
use std::io::Cursor;
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![post_mcp, get_mcp, delete_mcp]
}

pub const PROTOCOL: &str = "2025-06-18";
const KNOWN_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Lauf hinter dem Token.
pub struct RunAuth {
    pub run_id: i64,
    pub thread_id: i64,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for RunAuth {
    type Error = ();

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, ()> {
        let Some(state) = req.rocket().state::<AppState>() else { return Outcome::Error((Status::InternalServerError, ())) };
        let token = req
            .headers()
            .get_one("Authorization")
            .and_then(|h| h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer ")))
            .map(str::trim)
            .unwrap_or("");
        if token.is_empty() {
            return Outcome::Error((Status::Unauthorized, ()));
        }
        let row: Option<(i64, i64)> = sqlx::query_as(
            "SELECT k.run_id, k.thread_id FROM run_tokens k JOIN runs r ON r.id = k.run_id WHERE k.token_hash = ? AND r.status IN ('queued', 'running')",
        )
        .bind(hash_token(token))
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        match row {
            Some((run_id, thread_id)) => Outcome::Success(RunAuth { run_id, thread_id }),
            None => Outcome::Error((Status::Unauthorized, ())),
        }
    }
}

/// Antwort des MCP-Servers.
pub struct McpReply {
    status: Status,
    body: Option<Value>,
    session: Option<String>,
}

impl<'r> Responder<'r, 'static> for McpReply {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        let mut r = Response::build();
        r.status(self.status);
        if let Some(s) = self.session {
            r.header(Header::new("Mcp-Session-Id", s));
        }
        if self.status == Status::Unauthorized {
            r.header(Header::new("WWW-Authenticate", "Bearer"));
        }
        if let Some(b) = self.body {
            let text = b.to_string();
            r.header(ContentType::JSON).sized_body(text.len(), Cursor::new(text));
        }
        r.ok()
    }
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// `POST /mcp` – JSON-RPC (einzeln oder als Liste).
#[post("/mcp", data = "<body>")]
async fn post_mcp(state: &State<AppState>, _port: LocalPort, auth: Option<RunAuth>, body: String) -> McpReply {
    let Some(auth) = auth else {
        return McpReply {
            status: Status::Unauthorized,
            body: Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32001, "message": "Ungültiges oder abgelaufenes Token."}})),
            session: None,
        };
    };
    let parsed: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return McpReply { status: Status::BadRequest, body: Some(rpc_error(&Value::Null, -32700, "Parse error")), session: None },
    };
    let mut session = None;
    let reply = match parsed {
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                if let Some(r) = handle(state, &auth, item, &mut session).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(Value::Array(out))
            }
        }
        item => handle(state, &auth, item, &mut session).await,
    };
    match reply {
        Some(b) => McpReply { status: Status::Ok, body: Some(b), session },
        None => McpReply { status: Status::Accepted, body: None, session },
    }
}

/// `GET /mcp` – kein Ereignisstrom vom Server.
#[get("/mcp")]
fn get_mcp() -> Status {
    Status::MethodNotAllowed
}

/// `DELETE /mcp` – Sitzung beenden (zustandslos, immer erfolgreich).
#[delete("/mcp")]
fn delete_mcp() -> Status {
    Status::Ok
}

async fn handle(state: &AppState, auth: &RunAuth, msg: Value, session: &mut Option<String>) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or("").to_string();
    // Antworten des Clients und Benachrichtigungen: nichts zurück
    let id = match id {
        Some(Value::Null) | None => return None,
        Some(id) => id,
    };
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result = match method.as_str() {
        "initialize" => {
            let wanted = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
            let version = if KNOWN_PROTOCOLS.contains(&wanted) { wanted } else { PROTOCOL };
            *session = Some(uuid::Uuid::new_v4().simple().to_string());
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "nrzx", "title": "nrzX", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "nrzX tools: subagents, preview, notices and questions to the user."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools_for(state, auth).await})),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(match call(state, auth, &name, args).await {
                // Text (Antwort auf eine Rückfrage, Ergebnis von nrzGit) unverändert weitergeben
                Ok(Value::String(text)) => json!({"content": [{"type": "text", "text": text}], "isError": false}),
                Ok(v) => json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                    "structuredContent": v,
                    "isError": false
                }),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            })
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, m)) => rpc_error(&id, code, &m),
    })
}

fn agent_enum() -> Value {
    json!(AgentKind::ALL.iter().map(|k| k.key()).collect::<Vec<_>>())
}

/// Werkzeuge für den Thread hinter dem Token.
async fn tools_for(state: &AppState, auth: &RunAuth) -> Value {
    let mut list = match tool_list() {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    let Ok(t) = threads::row(state, auth.thread_id).await else { return Value::Array(list) };
    if super::questions::allowed(&t) {
        list.push(json!({
            "name": "ask_user",
            "title": "Ask the user",
            "description": "Ask the user a question and wait for the answer. Only when a decision truly needs them (ambiguous goal, risky or irreversible step) – otherwise decide yourself. Offer 2–5 short options when possible; the user can also answer freely or leave the decision to you.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": {"type": "string"},
                    "options": {"type": "array", "maxItems": 8, "items": {"type": "object", "properties": {"label": {"type": "string"}, "description": {"type": "string"}}, "required": ["label"]}},
                    "multiple": {"type": "boolean", "description": "Several options may be chosen"}
                },
                "required": ["question"]
            }
        }));
    }
    if t.bot_id.is_some() {
        list.extend(super::bots::memory_tools());
    }
    if crate::team::for_thread(state, &t).await {
        list.extend(crate::team::tools());
    }
    for (server, _, _) in git_gateways(state, &t).await {
        let (title, what) = ("nrzGit", "repos, files, issues, PRs, releases, registry, ports");
        list.push(json!({
            "name": format!("{server}_find"),
            "title": format!("Find {title} tools"),
            "description": format!("Search the tools of the user's git platform {title} ({what}). Returns matching tools with their input schema; without query only all names."),
            "inputSchema": {"type": "object", "properties": {"query": {"type": "string", "description": "e.g. \"create pull request\""}}}
        }));
        list.push(json!({
            "name": format!("{server}_call"),
            "title": format!("Call an {title} tool"),
            "description": format!("Call an {title} tool found with {server}_find."),
            "inputSchema": {"type": "object", "properties": {"tool": {"type": "string"}, "arguments": {"type": "object"}}, "required": ["tool"]}
        }));
    }
    Value::Array(list)
}

/// nrzGit über nrzx (Servername `nrzgit`, Adresse, Token) – nur für CLIs
/// ohne eigene Werkzeugsuche.
async fn git_gateways(state: &AppState, t: &threads::ThreadRow) -> Vec<(&'static str, String, String)> {
    if t.read_only || super::runs::uses_tool_search(t.kind()) {
        return Vec::new();
    }
    crate::workspace::forge_accesses(state, t.user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| a.mcp().map(|(url, token)| (a.server_name(), url, token)))
        .collect()
}

/// Beschreibung der Werkzeuge (JSON Schema).
pub fn tool_list() -> Value {
    json!([
        {
            "name": "spawn_subagents",
            "title": "Start subagents",
            "description": "Start subagents in parallel, each in its own git worktree and branch. Worth it only for large, independent parts – each subagent costs a full context; do small or tightly coupled work yourself. Give each a short title and a complete, self-contained prompt (it does not see this conversation). Use effort \"low\" or a cheaper model for simple tasks. Then call wait_for_subagents.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": subagents::MAX_TASKS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": {"type": "string", "description": "Short title, e.g. \"Share link API\""},
                                "prompt": {"type": "string", "description": "Complete instructions for the subagent"},
                                "agent": {"type": "string", "enum": agent_enum(), "description": "Agent CLI (default: same as yours)"},
                                "accountId": {"type": "integer", "description": "nrzX account id (default: the user's default account for that agent)"},
                                "model": {"type": "string", "description": "Model (default: yours for the same agent, else the CLI default)"},
                                "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"]}
                            },
                            "required": ["title", "prompt"]
                        }
                    }
                },
                "required": ["tasks"]
            }
        },
        {
            "name": "wait_for_subagents",
            "title": "Wait for subagents",
            "description": "Wait until your subagents (all, or the given ids) have finished, or until the timeout. Returns per subagent: status, last answer, branch, changed files, diff stat and commits. If allDone is false, call it again.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": {"type": "array", "items": {"type": "integer"}},
                    "timeoutSeconds": {"type": "integer", "minimum": 1, "maximum": 3000, "description": "Default 600"}
                }
            }
        },
        {
            "name": "subagent_status",
            "title": "Subagent status",
            "description": "Current status of your subagents (all, or the given ids) without waiting.",
            "inputSchema": {"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "integer"}}}}
        },
        {
            "name": "stop_subagent",
            "title": "Stop a subagent",
            "description": "Stop one of your subagents (and its own subagents).",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}
        },
        {
            "name": "open_preview",
            "title": "Open preview",
            "description": "Show a web server you started (listening on localhost:<port>) in the user's preview panel.",
            "inputSchema": {
                "type": "object",
                "properties": {"port": {"type": "integer", "minimum": 1, "maximum": 65535}, "path": {"type": "string", "description": "Path to open, default /"}},
                "required": ["port"]
            }
        },
        {
            "name": "notify_user",
            "title": "Notify the user",
            "description": "Show the user a short notice (toast) and note it in the thread – for things they should know right away.",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}
        }
    ])
}

fn ids_arg(v: &Value) -> Option<Vec<i64>> {
    v["ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
}

/// Ein Werkzeug ausführen. Fehler als Text (für `isError`).
async fn call(state: &AppState, auth: &RunAuth, name: &str, args: Value) -> Result<Value, String> {
    let thread = threads::row(state, auth.thread_id).await.map_err(|e| e.to_string())?;
    let r: AppResult<Value> = async {
        match name {
            "spawn_subagents" => {
                let tasks: Vec<SubagentTask> = serde_json::from_value(args["tasks"].clone())
                    .map_err(|e| crate::error::AppError::bad(format!("tasks: {e}")))?;
                let kids = subagents::spawn(state, &thread, tasks, false).await?;
                Ok(json!({
                    "subagents": kids.iter().map(|k| json!({"id": k.id, "title": k.title, "agent": k.agent.key(), "model": k.model, "status": k.status.key(), "branch": k.branch})).collect::<Vec<_>>(),
                    "next": "Call wait_for_subagents to wait for their results."
                }))
            }
            "wait_for_subagents" => {
                let secs = args["timeoutSeconds"].as_u64().unwrap_or(600).clamp(1, 3000);
                let (all_done, reports) = subagents::wait(state, thread.id, ids_arg(&args), Duration::from_secs(secs)).await?;
                Ok(json!({"allDone": all_done, "subagents": reports}))
            }
            "subagent_status" => Ok(json!({"subagents": subagents::reports(state, thread.id, ids_arg(&args)).await?})),
            "stop_subagent" => {
                let id = args["id"].as_i64().ok_or_else(|| crate::error::AppError::bad("id fehlt"))?;
                Ok(json!({"subagent": subagents::stop(state, thread.id, id).await?}))
            }
            "open_preview" => {
                let port = args["port"].as_u64().filter(|p| (1..=65535).contains(p)).ok_or_else(|| crate::error::AppError::bad("port: 1–65535"))? as u16;
                let mut path = args["path"].as_str().unwrap_or("/").trim().to_string();
                if !path.starts_with('/') {
                    path.insert(0, '/');
                }
                hub::publish(state, thread.user_id, HubEvent::PreviewOpened { thread_id: thread.id, port, path: path.clone() });
                let data = super::messages::jv(&SystemData::of("notice"));
                super::messages::insert(state, thread.user_id, thread.id, Some(auth.run_id), "system", &format!("Vorschau geöffnet: Port {port}{path}"), &data).await?;
                Ok(json!({"opened": true, "port": port, "path": path}))
            }
            "notify_user" => {
                let text: String = args["text"].as_str().unwrap_or("").trim().chars().take(500).collect();
                if text.is_empty() {
                    return Err(crate::error::AppError::bad("text fehlt"));
                }
                hub::publish(state, thread.user_id, HubEvent::Notice { text: format!("{}: {text}", thread.title), error: false });
                let data = super::messages::jv(&SystemData::of("notice"));
                super::messages::insert(state, thread.user_id, thread.id, Some(auth.run_id), "system", &text, &data).await?;
                Ok(json!({"sent": true}))
            }
            "ask_user" => Ok(Value::String(super::questions::ask(state, &thread, auth.run_id, &args).await?)),
            "nrzgit_find" | "nrzgit_call" => {
                let (server, action) = name.split_once('_').unwrap_or((name, ""));
                let Some((server, url, token)) = git_gateways(state, &thread).await.into_iter().find(|(s, _, _)| *s == server) else {
                    return Err(crate::error::AppError::bad("nrzGit ist nicht verbunden."));
                };
                let r = if action == "find" {
                    super::gitgateway::find(server, &url, &token, args["query"].as_str().unwrap_or("")).await
                } else {
                    super::gitgateway::call(server, &url, &token, args["tool"].as_str().unwrap_or(""), args.get("arguments").cloned().unwrap_or(json!({}))).await.map(Value::String)
                };
                r.map_err(crate::error::AppError::bad)
            }
            "remember" | "rewrite_memory" => super::bots::memory_call(state, &thread, name, &args).await,
            "team_status" | "team_send" | "team_read" => crate::team::tool_call(state, &thread, name, &args).await,
            other => Err(crate::error::AppError::bad(format!("Unknown tool: {other}"))),
        }
    }
    .await;
    r.map_err(|e| e.to_string())
}
