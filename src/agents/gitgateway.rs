//! nrzGit über nrzx statt als eigener MCP-Server: Die CLIs ausser Claude Code laden
//! alle Werkzeuge eines MCP-Servers vorab in den Kontext – bei nrzGit rund 150 Schemas in jedem
//! Lauf. Stattdessen bekommen sie je Plattform zwei kleine Werkzeuge: `nrzgit_find`
//! (passende Werkzeuge mit Schema suchen) und `nrzgit_call` (eines aufrufen). Die
//! Liste der Werkzeuge wird zehn Minuten gemerkt (je Adresse).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CACHE_FOR: Duration = Duration::from_secs(600);

static TOOLS: Mutex<Option<HashMap<String, (Instant, Vec<Value>)>>> = Mutex::new(None);

/// Name der Plattform für Meldungen.
fn title(_server: &str) -> &'static str {
    "nrzGit"
}

/// JSON-RPC an den MCP-Server der Plattform (Antwort als JSON oder als SSE-Strom).
async fn rpc(server: &str, url: &str, token: &str, method: &str, params: Value) -> Result<Value, String> {
    let forge = title(server);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let res = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("{forge}: {e}"))?;
    let status = res.status();
    let text = res.text().await.map_err(|e| format!("{forge}: {e}"))?;
    if !status.is_success() {
        return Err(format!("{forge}: HTTP {status}"));
    }
    let raw = if text.trim_start().starts_with('{') {
        text.trim().to_string()
    } else {
        text.lines().find_map(|l| l.strip_prefix("data:").map(|d| d.trim().to_string())).ok_or(format!("{forge}: leere Antwort"))?
    };
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("{forge}: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(e["message"].as_str().unwrap_or("Fehler").to_string());
    }
    Ok(v["result"].clone())
}

async fn tools(server: &str, url: &str, token: &str) -> Result<Vec<Value>, String> {
    if let Some((at, list)) = TOOLS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(url)) {
        if at.elapsed() < CACHE_FOR {
            return Ok(list.clone());
        }
    }
    let r = rpc(server, url, token, "tools/list", json!({})).await?;
    let list = r["tools"].as_array().cloned().unwrap_or_default();
    TOOLS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(url.to_string(), (Instant::now(), list.clone()));
    Ok(list)
}

/// Werkzeuge suchen: ohne Suchbegriff nur die Namen, sonst die besten Treffer mit Schema.
pub async fn find(server: &str, url: &str, token: &str, query: &str) -> Result<Value, String> {
    let list = tools(server, url, token).await?;
    let words: Vec<String> = query.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(String::from).collect();
    if words.is_empty() {
        let names: Vec<&str> = list.iter().filter_map(|t| t["name"].as_str()).collect();
        return Ok(json!({"tools": names.join(", "), "next": format!("Call {server}_find with a query to get schemas.")}));
    }
    let mut scored: Vec<(usize, &Value)> = list
        .iter()
        .map(|t| {
            let name = t["name"].as_str().unwrap_or("").to_lowercase();
            let desc = t["description"].as_str().unwrap_or("").to_lowercase();
            let score = words.iter().map(|w| if name.contains(w.as_str()) { 3 } else if desc.contains(w.as_str()) { 1 } else { 0 }).sum();
            (score, t)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let hits: Vec<Value> = scored
        .into_iter()
        .take(6)
        .map(|(_, t)| json!({"name": t["name"], "description": t["description"], "inputSchema": t["inputSchema"]}))
        .collect();
    Ok(json!({"tools": hits}))
}

/// Werkzeug der Plattform aufrufen; liefert den Text des Ergebnisses.
pub async fn call(server: &str, url: &str, token: &str, tool: &str, arguments: Value) -> Result<String, String> {
    if tool.trim().is_empty() {
        return Err("tool fehlt".into());
    }
    let r = rpc(server, url, token, "tools/call", json!({"name": tool, "arguments": arguments})).await?;
    let text: Vec<&str> = r["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect()).unwrap_or_default();
    let text = text.join("\n");
    if r["isError"].as_bool() == Some(true) {
        return Err(if text.is_empty() { format!("Fehler von {}", title(server)) } else { text });
    }
    Ok(text)
}
