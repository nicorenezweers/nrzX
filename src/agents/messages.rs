//! Nachrichten eines Threads speichern und live senden (`HubEvent::Message`; gleiche ID
//! ersetzt die Nachricht beim Streamen). Markdown von Benutzer- und Agent-Nachrichten wird
//! beim Schreiben gerendert.

use crate::db::now;
use crate::error::AppResult;
use crate::{hub, markdown, AppState};
use nrzx_shared::{HubEvent, Message};
use serde_json::Value;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MessageRow {
    pub id: i64,
    pub thread_id: i64,
    pub run_id: Option<i64>,
    pub role: String,
    pub content: String,
    pub data: String,
    pub html: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Hinweis an den Agenten vor einem Zwischenruf (geht nur an das CLI, nicht in den Chat).
pub const INTERJECTION_NOTE: &str = "(The user interrupted you with this message – take it into account, then continue with the task if it still applies.)";

impl MessageRow {
    pub fn to_json(&self) -> Message {
        // Ältere Zwischenrufe haben den Hinweis noch im Text: im Chat weglassen
        let (content, html) = match self.content.strip_prefix(INTERJECTION_NOTE).filter(|_| self.role == "user") {
            Some(rest) => (rest.trim_start().to_string(), render("user", rest.trim_start())),
            None => (self.content.clone(), self.html.clone()),
        };
        Message {
            id: self.id,
            thread_id: self.thread_id,
            run_id: self.run_id,
            role: self.role.clone(),
            content,
            html,
            data: serde_json::from_str(&self.data).unwrap_or(Value::Object(Default::default())),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

fn render(role: &str, content: &str) -> String {
    if matches!(role, "user" | "assistant") && !content.trim().is_empty() {
        markdown::render_plain(content)
    } else {
        String::new()
    }
}

/// Neue Nachricht speichern und senden.
pub async fn insert(state: &AppState, user_id: i64, thread_id: i64, run_id: Option<i64>, role: &str, content: &str, data: &Value) -> AppResult<Message> {
    let ts = now();
    let html = render(role, content);
    let data_text = data.to_string();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO messages (thread_id, run_id, role, content, data, html, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(thread_id)
    .bind(run_id)
    .bind(role)
    .bind(content)
    .bind(&data_text)
    .bind(&html)
    .bind(ts)
    .bind(ts)
    .fetch_one(&state.db)
    .await?;
    let m = Message { id, thread_id, run_id, role: role.into(), content: content.into(), html, data: data.clone(), created_at: ts, updated_at: ts };
    hub::publish(state, user_id, HubEvent::Message { message: m.clone() });
    Ok(m)
}

/// Nachricht fortschreiben (Inhalt und Daten) und senden.
pub async fn update(state: &AppState, user_id: i64, m: &mut Message, content: &str, data: &Value) -> AppResult<()> {
    let ts = now();
    m.html = render(&m.role, content);
    m.content = content.to_string();
    m.data = data.clone();
    m.updated_at = ts;
    sqlx::query("UPDATE messages SET content = ?, data = ?, html = ?, updated_at = ? WHERE id = ?")
        .bind(&m.content)
        .bind(m.data.to_string())
        .bind(&m.html)
        .bind(ts)
        .bind(m.id)
        .execute(&state.db)
        .await?;
    hub::publish(state, user_id, HubEvent::Message { message: m.clone() });
    Ok(())
}

/// Nachrichten eines Threads (ab `after`, höchstens `limit`).
pub async fn list(state: &AppState, thread_id: i64, after: i64, limit: i64) -> AppResult<Vec<Message>> {
    let rows: Vec<MessageRow> = sqlx::query_as("SELECT * FROM messages WHERE thread_id = ? AND id > ? ORDER BY id LIMIT ?")
        .bind(thread_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&state.db)
        .await?;
    Ok(rows.iter().map(MessageRow::to_json).collect())
}

/// Die letzten `n` Nachrichten eines Threads (älteste zuerst) – für nrzXMobile.
pub async fn tail(state: &AppState, thread_id: i64, n: i64) -> AppResult<Vec<Message>> {
    let mut rows: Vec<MessageRow> = sqlx::query_as("SELECT * FROM messages WHERE thread_id = ? ORDER BY id DESC LIMIT ?")
        .bind(thread_id)
        .bind(n)
        .fetch_all(&state.db)
        .await?;
    rows.reverse();
    Ok(rows.iter().map(MessageRow::to_json).collect())
}

/// Text kürzen (Zeichen, nicht Bytes) – mit Hinweis am Ende.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}\n… (gekürzt)")
}

/// Als JSON-Wert (die Formen hier lassen sich immer serialisieren).
pub fn jv<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_default()
}
