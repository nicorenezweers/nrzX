//! Warteschlange je Thread: Nachrichten, die der Benutzer schickt, während der Agent noch
//! arbeitet, warten in `threads.queued` und gehen nach dem Lauf zusammen als nächste Nachricht
//! raus (nicht nach einem Stopp oder Rate-Limit – dann bleiben sie stehen). Mit „Zwischenrufen“
//! kommt die Nachricht als Zwischenruf in die Warteschlange und der Lauf wird im nächsten
//! passenden Moment beendet (nach einem Werkzeug bzw. einer Antwort, spätestens nach
//! [`runs::SOFT_STOP_GRACE`]) – ohne „Gestoppt“; dann geht sie gleich raus (die Sitzung des CLIs
//! läuft weiter, Subagenten arbeiten weiter).

use super::{attachments, runs, runtime, threads};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::RetractedMessage;
use nrzx_shared::{QueuedMessage, ThreadSummary};
use rocket::serde::json::Json;
use rocket::{delete, post, routes, Route, State};

pub fn routes() -> Vec<Route> {
    routes![remove, withdraw, send_now]
}

/// Höchstens so viele wartende Nachrichten je Thread.
const MAX: usize = 20;

pub async fn load(state: &AppState, thread_id: i64) -> AppResult<Vec<QueuedMessage>> {
    let raw: Option<String> = sqlx::query_scalar("SELECT queued FROM threads WHERE id = ?").bind(thread_id).fetch_optional(&state.db).await?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default())
}

async fn save(state: &AppState, thread_id: i64, list: &[QueuedMessage]) -> AppResult<()> {
    sqlx::query("UPDATE threads SET queued = ? WHERE id = ?")
        .bind(serde_json::to_string(list).unwrap_or_else(|_| "[]".into()))
        .bind(thread_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Nachricht anhängen.
pub async fn push(state: &AppState, thread_id: i64, text: &str, attachments: &[String]) -> AppResult<()> {
    push_as(state, thread_id, text, attachments, false).await
}

async fn push_as(state: &AppState, thread_id: i64, text: &str, attachments: &[String], interrupt: bool) -> AppResult<()> {
    let mut list = load(state, thread_id).await?;
    if list.len() >= MAX {
        return Err(AppError::bad("Die Warteschlange ist voll."));
    }
    list.push(QueuedMessage {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        text: text.to_string(),
        attachments: attachments.to_vec(),
        created_at: now(),
        interrupt,
    });
    save(state, thread_id, &list).await?;
    threads::publish(state, thread_id).await
}

/// Mehrere Nachrichten zu einer zusammenfassen.
fn combine(list: &[QueuedMessage]) -> (String, Vec<String>) {
    let text = if list.len() == 1 {
        list[0].text.clone()
    } else {
        list.iter().map(|q| q.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n---\n\n")
    };
    (text, list.iter().flat_map(|q| q.attachments.clone()).collect())
}

/// Nach einem Lauf: Wartendes senden (nicht nach Stopp oder geplantem Weitermachen).
pub async fn flush(state: &AppState, thread_id: i64) -> AppResult<()> {
    let t = threads::row(state, thread_id).await?;
    if t.resume_at.is_some() {
        return Ok(());
    }
    send_queued(state, thread_id).await
}

/// Wartendes jetzt senden (läuft der Agent noch, nichts tun).
async fn send_queued(state: &AppState, thread_id: i64) -> AppResult<()> {
    if runtime::get(state).is_active(thread_id) {
        return Ok(());
    }
    let list = load(state, thread_id).await?;
    if list.is_empty() {
        return Ok(());
    }
    save(state, thread_id, &[]).await?;
    let (text, atts) = combine(&list);
    let interjection = list.iter().any(|q| q.interrupt);
    let started = if interjection {
        let prompt = format!("{}\n\n{text}", super::messages::INTERJECTION_NOTE);
        runs::start_interjection(state, thread_id, &text, &prompt, &atts).await
    } else {
        runs::start(state, thread_id, &text, &atts).await
    };
    if let Err(e) = started {
        // Zurücklegen, damit nichts verloren geht
        save(state, thread_id, &list).await?;
        threads::publish(state, thread_id).await?;
        return Err(e);
    }
    Ok(())
}

/// Zwischenruf: Nachricht vormerken und den Agenten im nächsten passenden Moment unterbrechen;
/// läuft keiner (mehr), geht sie sofort raus.
pub async fn interrupt(state: &AppState, thread_id: i64, text: &str, attachments: &[String]) -> AppResult<()> {
    push_as(state, thread_id, text, attachments, true).await?;
    if !runs::signal_soft_stop(state, thread_id) {
        send_queued(state, thread_id).await?;
    }
    Ok(())
}

/// `DELETE /threads/<id>/queue/<qid>` – wartende Nachricht verwerfen.
#[delete("/threads/<id>/queue/<qid>")]
async fn remove(state: &State<AppState>, user: CurrentUser, id: i64, qid: &str) -> AppResult<Json<ThreadSummary>> {
    threads::get_row(state, user.id(), id).await?;
    let mut list = load(state, id).await?;
    list.retain(|q| q.id != qid);
    save(state, id, &list).await?;
    threads::publish(state, id).await?;
    Ok(Json(threads::summary(state, id).await?))
}

/// `POST /threads/<id>/queue/<qid>/withdraw` – wartende Nachricht zurückziehen: aus der
/// Warteschlange nehmen und Text und Anhänge zurückgeben (für den Composer). 404, wenn sie
/// schon gesendet wurde.
#[post("/threads/<id>/queue/<qid>/withdraw")]
async fn withdraw(state: &State<AppState>, user: CurrentUser, id: i64, qid: &str) -> AppResult<Json<RetractedMessage>> {
    threads::get_row(state, user.id(), id).await?;
    let mut list = load(state, id).await?;
    let Some(pos) = list.iter().position(|q| q.id == qid) else { return Err(AppError::NotFound) };
    let q = list.remove(pos);
    save(state, id, &list).await?;
    threads::publish(state, id).await?;
    // Gelöschte oder fremde Anhänge einfach weglassen
    let mut atts = Vec::new();
    for u in &q.attachments {
        if let Ok(rows) = attachments::load(state, user.id(), std::slice::from_ref(u)).await {
            atts.extend(rows.iter().map(|r| r.to_json()));
        }
    }
    Ok(Json(RetractedMessage { text: q.text, attachments: atts, thread: threads::summary(state, id).await? }))
}

/// `POST /threads/<id>/queue/<qid>/now` – wartende Nachricht sofort senden: der Agent wird gleich
/// unterbrochen (nicht erst im nächsten passenden Moment), dann geht die Warteschlange raus.
#[post("/threads/<id>/queue/<qid>/now")]
async fn send_now(state: &State<AppState>, user: CurrentUser, _port: runtime::LocalPort, id: i64, qid: &str) -> AppResult<Json<ThreadSummary>> {
    threads::get_row(state, user.id(), id).await?;
    let mut list = load(state, id).await?;
    let Some(pos) = list.iter().position(|q| q.id == qid) else { return Err(AppError::NotFound) };
    let mut q = list.remove(pos);
    q.interrupt = true;
    list.insert(pos, q);
    save(state, id, &list).await?;
    threads::publish(state, id).await?;
    if !runs::signal_interrupt_now(state, id) {
        send_queued(state, id).await?;
    }
    Ok(Json(threads::summary(state, id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines() {
        let q = |t: &str, a: &[&str]| QueuedMessage { text: t.into(), attachments: a.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        assert_eq!(combine(&[q("a", &[])]).0, "a");
        let (t, a) = combine(&[q("a", &["x"]), q("b", &["y"])]);
        assert_eq!(t, "a\n\n---\n\nb");
        assert_eq!(a, vec!["x", "y"]);
    }
}
