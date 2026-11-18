//! `/btw` – Seiten-Chat zum Zwischenstand: Eine Frage („wie weit bist du?“, „was machst du
//! gerade?“) geht mit einem verdichteten Verlauf des Threads (letzte Nachrichten, Werkzeugschritte
//! als Einzeiler, Todo-Liste) an einen kurzen Nebenlauf (`runs::oneshot`, nur lesen) – mit dem im
//! Seiten-Chat gewählten Modell (auch ein anderer Agent), sonst einem günstigen Modell. Der Agent
//! wird nicht unterbrochen, nichts davon kommt in seinen Verlauf.
//!
//! `/status` (`POST /threads/<id>/status`) nutzt denselben verdichteten Verlauf für einen Stand mit
//! Zeitschätzung (Zeile `ESTIMATE:`), gespeichert wird dabei nichts; Laufzeit und Todo-Fortschritt
//! kommen als Fakten direkt aus der Datenbank dazu.

use super::messages::clip;
use super::{runs, threads};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::{markdown, AppState};
use nrzx_shared::agents::{split_estimate, BtwAsk, BtwEntry, StatusAsk, StatusReport, ToolCall};
use rocket::serde::json::Json;
use rocket::{delete, get, post, routes, Route, State};
use serde_json::Value;

pub fn routes() -> Vec<Route> {
    routes![list, ask, clear, status]
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    thread_id: i64,
    question: String,
    answer: String,
    status: String,
    created_at: i64,
}

impl Row {
    fn to_json(&self) -> BtwEntry {
        BtwEntry {
            id: self.id,
            thread_id: self.thread_id,
            question: self.question.clone(),
            html: if self.status == "done" { markdown::render_plain(&self.answer) } else { String::new() },
            answer: self.answer.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
        }
    }
}

async fn entries(state: &AppState, thread_id: i64) -> AppResult<Vec<Row>> {
    Ok(sqlx::query_as("SELECT id, thread_id, question, answer, status, created_at FROM btw_entries WHERE thread_id = ? ORDER BY id DESC LIMIT 50")
        .bind(thread_id)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .rev()
        .collect())
}

/// `GET /threads/<id>/btw`
#[get("/threads/<id>/btw")]
async fn list(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<Vec<BtwEntry>>> {
    threads::get_row(state, user.id(), id).await?;
    Ok(Json(entries(state, id).await?.iter().map(Row::to_json).collect()))
}

/// `DELETE /threads/<id>/btw` – Seiten-Chat leeren.
#[delete("/threads/<id>/btw")]
async fn clear(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<Vec<BtwEntry>>> {
    threads::get_row(state, user.id(), id).await?;
    sqlx::query("DELETE FROM btw_entries WHERE thread_id = ?").bind(id).execute(&state.db).await?;
    Ok(Json(Vec::new()))
}

/// `POST /threads/<id>/btw` – Frage stellen; wartet auf die Antwort des Nebenlaufs.
#[post("/threads/<id>/btw", data = "<body>")]
async fn ask(state: &State<AppState>, user: CurrentUser, _port: super::runtime::LocalPort, id: i64, body: Json<BtwAsk>) -> AppResult<Json<BtwEntry>> {
    let t = threads::get_row(state, user.id(), id).await?;
    let question: String = body.question.trim().chars().take(2000).collect();
    let question = if question.is_empty() { "Wie ist der aktuelle Stand?".to_string() } else { question };
    let ts = now();
    let entry_id: i64 = sqlx::query_scalar("INSERT INTO btw_entries (thread_id, question, created_at, updated_at) VALUES (?, ?, ?, ?) RETURNING id")
        .bind(id)
        .bind(&question)
        .bind(ts)
        .bind(ts)
        .fetch_one(&state.db)
        .await?;
    let prompt = build_prompt(state, &t, &question, entry_id).await?;
    let pick = body.agent.map(|agent| runs::SidePick { agent, account_id: body.account_id, model: body.model.trim().to_string(), effort: body.effort.trim().to_string() });
    let (answer, status) = match runs::oneshot(state, id, &prompt, pick).await {
        Ok(a) => (a, "done"),
        Err(e) => (e.to_string(), "error"),
    };
    sqlx::query("UPDATE btw_entries SET answer = ?, status = ?, updated_at = ? WHERE id = ?")
        .bind(&answer)
        .bind(status)
        .bind(now())
        .bind(entry_id)
        .execute(&state.db)
        .await?;
    let row: Row = sqlx::query_as("SELECT id, thread_id, question, answer, status, created_at FROM btw_entries WHERE id = ?")
        .bind(entry_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(row.to_json()))
}

/// `POST /threads/<id>/status` – Stand und geschätzte Restdauer (nichts wird gespeichert).
#[post("/threads/<id>/status", data = "<body>")]
async fn status(state: &State<AppState>, user: CurrentUser, _port: super::runtime::LocalPort, id: i64, body: Json<StatusAsk>) -> AppResult<Json<StatusReport>> {
    let t = threads::get_row(state, user.id(), id).await?;
    let summary = threads::summary(state, id).await?;
    let active = summary.status.is_active();
    let (todos_done, todos_total, todo_current) = todo_progress(state, id).await?;
    let lang = if body.lang.trim().is_empty() { "de".to_string() } else { body.lang.trim().to_string() };
    let elapsed = summary.run_started_at.map(|s| (now() - s).max(0) / 60);
    let facts = format!(
        "Facts: the agent is {}{}{}.",
        if active { "working right now" } else { "not working right now (finished, stopped or idle)" },
        elapsed.filter(|_| active).map(|m| format!(", this run started {m} minutes ago")).unwrap_or_default(),
        if todos_total > 0 { format!(", its todo list has {todos_done} of {todos_total} items done") } else { String::new() },
    );
    let question = format!(
        "{facts}\n\nGive the current status of the task and an estimate of how much longer the agent will need until the whole task is done. \
         Reply in the language with the code \"{lang}\", in exactly this format:\n\
         First line: `ESTIMATE: <short estimate, e.g. \"about 10 min\", \"1–2 h\", \"almost done\", or \"done\" if it is not working and the task looks finished>` \
         (keep the word ESTIMATE in English, translate the value).\n\
         Then 2–5 short bullet points: what is done, what it is doing now, what is still open (blockers if any). No headings, no preamble."
    );
    let prompt = build_prompt(state, &t, &question, i64::MAX).await?;
    let pick = body.agent.map(|agent| runs::SidePick { agent, account_id: body.account_id, model: body.model.trim().to_string(), effort: body.effort.trim().to_string() });
    let mut report = StatusReport { active, running_since: summary.run_started_at.filter(|_| active), todos_done, todos_total, todo_current, ..Default::default() };
    match runs::oneshot(state, id, &prompt, pick).await {
        Ok(answer) => {
            let (estimate, rest) = split_estimate(&answer);
            report.estimate = estimate;
            report.html = markdown::render_plain(&rest);
            report.summary = rest;
        }
        Err(e) => report.error = e.to_string(),
    }
    Ok(Json(report))
}

/// Fortschritt der letzten Todo-Liste: (erledigt, gesamt, aktueller Eintrag).
async fn todo_progress(state: &AppState, thread_id: i64) -> AppResult<(u32, u32, String)> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT data FROM messages WHERE thread_id = ? AND role = 'tool' ORDER BY id DESC LIMIT 400")
        .bind(thread_id)
        .fetch_all(&state.db)
        .await?;
    for (data,) in rows {
        let call: ToolCall = serde_json::from_str(&data).unwrap_or_default();
        if call.kind != "todo" {
            continue;
        }
        let items = call.input["todos"].as_array().or(call.input["items"].as_array()).or(call.input["plan"].as_array()).cloned().unwrap_or_default();
        let text = |i: &Value| ["content", "text", "description", "step", "title"].iter().find_map(|k| i[*k].as_str()).unwrap_or("").to_string();
        let done = items.iter().filter(|i| i["completed"].as_bool() == Some(true) || i["status"].as_str() == Some("completed")).count() as u32;
        let current = items.iter().find(|i| i["status"].as_str() == Some("in_progress")).map(text).unwrap_or_default();
        return Ok((done, items.len() as u32, current));
    }
    Ok((0, 0, String::new()))
}

/// Verdichteter Verlauf für die Seitenfrage (höchstens ~24 000 Zeichen).
async fn build_prompt(state: &AppState, t: &threads::ThreadRow, question: &str, entry_id: i64) -> AppResult<String> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT role, content, data FROM messages WHERE thread_id = ? ORDER BY id DESC LIMIT 160").bind(t.id).fetch_all(&state.db).await?;
    let mut lines: Vec<String> = Vec::new();
    let mut todos: Option<String> = None;
    let mut budget = 20_000usize;
    for (role, content, data) in rows {
        let line = match role.as_str() {
            "user" => format!("USER: {}", clip(content.trim(), 1500)),
            "assistant" => format!("AGENT: {}", clip(content.trim(), 1500)),
            "error" => format!("ERROR: {}", clip(content.trim(), 300)),
            "question" => format!("AGENT ASKED THE USER: {}", clip(content.trim(), 300)),
            "tool" => {
                let call: ToolCall = serde_json::from_str(&data).unwrap_or_default();
                if call.kind == "todo" && todos.is_none() {
                    todos = Some(todo_text(&call.input));
                    continue;
                }
                if call.kind == "think" {
                    continue;
                }
                let s = if call.summary.is_empty() { call.name.clone() } else { call.summary.clone() };
                format!("  · {}{}", clip(&s, 160), if call.status == "error" { " (failed)" } else { "" })
            }
            _ => continue,
        };
        if line.len() > budget {
            break;
        }
        budget -= line.len();
        lines.push(line);
    }
    lines.reverse();
    let status = match t.status() {
        s if s.is_active() => "still working",
        nrzx_shared::ThreadStatus::Error => "stopped with an error",
        _ => "finished / idle",
    };
    // Frühere Fragen im Seiten-Chat (für Rückbezüge)
    let earlier: Vec<(String, String)> = sqlx::query_as("SELECT question, answer FROM btw_entries WHERE thread_id = ? AND id < ? AND status = 'done' ORDER BY id DESC LIMIT 3")
        .bind(t.id)
        .bind(entry_id)
        .fetch_all(&state.db)
        .await?;
    let mut p = format!(
        "A coding agent ({}) works on the task \"{}\". Its current state: {status}.\n\
         Below is a condensed transcript (oldest first; tool steps as one-liners). Answer the user's side question \
         about the progress from this transcript only – concise, concrete (what is done, what is in progress, what is next, \
         blockers). Use a short list if helpful. Do not use tools.\n\n",
        t.kind().name(),
        t.title
    );
    if let Some(todo) = todos {
        p.push_str(&format!("Agent's todo list (latest):\n{todo}\n\n"));
    }
    p.push_str("Transcript:\n");
    p.push_str(&lines.join("\n"));
    if !earlier.is_empty() {
        p.push_str("\n\nEarlier side questions:\n");
        for (q, a) in earlier.iter().rev() {
            p.push_str(&format!("Q: {}\nA: {}\n", clip(q, 300), clip(a, 600)));
        }
    }
    p.push_str(&format!("\n\nSide question: {question}"));
    Ok(p)
}

/// Todo-Liste (Claude, OpenCode, Gemini, Codex) als Text.
fn todo_text(input: &Value) -> String {
    let items = input["todos"].as_array().or(input["items"].as_array()).or(input["plan"].as_array()).cloned().unwrap_or_default();
    items
        .iter()
        .map(|i| {
            let text = ["content", "text", "description", "step", "title"].iter().find_map(|k| i[*k].as_str()).unwrap_or("");
            let done = i["completed"].as_bool() == Some(true) || i["status"].as_str() == Some("completed");
            let doing = i["status"].as_str() == Some("in_progress");
            format!("[{}] {text}", if done { "x" } else if doing { "~" } else { " " })
        })
        .collect::<Vec<_>>()
        .join("\n")
}
