//! Deep Research: Ein Thread mit `research` recherchiert eine Frage im Netz – Plan, viele Suchen,
//! Quellen lesen und gegenprüfen – und schreibt das Ergebnis als Research Paper nach `paper.md` in
//! seinem Ordner (`research/<id>`); er legt es früh an und ergänzt es laufend, die Oberfläche zeigt
//! es live. Ein zweiter Thread (`discuss_of`) ist das Gespräch über die Recherche: er arbeitet im
//! selben Ordner und bekommt das Paper in seinen Hinweis, so dass man über das Paper reden kann.
//! Beide laufen wie andere Threads (Läufe, Nachrichten, Stoppen); nur Hinweis und Ordner sind andere.

use super::threads::{self, ThreadRow};
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::ResearchPaper;
use nrzx_shared::ThreadSummary;
use rocket::serde::json::Json;
use rocket::{get, post, routes, Route, State};
use std::path::{Path, PathBuf};

pub fn routes() -> Vec<Route> {
    routes![paper, discuss]
}

/// Datei des Papers im Ordner der Recherche.
pub const PAPER: &str = "paper.md";

/// Höchstens so viel vom Paper steht im Hinweis des Gesprächs (der Rest steht in der Datei).
const PAPER_IN_HINT: usize = 60_000;

/// Ordner einer Recherche (auch der ihres Gesprächs).
pub fn dir(state: &AppState, research_id: i64) -> PathBuf {
    state.config.data_dir.join("research").join(research_id.to_string())
}

/// Recherche eines Threads: er selbst oder die, über die er spricht.
fn research_of(t: &ThreadRow) -> Option<i64> {
    t.discuss_of.or(t.research.then_some(t.id))
}

/// Systemhinweis für Recherche und Gespräch darüber (statt des Hinweises für Coding-Agenten).
pub async fn hint(state: &AppState, t: &ThreadRow, workdir: &Path) -> Option<String> {
    if t.research {
        return Some(format!(
            "You are nrzX's Deep Research agent. cwd: {} – research the user's question thoroughly on the web and write a research paper.\n\
             Method: break the question into sub-questions and plan; search broadly (many queries, different angles and languages, recent and \
             primary sources); open and read the most relevant sources instead of relying on snippets; cross-check facts, note disagreements and \
             uncertainty; keep going until the sub-questions are answered well (typically 15–40 searches and fetches).\n\
             Write the paper in Markdown to {PAPER} in your cwd: create it right away (title, the question, planned structure) and update it after \
             every few findings, so the user can watch it grow. Structure: # Title, a short abstract, key findings as bullets, one section per \
             sub-question with evidence and numbered citations like [1], a conclusion with open questions, and a final \"Quellen\" section listing \
             every citation as \"[n] Title – URL\".\n\
             Never ask the user anything – decide yourself. Write in the user's language. When done, reply with a 2–3 sentence summary.",
            workdir.display()
        ));
    }
    let research_id = t.discuss_of?;
    let path = dir(state, research_id).join(PAPER);
    let paper = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let question: String = sqlx::query_scalar("SELECT title FROM threads WHERE id = ?").bind(research_id).fetch_optional(&state.db).await.ok().flatten().unwrap_or_default();
    let mut h = format!(
        "You discuss a research paper with the user in nrzX (research question: \"{question}\"). cwd: {} – the paper is {PAPER} there.\n\
         Answer grounded in the paper: cite its sections and its numbered sources, say clearly when something is not covered, and only search \
         the web for genuinely new follow-up questions (then say so). Keep answers concise. Change {PAPER} only when the user asks you to.\n",
        workdir.display()
    );
    if paper.trim().is_empty() {
        h.push_str("The paper is not written yet (the research may still be running) – read the file again when the user asks.\n");
    } else {
        let mut shown: String = paper.chars().take(PAPER_IN_HINT).collect();
        if shown.len() < paper.len() {
            shown.push_str("\n\n[… the paper continues – read the file for the rest]");
        }
        h.push_str("The paper as of this message (read the file again if the user says it changed):\n<paper>\n");
        h.push_str(&shown);
        h.push_str("\n</paper>\n");
    }
    h.push_str("Reply in the user's language.");
    Some(h)
}

/// `GET /threads/<id>/paper` – Research Paper der Recherche (auch über ihr Gespräch abrufbar).
#[get("/threads/<id>/paper")]
async fn paper(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ResearchPaper>> {
    let t = threads::get_row(state, user.id(), id).await?;
    let Some(research_id) = research_of(&t) else { return Err(AppError::NotFound) };
    let path = dir(state, research_id).join(PAPER);
    let markdown = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let updated_at = tokio::fs::metadata(&path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let html = if markdown.trim().is_empty() { String::new() } else { crate::markdown::render_plain(&markdown) };
    Ok(Json(ResearchPaper { research_id, markdown, html, updated_at }))
}

/// `POST /threads/<id>/discuss` – Gespräch über die Recherche (das bestehende oder ein neues mit
/// demselben Agenten).
#[post("/threads/<id>/discuss")]
async fn discuss(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Json<ThreadSummary>> {
    let t = threads::get_row(state, user.id(), id).await?;
    if !t.research {
        return Err(AppError::bad("Das ist keine Recherche."));
    }
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM threads WHERE discuss_of = ? AND user_id = ? ORDER BY id DESC LIMIT 1")
        .bind(id)
        .bind(user.id())
        .fetch_optional(&state.db)
        .await?;
    if let Some(chat) = existing {
        return Ok(Json(threads::summary(state, chat).await?));
    }
    let (account_id, model, effort) = threads::resolve_agent(state, user.id(), t.kind(), t.account_id, &t.model, &t.effort).await?;
    let title: String = format!("Diskussion: {}", t.title).chars().take(120).collect();
    let chat = threads::insert(state, user.id(), None, None, &title, t.kind(), account_id, &model, &effort, false).await?;
    sqlx::query("UPDATE threads SET discuss_of = ?, resume_auto = (SELECT auto_resume FROM users WHERE id = ?) WHERE id = ?")
        .bind(id)
        .bind(user.id())
        .bind(chat)
        .execute(&state.db)
        .await?;
    threads::publish(state, chat).await?;
    Ok(Json(threads::summary(state, chat).await?))
}
