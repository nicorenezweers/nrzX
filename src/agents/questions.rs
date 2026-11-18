//! Rückfragen des Agenten (`ask_user` im MCP-Server von nrzx): Die Frage wird als Nachricht
//! (Rolle `question`, Daten [`QuestionData`]) gespeichert, der Thread wartet („waiting“), und der
//! Aufruf des Werkzeugs blockiert, bis der Benutzer in der Oberfläche antwortet, die Entscheidung
//! dem Agenten überlässt, den Lauf stoppt oder die Zeit abläuft. Im FreeStyle-Modus und für
//! Subagenten gibt es keine Rückfragen.

use super::{messages, runs, runtime, threads};
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{AnswerQuestion, QuestionData, QuestionOption};
use nrzx_shared::{Message, ThreadStatus, ThreadSummary};
use rocket::serde::json::Json;
use rocket::{post, routes, Route, State};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub fn routes() -> Vec<Route> {
    routes![answer]
}

/// Längste Wartezeit auf eine Antwort (unter dem Zeitlimit der CLIs für MCP-Aufrufe).
const MAX_WAIT: Duration = Duration::from_secs(50 * 60);

/// Antwort, wenn der Benutzer die Entscheidung abgibt.
const DELEGATED: &str = "The user leaves this decision to you: choose the most sensible option yourself and continue.";

/// Offene Fragen: Nachrichten-ID → Rückkanal zum wartenden Werkzeugaufruf.
static PENDING: Mutex<Option<HashMap<i64, oneshot::Sender<String>>>> = Mutex::new(None);

fn pending() -> std::sync::MutexGuard<'static, Option<HashMap<i64, oneshot::Sender<String>>>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Darf dieser Thread Rückfragen stellen?
pub fn allowed(t: &threads::ThreadRow) -> bool {
    !t.freestyle && !t.read_only && t.parent_id.is_none()
}

/// Optionen aus den Argumenten: Liste aus Texten oder `{label, description}`.
fn options(v: &serde_json::Value) -> Vec<QuestionOption> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|o| match o {
                    serde_json::Value::String(s) => Some(QuestionOption { label: s.trim().to_string(), description: String::new() }),
                    o => Some(QuestionOption {
                        label: o["label"].as_str()?.trim().to_string(),
                        description: o["description"].as_str().unwrap_or("").trim().to_string(),
                    }),
                })
                .filter(|o| !o.label.is_empty())
                .take(8)
                .collect()
        })
        .unwrap_or_default()
}

/// Frage stellen und auf die Antwort warten (Werkzeug `ask_user`). Liefert den Text für den Agenten.
pub async fn ask(state: &AppState, thread: &threads::ThreadRow, run_id: i64, args: &serde_json::Value) -> AppResult<String> {
    if !allowed(thread) {
        return Ok("FreeStyle mode: the user does not want questions. Decide yourself sensibly and continue.".into());
    }
    let question: String = args["question"].as_str().unwrap_or("").trim().chars().take(2000).collect();
    if question.is_empty() {
        return Err(AppError::bad("question fehlt"));
    }
    let data = QuestionData { options: options(&args["options"]), multiple: args["multiple"].as_bool().unwrap_or(false), ..Default::default() };
    let mut msg = messages::insert(state, thread.user_id, thread.id, Some(run_id), "question", &question, &messages::jv(&data)).await?;
    let (tx, mut rx) = oneshot::channel();
    pending().get_or_insert_with(HashMap::new).insert(msg.id, tx);
    threads::set_status(state, thread.id, ThreadStatus::Waiting, Some(true)).await?;
    threads::publish(state, thread.id).await?;

    // Warten – bis zur Antwort, zum Ende des Laufs oder zur Zeitgrenze
    let started = Instant::now();
    let answer = loop {
        match tokio::time::timeout(Duration::from_secs(2), &mut rx).await {
            Ok(Ok(a)) => break Some(a),
            Ok(Err(_)) => break None,
            Err(_) => {
                let run_alive = runtime::get(state)
                    .active
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&thread.id)
                    .is_some_and(|a| a.run_id == run_id);
                if !run_alive || started.elapsed() > MAX_WAIT {
                    break None;
                }
            }
        }
    };
    pending().as_mut().map(|m| m.remove(&msg.id));
    // Ohne Antwort ist die Frage abgelaufen – zuerst festhalten: entscheidet der Agent selbst und
    // macht weiter, wartet er nicht mehr auf den Benutzer (kein Gelb mehr in der Seitenleiste)
    if answer.is_none() {
        let mut d: QuestionData = serde_json::from_value(msg.data.clone()).unwrap_or_default();
        d.expired = true;
        let content = msg.content.clone();
        messages::update(state, thread.user_id, &mut msg, &content, &messages::jv(&d)).await?;
    }
    if runtime::get(state).is_active(thread.id) {
        threads::set_status(state, thread.id, ThreadStatus::Working, None).await?;
    }
    threads::publish(state, thread.id).await?;
    Ok(answer.unwrap_or_else(|| "No answer from the user. Decide yourself sensibly and continue; mention the decision in your summary.".into()))
}

/// `POST /threads/<id>/questions/<mid>` – Rückfrage beantworten (oder dem Agenten überlassen).
/// Wartet der Agent nicht mehr (Lauf beendet), wird die Antwort als neue Nachricht gesendet.
#[post("/threads/<id>/questions/<mid>", data = "<body>")]
async fn answer(state: &State<AppState>, user: CurrentUser, _port: runtime::LocalPort, id: i64, mid: i64, body: Json<AnswerQuestion>) -> AppResult<Json<ThreadSummary>> {
    threads::get_row(state, user.id(), id).await?;
    let row: Option<messages::MessageRow> =
        sqlx::query_as("SELECT * FROM messages WHERE id = ? AND thread_id = ? AND role = 'question'").bind(mid).bind(id).fetch_optional(&state.db).await?;
    let Some(row) = row else { return Err(AppError::NotFound) };
    let mut msg: Message = row.to_json();
    let mut d: QuestionData = serde_json::from_value(msg.data.clone()).unwrap_or_default();
    if d.answer.is_some() {
        return Err(AppError::conflict("Diese Frage ist schon beantwortet."));
    }
    let b = body.into_inner();
    let text: String = b.answer.trim().chars().take(4000).collect();
    if text.is_empty() && !b.delegate {
        return Err(AppError::bad("Bitte eine Antwort wählen oder eingeben."));
    }
    d.delegated = b.delegate;
    d.answer = Some(if b.delegate { String::new() } else { text.clone() });
    let content = msg.content.clone();
    messages::update(state, user.id(), &mut msg, &content, &messages::jv(&d)).await?;

    let for_agent = if b.delegate { DELEGATED.to_string() } else { format!("The user answered: {text}") };
    let waiting = pending().as_mut().and_then(|m| m.remove(&mid));
    let delivered = waiting.is_some_and(|tx| tx.send(for_agent).is_ok());
    if !delivered {
        {
            // Agent wartet nicht mehr: als neue Nachricht weitergeben
            if runtime::get(state).is_active(id) {
                return Err(AppError::conflict("Der Agent arbeitet noch – die Frage ist abgelaufen."));
            }
            let prompt = format!("Regarding your question \"{}\": {}", content.chars().take(300).collect::<String>(), if b.delegate { DELEGATED } else { &text });
            runs::start(state, id, &prompt, &[]).await?;
        }
    }
    Ok(Json(threads::summary(state, id).await?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_options() {
        let v = serde_json::json!(["A", {"label": "B", "description": "b"}, {"x": 1}, ""]);
        let o = super::options(&v);
        assert_eq!(o.len(), 2);
        assert_eq!(o[1].description, "b");
    }
}
