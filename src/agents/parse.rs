//! Ausgabe der CLIs (eine JSON-Zeile je Ereignis) in einheitliche Ereignisse übersetzen:
//! Text (gestreamt oder ganz), Werkzeugaufrufe mit Ergebnis, Denkschritte, Sitzungs-ID,
//! Verbrauch, Fehler. Zeilen, die kein JSON sind, kommen als Rohtext.
//!
//! * Claude Code: `-p --output-format stream-json --verbose --include-partial-messages`
//! * Codex: `exec --json` (thread.started, item.*, turn.completed …)
//! * Gemini CLI: `--output-format stream-json` (init, message, tool_use, tool_result, result)
//! * OpenCode / Kilo: `run --format json` (step_start, text, tool_use, step_finish, error)
//! * Grok CLI: `-p` (OpenAI-Nachrichten als JSON-Zeilen am Ende)

use nrzx_shared::agents::EditDiff;
use nrzx_shared::AgentKind;
use serde_json::Value;

/// Einheitliches Ereignis eines Laufs.
#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    /// Sitzungs-ID des CLIs (zum Fortsetzen)
    Session(String),
    /// Stück Text, an die offene Antwort anhängen
    TextDelta(String),
    /// Ganzer Textblock (ersetzt eine gestreamte Antwort bzw. legt eine neue an)
    Text(String),
    /// Offene Antwort abschliessen (z. B. weil ein Werkzeug beginnt)
    CloseText,
    /// Denkschritt
    Thinking(String),
    ToolStart { id: String, name: String, input: Value },
    ToolUpdate { id: String, input: Value },
    ToolEnd { id: String, output: String, error: bool },
    /// Verbrauch dieses Abschnitts (wird addiert)
    UsageAdd { input: i64, output: i64, cost: f64, cache_read: i64, cache_write: i64, reasoning: i64 },
    /// Gesamtverbrauch des Laufs (ersetzt)
    UsageTotal { input: i64, output: i64, cost: f64, cache_read: i64, cache_write: i64, reasoning: i64 },
    /// Stand einer Antwort des Modells während des Laufs (Claude: je Nachrichten-ID, kommt
    /// mehrfach – der höchste Stand zählt), bis der Gesamtverbrauch ihn ersetzt
    UsageLive { id: String, input: i64, output: i64, cache_read: i64, cache_write: i64 },
    System(String),
    /// MCP-Server, die nicht verbunden sind (Namen)
    McpFailed(Vec<String>),
    Error(String),
    Raw(String),
    /// Ende: Fehler? Schlusstext (nur genutzt, wenn es sonst keine Antwort gab)
    Result { error: bool, text: String },
}

/// Zustand je Lauf.
#[derive(Debug, Default)]
pub struct Parser {
    pub kind: AgentKind,
    session_sent: bool,
}

impl Parser {
    pub fn new(kind: AgentKind) -> Self {
        Parser { kind, session_sent: false }
    }

    /// Eine Zeile der Standardausgabe.
    pub fn line(&mut self, line: &str) -> Vec<Ev> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            return vec![Ev::Raw(strip_ansi(line.trim_end()))];
        };
        if !v.is_object() {
            return vec![Ev::Raw(trimmed.to_string())];
        }
        let mut out = match self.kind {
            AgentKind::Claude => claude(&v),
            AgentKind::Codex => codex(&v),
            AgentKind::Gemini => gemini(&v),
            AgentKind::Opencode | AgentKind::Kilo => opencode(&v),
            AgentKind::Grok => grok(&v),
        };
        // Sitzungs-ID nur einmal melden
        out.retain(|e| match e {
            Ev::Session(_) if self.session_sent => false,
            Ev::Session(_) => {
                self.session_sent = true;
                true
            }
            _ => true,
        });
        out
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Ergebnis eines Werkzeugs als Text (String, Liste von Textblöcken, JSON).
pub fn content_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(t) => t.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| match i["type"].as_str() {
                Some("text") => s(&i["text"]),
                Some("image") => "[Bild]".into(),
                _ => content_text(i),
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => {
            if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                return t.to_string();
            }
            if let Some(c) = o.get("content") {
                return content_text(c);
            }
            serde_json::to_string_pretty(v).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

// ------------------------------------------------------------------ Claude Code

fn claude(v: &Value) -> Vec<Ev> {
    let mut out = Vec::new();
    // Ereignisse eigener Subagenten von Claude (Task-Werkzeug) nicht in den Thread mischen
    let nested = !v["parent_tool_use_id"].is_null() && v.get("parent_tool_use_id").is_some();
    // Verbrauch jeder Antwort (auch der eigenen Subagenten) schon während des Laufs
    if v["type"] == "assistant" {
        let m = &v["message"];
        if let (Some(id), Some(u)) = (m["id"].as_str().filter(|i| !i.is_empty()), m.get("usage").filter(|u| u.is_object())) {
            let cache_read = u["cache_read_input_tokens"].as_i64().unwrap_or(0);
            let cache_write = u["cache_creation_input_tokens"].as_i64().unwrap_or(0);
            out.push(Ev::UsageLive {
                id: id.to_string(),
                input: u["input_tokens"].as_i64().unwrap_or(0) + cache_read + cache_write,
                output: u["output_tokens"].as_i64().unwrap_or(0),
                cache_read,
                cache_write,
            });
        }
    }
    match v["type"].as_str().unwrap_or("") {
        "system" => {
            if v["subtype"] == "init" {
                if let Some(id) = v["session_id"].as_str() {
                    out.push(Ev::Session(id.to_string()));
                }
                if let Some(servers) = v["mcp_servers"].as_array() {
                    let failed: Vec<String> = servers
                        .iter()
                        .filter(|s| matches!(s["status"].as_str(), Some("failed") | Some("needs-auth")))
                        .map(|s| s["name"].as_str().unwrap_or("?").to_string())
                        .collect();
                    if !failed.is_empty() {
                        out.push(Ev::McpFailed(failed));
                    }
                }
            }
        }
        "stream_event" if !nested => {
            let ev = &v["event"];
            match ev["type"].as_str().unwrap_or("") {
                "content_block_start" => {
                    if ev["content_block"]["type"] != "text" {
                        out.push(Ev::CloseText);
                    }
                }
                "content_block_delta" => {
                    if ev["delta"]["type"] == "text_delta" {
                        if let Some(t) = ev["delta"]["text"].as_str() {
                            out.push(Ev::TextDelta(t.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }
        "assistant" if !nested => {
            if let Some(blocks) = v["message"]["content"].as_array() {
                for b in blocks {
                    match b["type"].as_str().unwrap_or("") {
                        "text" => {
                            let t = s(&b["text"]);
                            if !t.trim().is_empty() {
                                out.push(Ev::Text(t));
                            }
                        }
                        "thinking" => {
                            let t = s(&b["thinking"]);
                            if !t.trim().is_empty() {
                                out.push(Ev::Thinking(t));
                            }
                        }
                        "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                            out.push(Ev::CloseText);
                            out.push(Ev::ToolStart { id: s(&b["id"]), name: s(&b["name"]), input: b["input"].clone() });
                        }
                        _ => {}
                    }
                }
            }
        }
        "user" if !nested => {
            if let Some(blocks) = v["message"]["content"].as_array() {
                for b in blocks {
                    if b["type"] == "tool_result" {
                        out.push(Ev::ToolEnd {
                            id: s(&b["tool_use_id"]),
                            output: content_text(&b["content"]),
                            error: b["is_error"].as_bool().unwrap_or(false),
                        });
                    }
                }
            }
        }
        "result" => {
            out.push(Ev::CloseText);
            if let Some(id) = v["session_id"].as_str() {
                out.push(Ev::Session(id.to_string()));
            }
            let u = &v["usage"];
            let input = u["input_tokens"].as_i64().unwrap_or(0)
                + u["cache_creation_input_tokens"].as_i64().unwrap_or(0)
                + u["cache_read_input_tokens"].as_i64().unwrap_or(0);
            out.push(Ev::UsageTotal {
                input,
                output: u["output_tokens"].as_i64().unwrap_or(0),
                cost: v["total_cost_usd"].as_f64().unwrap_or(0.0),
                cache_read: u["cache_read_input_tokens"].as_i64().unwrap_or(0),
                cache_write: u["cache_creation_input_tokens"].as_i64().unwrap_or(0),
                reasoning: 0,
            });
            let error = v["is_error"].as_bool().unwrap_or(false) || v["subtype"].as_str().is_some_and(|s| s.starts_with("error"));
            let mut text = s(&v["result"]);
            if error && text.is_empty() {
                text = v["errors"]
                    .as_array()
                    .map(|a| a.iter().map(content_text).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_else(|| s(&v["subtype"]));
            }
            out.push(Ev::Result { error, text });
        }
        _ => {}
    }
    out
}

// ------------------------------------------------------------------ Codex

fn codex(v: &Value) -> Vec<Ev> {
    let mut out = Vec::new();
    match v["type"].as_str().unwrap_or("") {
        "thread.started" => {
            if let Some(id) = v["thread_id"].as_str() {
                out.push(Ev::Session(id.to_string()));
            }
        }
        "item.started" | "item.updated" | "item.completed" => {
            let phase = v["type"].as_str().unwrap_or("");
            let item = &v["item"];
            let id = s(&item["id"]);
            let done = phase == "item.completed";
            let failed = item["status"] == "failed" || item["status"] == "declined";
            match item["type"].as_str().unwrap_or("") {
                "agent_message" => {
                    if done {
                        let t = s(&item["text"]);
                        if !t.trim().is_empty() {
                            out.push(Ev::Text(t));
                        }
                    }
                }
                "reasoning" => {
                    if done {
                        let t = s(&item["text"]);
                        if !t.trim().is_empty() {
                            out.push(Ev::Thinking(t));
                        }
                    }
                }
                "error" => {
                    if done {
                        out.push(Ev::Error(s(&item["message"])));
                    }
                }
                kind @ ("command_execution" | "file_change" | "mcp_tool_call" | "web_search" | "todo_list" | "collab_tool_call") => {
                    let mut input = item.clone();
                    if let Some(o) = input.as_object_mut() {
                        for k in ["aggregated_output", "result", "error", "status", "id", "type"] {
                            o.remove(k);
                        }
                    }
                    let name = if kind == "mcp_tool_call" {
                        format!("mcp__{}__{}", s(&item["server"]), s(&item["tool"]))
                    } else {
                        kind.to_string()
                    };
                    if phase == "item.started" {
                        out.push(Ev::ToolStart { id: id.clone(), name, input });
                    } else if !done {
                        out.push(Ev::ToolUpdate { id: id.clone(), input });
                    } else {
                        // Abgeschlossene Elemente ohne „started“ (z. B. web_search) anlegen
                        out.push(Ev::ToolStart { id: id.clone(), name, input });
                        let output = match kind {
                            "command_execution" => {
                                let mut o = s(&item["aggregated_output"]);
                                if let Some(code) = item["exit_code"].as_i64() {
                                    if code != 0 {
                                        o.push_str(&format!("\n(exit {code})"));
                                    }
                                }
                                o
                            }
                            "mcp_tool_call" => {
                                if !item["error"].is_null() {
                                    content_text(&item["error"]["message"])
                                } else {
                                    content_text(&item["result"]["content"])
                                }
                            }
                            "file_change" => item["changes"]
                                .as_array()
                                .map(|c| {
                                    c.iter().map(|x| format!("{} {}", s(&x["kind"]), s(&x["path"]))).collect::<Vec<_>>().join("\n")
                                })
                                .unwrap_or_default(),
                            _ => String::new(),
                        };
                        let error = failed
                            || item["exit_code"].as_i64().is_some_and(|c| c != 0)
                            || (kind == "mcp_tool_call" && !item["error"].is_null());
                        out.push(Ev::ToolEnd { id, output, error });
                    }
                }
                _ => {}
            }
        }
        "turn.completed" => {
            let u = &v["usage"];
            out.push(Ev::UsageAdd {
                input: u["input_tokens"].as_i64().unwrap_or(0),
                output: u["output_tokens"].as_i64().unwrap_or(0) + u["reasoning_output_tokens"].as_i64().unwrap_or(0),
                cost: 0.0,
                cache_read: u["cached_input_tokens"].as_i64().unwrap_or(0),
                cache_write: 0,
                reasoning: u["reasoning_output_tokens"].as_i64().unwrap_or(0),
            });
        }
        "turn.failed" => {
            out.push(Ev::Result { error: true, text: s(&v["error"]["message"]) });
        }
        "error" => out.push(Ev::Error(s(&v["message"]))),
        _ => {}
    }
    out
}

// ------------------------------------------------------------------ Gemini CLI

fn gemini(v: &Value) -> Vec<Ev> {
    let mut out = Vec::new();
    match v["type"].as_str().unwrap_or("") {
        "init" => {
            if let Some(id) = v["session_id"].as_str() {
                out.push(Ev::Session(id.to_string()));
            }
        }
        "message" => {
            if v["role"] == "assistant" {
                let t = s(&v["content"]);
                if v["delta"].as_bool() == Some(true) {
                    out.push(Ev::TextDelta(t));
                } else if !t.trim().is_empty() {
                    out.push(Ev::Text(t));
                }
            }
        }
        "tool_use" => {
            out.push(Ev::CloseText);
            out.push(Ev::ToolStart { id: s(&v["tool_id"]), name: s(&v["tool_name"]), input: v["parameters"].clone() });
        }
        "tool_result" => {
            let error = v["status"] == "error";
            let output = if error && !v["error"].is_null() { s(&v["error"]["message"]) } else { content_text(&v["output"]) };
            out.push(Ev::ToolEnd { id: s(&v["tool_id"]), output, error });
        }
        "error" => {
            let msg = s(&v["message"]);
            if v["severity"] == "warning" {
                out.push(Ev::System(msg));
            } else {
                out.push(Ev::Error(msg));
            }
        }
        "result" => {
            out.push(Ev::CloseText);
            let st = &v["stats"];
            if st.is_object() {
                out.push(Ev::UsageTotal {
                    input: st["input_tokens"].as_i64().unwrap_or(0),
                    output: st["output_tokens"].as_i64().unwrap_or(0),
                    cost: 0.0,
                    cache_read: st["cached"].as_i64().or(st["cached_tokens"].as_i64()).unwrap_or(0),
                    cache_write: 0,
                    reasoning: st["thoughts"].as_i64().or(st["thoughts_tokens"].as_i64()).unwrap_or(0),
                });
            }
            let error = v["status"] == "error";
            out.push(Ev::Result { error, text: if error { s(&v["error"]["message"]) } else { String::new() } });
        }
        _ => {}
    }
    out
}

// ------------------------------------------------------------------ OpenCode / Kilo

fn opencode(v: &Value) -> Vec<Ev> {
    let mut out = Vec::new();
    if let Some(id) = v["sessionID"].as_str() {
        out.push(Ev::Session(id.to_string()));
    }
    let part = &v["part"];
    match v["type"].as_str().unwrap_or("") {
        "text" => {
            let t = s(&part["text"]);
            if !t.trim().is_empty() {
                out.push(Ev::Text(t));
            }
        }
        "reasoning" => {
            let t = s(&part["text"]);
            if !t.trim().is_empty() {
                out.push(Ev::Thinking(t));
            }
        }
        "tool_use" => {
            let st = &part["state"];
            let id = if part["callID"].is_string() { s(&part["callID"]) } else { s(&part["id"]) };
            out.push(Ev::CloseText);
            out.push(Ev::ToolStart { id: id.clone(), name: s(&part["tool"]), input: st["input"].clone() });
            match st["status"].as_str().unwrap_or("") {
                "completed" => out.push(Ev::ToolEnd { id, output: content_text(&st["output"]), error: false }),
                "error" => out.push(Ev::ToolEnd { id, output: content_text(&st["error"]), error: true }),
                _ => {}
            }
        }
        "step_finish" => {
            let t = &part["tokens"];
            out.push(Ev::UsageAdd {
                input: t["input"].as_i64().unwrap_or(0) + t["cache"]["read"].as_i64().unwrap_or(0) + t["cache"]["write"].as_i64().unwrap_or(0),
                output: t["output"].as_i64().unwrap_or(0) + t["reasoning"].as_i64().unwrap_or(0),
                cost: part["cost"].as_f64().unwrap_or(0.0),
                cache_read: t["cache"]["read"].as_i64().unwrap_or(0),
                cache_write: t["cache"]["write"].as_i64().unwrap_or(0),
                reasoning: t["reasoning"].as_i64().unwrap_or(0),
            });
        }
        "error" => {
            let e = &v["error"];
            let msg = e["data"]["message"].as_str().map(|m| m.to_string()).unwrap_or_else(|| {
                let n = s(&e["name"]);
                if n.is_empty() { content_text(e) } else { n }
            });
            out.push(Ev::Error(msg));
        }
        _ => {}
    }
    out
}

// ------------------------------------------------------------------ Grok CLI

fn grok(v: &Value) -> Vec<Ev> {
    let mut out = Vec::new();
    match v["role"].as_str().unwrap_or("") {
        "assistant" => {
            let t = s(&v["content"]);
            if let Some(calls) = v["tool_calls"].as_array() {
                if !t.trim().is_empty() {
                    out.push(Ev::Text(t.clone()));
                }
                for c in calls {
                    let args = c["function"]["arguments"].as_str().unwrap_or("{}");
                    let input = serde_json::from_str(args).unwrap_or(Value::String(args.to_string()));
                    out.push(Ev::ToolStart { id: s(&c["id"]), name: s(&c["function"]["name"]), input });
                }
            } else if t.starts_with("Error: ") {
                out.push(Ev::Error(t.trim_start_matches("Error: ").to_string()));
            } else if !t.trim().is_empty() {
                out.push(Ev::Text(t));
            }
        }
        "tool" => {
            let t = content_text(&v["content"]);
            let error = t.starts_with("Error");
            out.push(Ev::ToolEnd { id: s(&v["tool_call_id"]), output: t, error });
        }
        _ => {}
    }
    out
}

// ------------------------------------------------------------------ Werkzeuge beschreiben

/// Art, Ziel, Kurzbeschreibung und Diffs eines Werkzeugaufrufs (über alle CLIs).
pub fn describe(name: &str, input: &Value, workdirs: &[String]) -> (String, String, String, Vec<EditDiff>) {
    let lower = name.to_lowercase();
    let field = |keys: &[&str]| -> String {
        for k in keys {
            match &input[*k] {
                Value::String(t) if !t.is_empty() => return t.clone(),
                Value::Array(a) if !a.is_empty() => {
                    return a.iter().map(|x| x.as_str().map(|s| s.to_string()).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(" ")
                }
                _ => {}
            }
        }
        String::new()
    };
    let rel = |p: String| -> String {
        for w in workdirs {
            let w = w.trim_end_matches('/');
            if w.is_empty() {
                continue;
            }
            if let Some(r) = p.strip_prefix(w) {
                if r.is_empty() || r.starts_with('/') {
                    let r = r.trim_start_matches('/');
                    return if r.is_empty() { ".".into() } else { r.to_string() };
                }
            }
        }
        p
    };
    let path = rel(field(&["file_path", "filePath", "absolute_path", "path", "notebook_path", "dir_path", "file"]));

    // MCP: Claude „mcp__server__tool“, Codex (umbenannt) ebenso, OpenCode „server_tool“
    if let Some(rest) = name.strip_prefix("mcp__") {
        let (server, tool) = rest.split_once("__").unwrap_or(("", rest));
        let target = if server.is_empty() { tool.to_string() } else { format!("{server} · {tool}") };
        return ("mcp".into(), target.clone(), format!("MCP {target}"), Vec::new());
    }
    for server in ["nrzx", "nrzgit"] {
        if let Some(tool) = name.strip_prefix(&format!("{server}_")).or_else(|| name.strip_prefix(&format!("{server}__"))) {
            let target = format!("{server} · {tool}");
            return ("mcp".into(), target.clone(), format!("MCP {target}"), Vec::new());
        }
    }

    let trunc = |t: &str, n: usize| -> String {
        if t.chars().count() > n {
            format!("{}…", t.chars().take(n).collect::<String>())
        } else {
            t.to_string()
        }
    };
    let diffs = |edits: Vec<(String, String)>| -> Vec<EditDiff> {
        edits
            .into_iter()
            .map(|(b, a)| {
                let (adds, dels) = nrzx_shared::agents::line_changes(&b, &a);
                EditDiff { path: path.clone(), before: trunc(&b, 4000), after: trunc(&a, 4000), adds, dels, ..Default::default() }
            })
            .collect()
    };

    let (kind, target, verb, d): (&str, String, &str, Vec<EditDiff>) = match lower.as_str() {
        "read" | "read_file" | "view_file" | "read_many_files" | "notebookread" | "view" => ("read", path.clone(), "Read", Vec::new()),
        "write" | "write_file" | "create_file" => {
            let content = field(&["content", "contents", "file_text"]);
            ("write", path.clone(), "Write", diffs(vec![(String::new(), content)]))
        }
        "edit" | "replace" | "str_replace_editor" | "str_replace" | "notebookedit" | "edit_file" => {
            let before = field(&["old_string", "oldString", "old_str", "old_text"]);
            let after = field(&["new_string", "newString", "new_str", "new_text", "new_source"]);
            ("edit", path.clone(), "Edit", diffs(vec![(before, after)]))
        }
        "multiedit" | "multi_edit" => {
            let edits = input["edits"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|e| {
                            (
                                e["old_string"].as_str().or(e["oldString"].as_str()).unwrap_or("").to_string(),
                                e["new_string"].as_str().or(e["newString"].as_str()).unwrap_or("").to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            ("edit", path.clone(), "Edit", diffs(edits))
        }
        "file_change" | "patch" | "apply_patch" => {
            let files: Vec<String> = input["changes"]
                .as_array()
                .map(|c| c.iter().map(|x| rel(s(&x["path"]))).collect())
                .unwrap_or_default();
            let t = if files.is_empty() { path.clone() } else { files.join(", ") };
            ("edit", t, "Edit", Vec::new())
        }
        "bash" | "run_shell_command" | "shell" | "command_execution" | "exec_command" | "bashoutput" | "killshell" => {
            let cmd = field(&["command", "cmd"]);
            // Codex: "bash -lc '…'" → nur der eigentliche Befehl
            let cmd = cmd
                .strip_prefix("bash -lc ")
                .or_else(|| cmd.strip_prefix("/bin/bash -lc "))
                .or_else(|| cmd.strip_prefix("zsh -lc "))
                .map(|c| c.trim_matches(['\'', '"']).to_string())
                .unwrap_or(cmd);
            let first = cmd.lines().next().unwrap_or("").to_string();
            ("command", trunc(&first, 120), "Run", Vec::new())
        }
        "grep" | "search_file_content" | "glob" | "search" | "find" | "list" | "ls" | "list_directory" | "codebase_search" => {
            let pat = field(&["pattern", "query", "regex", "glob"]);
            let t = if pat.is_empty() { path.clone() } else { pat };
            ("search", trunc(&t, 120), "Search", Vec::new())
        }
        "webfetch" | "web_fetch" | "fetch" => ("web", trunc(&field(&["url", "prompt"]), 120), "Fetch", Vec::new()),
        "websearch" | "web_search" | "google_web_search" => ("web", trunc(&field(&["query", "q"]), 120), "Search web", Vec::new()),
        "task" | "agent" | "collab_tool_call" => ("task", trunc(&field(&["description", "title", "prompt"]), 120), "Task", Vec::new()),
        "todowrite" | "todo_list" | "write_todos" | "todoread" | "create_todo_list" | "update_todo_list" => {
            let n = input["todos"].as_array().or(input["items"].as_array()).map(|a| a.len()).unwrap_or(0);
            ("todo", if n > 0 { format!("{n}") } else { String::new() }, "Todos", Vec::new())
        }
        _ => {
            let t = if !path.is_empty() { path.clone() } else { trunc(&field(&["command", "query", "pattern", "url"]), 120) };
            ("other", t, "", Vec::new())
        }
    };
    let summary = match (verb, target.is_empty()) {
        ("", true) => name.to_string(),
        ("", false) => format!("{name} {target}"),
        (v, true) => v.to_string(),
        (v, false) => format!("{v} {target}"),
    };
    (kind.into(), target, summary, d)
}

/// ANSI-Farbcodes aus Rohtext entfernen.
pub fn strip_ansi(text: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*(\x07|\x1b\\)").unwrap());
    RE.replace_all(text, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_stream() {
        let mut p = Parser::new(AgentKind::Claude);
        assert_eq!(p.line(r#"{"type":"system","subtype":"init","session_id":"s1","mcp_servers":[{"name":"nrzx","status":"connected"}]}"#), vec![Ev::Session("s1".into())]);
        assert_eq!(
            p.line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hal"}},"parent_tool_use_id":null}"#),
            vec![Ev::TextDelta("Hal".into())]
        );
        let ev = p.line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/w/src/a.rs","old_string":"a","new_string":"b"}}]},"parent_tool_use_id":null}"#);
        assert_eq!(ev[0], Ev::CloseText);
        assert!(matches!(&ev[1], Ev::ToolStart { id, name, .. } if id == "t1" && name == "Edit"));
        let ev = p.line(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"ok"}],"is_error":false}]}}"#);
        assert_eq!(ev, vec![Ev::ToolEnd { id: "t1".into(), output: "ok".into(), error: false }]);
        // Subagent von Claude selbst: nicht im Thread
        assert!(p.line(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"inner"}]},"parent_tool_use_id":"t9"}"#).is_empty());
        // Verbrauch schon während des Laufs (auch von Subagenten)
        let ev = p.line(r#"{"type":"assistant","message":{"id":"msg_1","content":[],"usage":{"input_tokens":3,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":8}},"parent_tool_use_id":"t9"}"#);
        assert_eq!(ev, vec![Ev::UsageLive { id: "msg_1".into(), input: 113, output: 8, cache_read: 100, cache_write: 10 }]);
        let ev = p.line(r#"{"type":"result","subtype":"success","is_error":false,"result":"fertig","session_id":"s1","total_cost_usd":0.5,"usage":{"input_tokens":10,"cache_read_input_tokens":5,"output_tokens":7}}"#);
        assert!(ev.contains(&Ev::UsageTotal { input: 15, output: 7, cost: 0.5, cache_read: 5, cache_write: 0, reasoning: 0 }));
        assert!(ev.contains(&Ev::Result { error: false, text: "fertig".into() }));
        assert_eq!(p.line("kein json"), vec![Ev::Raw("kein json".into())]);
    }

    #[test]
    fn codex_items() {
        let mut p = Parser::new(AgentKind::Codex);
        assert_eq!(p.line(r#"{"type":"thread.started","thread_id":"th"}"#), vec![Ev::Session("th".into())]);
        let ev = p.line(r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"","status":"in_progress"}}"#);
        assert!(matches!(&ev[0], Ev::ToolStart { id, name, .. } if id == "item_1" && name == "command_execution"));
        let ev = p.line(r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"a\n","exit_code":0,"status":"completed"}}"#);
        assert!(ev.contains(&Ev::ToolEnd { id: "item_1".into(), output: "a\n".into(), error: false }));
        assert_eq!(p.line(r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Fertig."}}"#), vec![Ev::Text("Fertig.".into())]);
        assert_eq!(
            p.line(r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20}}"#),
            vec![Ev::UsageAdd { input: 100, output: 20, cost: 0.0, cache_read: 50, cache_write: 0, reasoning: 0 }]
        );
    }

    #[test]
    fn gemini_and_opencode() {
        let mut g = Parser::new(AgentKind::Gemini);
        assert_eq!(g.line(r#"{"type":"init","session_id":"g1","model":"x"}"#), vec![Ev::Session("g1".into())]);
        assert_eq!(g.line(r#"{"type":"message","role":"assistant","content":"Hi","delta":true}"#), vec![Ev::TextDelta("Hi".into())]);
        let mut o = Parser::new(AgentKind::Opencode);
        let ev = o.line(r#"{"type":"tool_use","sessionID":"ses_1","part":{"type":"tool","tool":"bash","callID":"c1","state":{"status":"completed","input":{"command":"ls"},"output":"x"}}}"#);
        assert_eq!(ev[0], Ev::Session("ses_1".into()));
        assert!(ev.contains(&Ev::ToolEnd { id: "c1".into(), output: "x".into(), error: false }));
        let ev = o.line(r#"{"type":"step_finish","sessionID":"ses_1","part":{"type":"step-finish","tokens":{"input":3,"output":4,"reasoning":1,"cache":{"read":2,"write":0}},"cost":0.01}}"#);
        assert_eq!(ev, vec![Ev::UsageAdd { input: 5, output: 5, cost: 0.01, cache_read: 2, cache_write: 0, reasoning: 1 }]);
    }

    #[test]
    fn describe_tools() {
        let w = vec!["/w".to_string()];
        let (k, t, s, d) = describe("Edit", &json!({"file_path": "/w/src/main.rs", "old_string": "a", "new_string": "b"}), &w);
        assert_eq!((k.as_str(), t.as_str(), s.as_str()), ("edit", "src/main.rs", "Edit src/main.rs"));
        assert_eq!(d[0].after, "b");
        assert_eq!((d[0].adds, d[0].dels), (1, 1));
        let (_, _, _, d) = describe("Write", &json!({"file_path": "/w/a.txt", "content": "1\n2\n3\n"}), &w);
        assert_eq!((d[0].adds, d[0].dels), (3, 0));
        let (k, t, _, _) = describe("command_execution", &json!({"command": "bash -lc 'cargo test'"}), &w);
        assert_eq!((k.as_str(), t.as_str()), ("command", "cargo test"));
        let (k, t, _, _) = describe("mcp__nrzgit__create_repo", &json!({}), &w);
        assert_eq!((k.as_str(), t.as_str()), ("mcp", "nrzgit · create_repo"));
        let (k, t, _, _) = describe("nrzgit_whoami", &json!({}), &w);
        assert_eq!((k.as_str(), t.as_str()), ("mcp", "nrzgit · whoami"));
        assert_eq!(strip_ansi("\x1b[31mrot\x1b[0m"), "rot");
    }
}
