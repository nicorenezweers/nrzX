//! Automatisch weitermachen nach einem Rate-Limit: Bricht ein Lauf ab, weil das Abo oder der
//! Schlüssel am Limit ist („usage limit reached“, „rate limit“, „429“, „quota exceeded“ …), plant
//! nrzX einen neuen Lauf zum Zeitpunkt, an dem das Limit zurückgesetzt wird
//! (`threads.resume_at`), und sendet dann „mach weiter“. Der Zeitpunkt stammt – in dieser
//! Reihenfolge – aus der Fehlermeldung (Unix-Zeit, „try again in 2 hours 5 minutes“), aus den
//! Limits des Kontos (`usage`) oder ist eine Pause, die mit jedem erfolglosen Versuch länger wird.
//! Eine eigene Nachricht, Stopp oder „Abbrechen“ in der Oberfläche heben den Plan auf.

use super::accounts;
use super::{runs, threads};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::SystemData;
use nrzx_shared::ThreadSummary;
use rocket::serde::json::Json;
use rocket::{delete, post, routes, Route, State};
use std::time::Duration;

pub fn routes() -> Vec<Route> {
    routes![cancel, resume_now]
}

/// Prompt des automatischen Laufs.
pub const RESUME_PROMPT: &str =
    "The rate limit has been reset. Please continue exactly where you left off and finish the task.";

/// Höchstens so viele automatische Versuche hintereinander.
const MAX_ATTEMPTS: i64 = 12;

/// Sieht die Meldung nach einem Rate- bzw. Nutzungslimit aus?
pub fn is_rate_limit(text: &str) -> bool {
    let t = text.to_lowercase();
    ([
        "rate limit",
        "rate_limit",
        "ratelimit",
        "usage limit",
        "limit reached",
        "hit your limit",
        "hit your usage",
        "too many requests",
        "quota exceeded",
        "exceeded your current quota",
        "resource_exhausted",
        "resource exhausted",
        "overloaded",
        "status 429",
        "error 429",
        "error: 429",
        "code 429",
        "429 too many",
        "out of credits",
        "credit balance is too low",
    ]
    .iter()
    .any(|k| t.contains(k))
        || regex::Regex::new(r"hit your [a-z0-9 -]{0,24}limit").is_ok_and(|re| re.is_match(&t)))
        && !t.contains("context window")
}

/// Uhrzeit aus „resets 8pm (Europe/Berlin)“, „resets at 20:30“, „resets 8:30 PM“: nächster
/// solcher Zeitpunkt in der Ortszeit dieses Rechners (die Zeitzone in der Meldung ist die des
/// Benutzers – nrzX läuft bei ihm).
pub fn reset_from_clock(text: &str, now_ts: i64) -> Option<i64> {
    let lower = text.to_lowercase();
    let re = regex::Regex::new(r"resets?\s+(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?\b").ok()?;
    let c = re.captures(&lower)?;
    let mut hour: i64 = c[1].parse().ok()?;
    let minute: i64 = c.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0);
    match c.get(3).map(|m| m.as_str()) {
        Some("pm") if hour < 12 => hour += 12,
        Some("am") if hour == 12 => hour = 0,
        None if c.get(2).is_none() => return None, // „resets 5“ ohne Uhrzeit-Form
        _ => {}
    }
    if hour > 23 || minute > 59 {
        return None;
    }
    let today = local_midnight(now_ts)?;
    let mut at = today + hour * 3600 + minute * 60;
    if at <= now_ts {
        at += 86_400;
    }
    Some(at)
}

/// Mitternacht (Ortszeit) des Tages von `ts` (unter Windows nicht ermittelt – dann gelten die
/// Limits des Kontos bzw. eine Pause).
#[cfg(not(unix))]
fn local_midnight(_ts: i64) -> Option<i64> {
    None
}

/// Mitternacht (Ortszeit) des Tages von `ts`.
#[cfg(unix)]
fn local_midnight(ts: i64) -> Option<i64> {
    // SAFETY: localtime_r/mktime schreiben nur in die übergebene Struktur.
    unsafe {
        let t = ts as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        let m = libc::mktime(&mut tm);
        (m != -1).then_some(m as i64)
    }
}

/// Zeitpunkt aus der Meldung: „…|1730000000“, „resets at 1730000000“, „try again in 2 hours 5
/// minutes“, „retry after 120 seconds“, „in 30s“.
pub fn reset_from_text(text: &str, now_ts: i64) -> Option<i64> {
    let re_epoch = regex::Regex::new(r"(?:\||reset[s]?(?:_at)?[^0-9]{0,12})(1[6-9]\d{8})\b").ok()?;
    if let Some(c) = re_epoch.captures(text) {
        if let Ok(v) = c[1].parse::<i64>() {
            if v > now_ts - 60 && v < now_ts + 40 * 86_400 {
                return Some(v);
            }
        }
    }
    let lower = text.to_lowercase();
    let re_in = regex::Regex::new(r"(?:in|after)\s+((?:\d+\s*(?:days?|d|hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)\b[\s,and]*)+)").ok()?;
    let caps = re_in.captures(&lower)?;
    let part = caps.get(1)?.as_str();
    let re_unit = regex::Regex::new(r"(\d+)\s*(days?|d|hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)\b").ok()?;
    let mut secs = 0i64;
    for c in re_unit.captures_iter(part) {
        let n: i64 = c[1].parse().ok()?;
        secs += match &c[2] {
            u if u.starts_with('d') => n * 86_400,
            u if u.starts_with('h') => n * 3600,
            u if u.starts_with('m') => n * 60,
            _ => n,
        };
    }
    (secs > 0).then_some(now_ts + secs)
}

/// Nach einem fehlgeschlagenen Lauf: bei einem Rate-Limit das Weitermachen planen.
pub async fn after_failure(state: &AppState, thread_id: i64, run_id: i64, error: &str) -> AppResult<()> {
    // Meldung des Laufs: Fehlertext und die letzten Fehler-/Antworttexte
    let rows: Vec<(String,)> = sqlx::query_as("SELECT content FROM messages WHERE run_id = ? AND role IN ('error', 'assistant') ORDER BY id DESC LIMIT 3")
        .bind(run_id)
        .fetch_all(&state.db)
        .await?;
    let mut text = error.to_string();
    for (c,) in rows {
        text.push('\n');
        text.push_str(&c);
    }
    if !is_rate_limit(&text) {
        return Ok(());
    }
    let t = threads::row(state, thread_id).await?;
    let ts = now();
    // KI-Kette: ein anderes Konto übernimmt sofort (statt auf das Zurücksetzen zu warten)
    let reset = reset_from_text(&text, ts).or_else(|| reset_from_clock(&text, ts));
    if let Some(plan) = super::chains::plan(state, &t, reset, &text).await? {
        tidy_messages(state, t.user_id, thread_id, run_id).await?;
        super::chains::apply(state, &t, &plan).await?;
        // Kurz als „Pause“ zeigen – der neue Lauf beginnt gleich und hebt sie auf
        sqlx::query("UPDATE threads SET resume_at = ?, resume_attempts = 0 WHERE id = ?").bind(ts + 60).bind(thread_id).execute(&state.db).await?;
        super::chains::continue_soon(state, thread_id);
        return threads::publish(state, thread_id).await;
    }
    let attempts: i64 = sqlx::query_scalar("SELECT resume_attempts FROM threads WHERE id = ?").bind(thread_id).fetch_one(&state.db).await?;
    if attempts >= MAX_ATTEMPTS {
        let data = crate::agents::messages::jv(&SystemData::of("notice"));
        crate::agents::messages::insert(state, t.user_id, thread_id, None, "system", "Rate-Limit: automatisches Weitermachen aufgegeben", &data).await?;
        return Ok(());
    }
    let mut at = reset_from_text(&text, ts);
    if at.is_none() {
        if let Some(acc_id) = t.account_id {
            if let Ok(acc) = accounts::get_row(state, t.user_id, acc_id).await {
                let (limits, _, _, _) = super::usage::limits(state, &acc, true).await;
                // Ausgeschöpftes Limit mit dem spätesten Zurücksetzen, sonst das nächste
                at = exhausted_reset(&limits, ts);
                if at.is_none() {
                    at = limits.iter().filter_map(|l| l.resets_at).filter(|r| *r > ts).min().filter(|r| *r - ts < 6 * 3600);
                }
            }
        }
    }
    if at.is_none() {
        at = reset_from_clock(&text, ts);
    }
    // Ohne Angabe: 5, 10, 20, 40 … Minuten (höchstens eine Stunde)
    let backoff = (300i64 << attempts.min(4)).min(3600);
    let at = at.map(|a| a + 30).unwrap_or(ts + backoff).max(ts + 30);
    sqlx::query("UPDATE threads SET resume_at = ?, resume_attempts = resume_attempts + 1 WHERE id = ?")
        .bind(at)
        .bind(thread_id)
        .execute(&state.db)
        .await?;
    let detail = tidy_messages(state, t.user_id, thread_id, run_id).await?.unwrap_or_else(|| error.trim().to_string());
    let data = crate::agents::messages::jv(&SystemData { kind: "rate_limit".into(), resets_at: Some(at), detail: crate::agents::messages::clip(&detail, 500) });
    crate::agents::messages::insert(state, t.user_id, thread_id, Some(run_id), "system", "Rate-Limit erreicht", &data).await?;
    threads::publish(state, thread_id).await
}

/// Spätestes Zurücksetzen eines ausgeschöpften Limits (in der Zukunft).
fn exhausted_reset(limits: &[nrzx_shared::agents::UsageLimit], ts: i64) -> Option<i64> {
    limits.iter().filter(|l| l.used_percent >= 99.0).filter_map(|l| l.resets_at).filter(|r| *r > ts).max()
}

/// Nach einem Wechsel von Agent oder Konto: ein geplantes Weitermachen an die Limits des neuen
/// Kontos anpassen – ist es selbst am Limit, bis zu dessen Zurücksetzen, sonst gleich.
pub async fn after_switch(state: &AppState, thread_id: i64) -> AppResult<()> {
    let t = threads::row(state, thread_id).await?;
    if t.resume_at.is_none() {
        return Ok(());
    }
    let ts = now();
    let mut at = None;
    if let Some(acc_id) = t.account_id {
        if let Ok(acc) = accounts::get_row(state, t.user_id, acc_id).await {
            let (limits, _, _, _) = super::usage::limits(state, &acc, true).await;
            at = exhausted_reset(&limits, ts).map(|r| r + 30);
        }
    }
    sqlx::query("UPDATE threads SET resume_at = ?, resume_attempts = 0 WHERE id = ?").bind(at.unwrap_or(ts + 5)).bind(thread_id).execute(&state.db).await?;
    Ok(())
}

/// Die englische Meldung des CLIs steht sonst doppelt im Verlauf (Antwort und Fehler): beide
/// entfernen – die Karte „Rate-Limit erreicht“ zeigt sie übersetzt mit Countdown. Liefert den
/// Originaltext. Nur wenn sie die letzten Nachrichten des Threads sind.
async fn tidy_messages(state: &AppState, user_id: i64, thread_id: i64, run_id: i64) -> AppResult<Option<String>> {
    let rows: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, role, content FROM messages WHERE thread_id = ? ORDER BY id DESC LIMIT 4").bind(thread_id).fetch_all(&state.db).await?;
    let mut ids = Vec::new();
    let mut text = None;
    for (id, role, content) in &rows {
        if matches!(role.as_str(), "assistant" | "error") && is_rate_limit(content) {
            ids.push(*id);
            text.get_or_insert_with(|| content.trim().to_string());
        } else {
            break;
        }
    }
    let Some(from) = ids.iter().min().copied() else { return Ok(text) };
    sqlx::query("DELETE FROM messages WHERE thread_id = ? AND run_id = ? AND id >= ?").bind(thread_id).bind(run_id).bind(from).execute(&state.db).await?;
    crate::hub::publish(state, user_id, nrzx_shared::HubEvent::MessagesRemoved { thread_id, from_id: from });
    Ok(text)
}

/// Fällige Threads fortsetzen (läuft alle 20 Sekunden).
pub async fn tick(state: &AppState) -> AppResult<()> {
    let due: Vec<(i64,)> = sqlx::query_as("SELECT id FROM threads WHERE resume_at IS NOT NULL AND resume_auto = 1 AND resume_at <= ? ORDER BY resume_at LIMIT 20")
        .bind(now())
        .fetch_all(&state.db)
        .await?;
    for (id,) in due {
        if super::runtime::get(state).is_active(id) {
            sqlx::query("UPDATE threads SET resume_at = NULL WHERE id = ?").bind(id).execute(&state.db).await?;
            continue;
        }
        if let Err(e) = runs::start_with(state, id, RESUME_PROMPT, &[], true).await {
            tracing::warn!("Thread {id} nach Rate-Limit fortsetzen: {e}");
            sqlx::query("UPDATE threads SET resume_at = NULL WHERE id = ?").bind(id).execute(&state.db).await?;
            let _ = threads::publish(state, id).await;
        }
    }
    Ok(())
}

pub fn spawn_loop(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            if let Err(e) = tick(&state).await {
                tracing::warn!("Rate-Limit-Warteschlange: {e}");
            }
        }
    });
}

/// `DELETE /threads/<id>/resume` – geplantes Weitermachen abbrechen.
#[delete("/threads/<id>/resume")]
async fn cancel(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadSummary>> {
    threads::get_row(state, user.id(), id).await?;
    sqlx::query("UPDATE threads SET resume_at = NULL, resume_attempts = 0 WHERE id = ?").bind(id).execute(&state.db).await?;
    threads::publish(state, id).await?;
    Ok(Json(threads::summary(state, id).await?))
}

/// `POST /threads/<id>/resume` – jetzt weitermachen (nicht auf das Zurücksetzen warten).
#[post("/threads/<id>/resume")]
async fn resume_now(state: &State<AppState>, user: CurrentUser, _port: super::runtime::LocalPort, id: i64) -> AppResult<Json<ThreadSummary>> {
    threads::get_row(state, user.id(), id).await?;
    if super::runtime::get(state).is_active(id) {
        return Err(AppError::conflict("Der Agent arbeitet noch – bitte warten oder stoppen."));
    }
    runs::start_with(state, id, RESUME_PROMPT, &[], true).await?;
    Ok(Json(threads::summary(state, id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects() {
        assert!(is_rate_limit("Claude AI usage limit reached|1793534400"));
        assert!(is_rate_limit("You've hit your usage limit. Try again in 2 hours 5 minutes."));
        assert!(is_rate_limit("API Error: 429 Too Many Requests"));
        assert!(is_rate_limit("[RESOURCE_EXHAUSTED] Quota exceeded for model"));
        assert!(!is_rate_limit("Datei nicht gefunden"));
    }

    #[test]
    fn reset_times() {
        let now = 1_793_500_000;
        assert_eq!(reset_from_text("Claude AI usage limit reached|1793534400", now), Some(1_793_534_400));
        assert_eq!(reset_from_text("Try again in 2 hours 5 minutes.", now), Some(now + 2 * 3600 + 300));
        assert_eq!(reset_from_text("please retry after 30s", now), Some(now + 30));
        assert_eq!(reset_from_text("limit reached", now), None);
    }

    #[test]
    fn five_hour_limit() {
        let msg = "You've hit your 5-hour limit · resets 8pm (Europe/Berlin)";
        assert!(is_rate_limit(msg));
        let now = 1_793_500_000;
        let Some(at) = reset_from_clock(msg, now) else { return assert!(cfg!(not(unix))) };
        if cfg!(not(unix)) {
            return;
        }
        assert!(at > now && at <= now + 86_400);
        assert_eq!((at - local_midnight(at).unwrap()) % 86_400, 20 * 3600);
        assert!(reset_from_clock("resets at 20:30", now).is_some());
        assert!(reset_from_clock("resets 5 times", now).is_none());
    }
}
