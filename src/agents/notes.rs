//! Notizblock: vorbereitete Prompts und Ideen je Benutzer (optional einem Projekt zugeordnet).
//! Sie gehen erst in ein Gespräch, wenn man sie im Composer übernimmt bzw. sendet.
//! Bilder hängen über das Anhang-System des Composers an (`POST /attachments`); die Notiz merkt
//! sich nur ihre UUIDs.

use super::attachments::{self, AttachmentRow};
use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{NoteInput, PromptNote};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{delete, get, patch, post, routes, Route, State};

pub fn routes() -> Vec<Route> {
    routes![list, create, update, remove]
}

const MAX_LEN: usize = 50_000;
/// Höchstzahl Bilder je Notiz
const MAX_ATTACHMENTS: usize = 20;

#[derive(sqlx::FromRow)]
struct NoteRow {
    id: i64,
    project_id: Option<i64>,
    text: String,
    attachments: String,
    created_at: i64,
    updated_at: i64,
}

/// Notiz mit ihren Anhängen (inzwischen gelöschte Anhänge fallen weg).
async fn to_json(state: &AppState, user_id: i64, r: NoteRow) -> AppResult<PromptNote> {
    let uuids: Vec<String> = serde_json::from_str(&r.attachments).unwrap_or_default();
    let mut attachments = Vec::new();
    for u in uuids {
        let row: Option<AttachmentRow> = sqlx::query_as("SELECT * FROM attachments WHERE uuid = ? AND user_id = ?")
            .bind(&u)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
        attachments.extend(row.map(|a| a.to_json()));
    }
    Ok(PromptNote { id: r.id, project_id: r.project_id, text: r.text, attachments, created_at: r.created_at, updated_at: r.updated_at })
}

async fn get(state: &AppState, user_id: i64, id: i64) -> AppResult<PromptNote> {
    let r: NoteRow = sqlx::query_as("SELECT id, project_id, text, attachments, created_at, updated_at FROM prompt_notes WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    to_json(state, user_id, r).await
}

/// Anhänge prüfen (gehören dem Benutzer, nicht zu viele) und als JSON-Liste ablegen.
async fn clean_attachments(state: &AppState, user_id: i64, uuids: &[String]) -> AppResult<String> {
    if uuids.len() > MAX_ATTACHMENTS {
        return Err(AppError::bad("Zu viele Bilder an der Notiz."));
    }
    let rows = attachments::load(state, user_id, uuids).await?;
    let mut list: Vec<String> = Vec::new();
    for r in rows {
        if !list.contains(&r.uuid) {
            list.push(r.uuid);
        }
    }
    Ok(serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()))
}

async fn check_project(state: &AppState, user_id: i64, project_id: i64) -> AppResult<()> {
    let ok: Option<(i64,)> = sqlx::query_as("SELECT id FROM projects WHERE id = ? AND user_id = ?").bind(project_id).bind(user_id).fetch_optional(&state.db).await?;
    ok.map(|_| ()).ok_or(AppError::NotFound)
}

fn clean(text: &str) -> AppResult<String> {
    if text.len() > MAX_LEN {
        return Err(AppError::bad("Die Notiz ist zu lang."));
    }
    Ok(text.to_string())
}

/// `GET /notes` – neueste zuerst.
#[get("/notes")]
async fn list(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<PromptNote>>> {
    let rows: Vec<NoteRow> = sqlx::query_as("SELECT id, project_id, text, attachments, created_at, updated_at FROM prompt_notes WHERE user_id = ? ORDER BY updated_at DESC, id DESC LIMIT 500")
        .bind(user.id())
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(to_json(state, user.id(), r).await?);
    }
    Ok(Json(out))
}

/// `POST /notes`
#[post("/notes", data = "<body>")]
async fn create(state: &State<AppState>, user: CurrentUser, body: Json<NoteInput>) -> AppResult<Json<PromptNote>> {
    let text = clean(body.text.as_deref().unwrap_or(""))?;
    if let Some(p) = body.project_id {
        check_project(state, user.id(), p).await?;
    }
    let atts = clean_attachments(state, user.id(), body.attachments.as_deref().unwrap_or_default()).await?;
    let ts = now();
    let id: i64 = sqlx::query_scalar("INSERT INTO prompt_notes (user_id, project_id, text, attachments, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) RETURNING id")
        .bind(user.id())
        .bind(body.project_id)
        .bind(text)
        .bind(atts)
        .bind(ts)
        .bind(ts)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(get(state, user.id(), id).await?))
}

/// `PATCH /notes/<id>`
#[patch("/notes/<id>", data = "<body>")]
async fn update(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<NoteInput>) -> AppResult<Json<PromptNote>> {
    get(state, user.id(), id).await?;
    let ts = now();
    if let Some(t) = &body.text {
        sqlx::query("UPDATE prompt_notes SET text = ?, updated_at = ? WHERE id = ?").bind(clean(t)?).bind(ts).bind(id).execute(&state.db).await?;
    }
    if let Some(list) = &body.attachments {
        let atts = clean_attachments(state, user.id(), list).await?;
        sqlx::query("UPDATE prompt_notes SET attachments = ?, updated_at = ? WHERE id = ?").bind(atts).bind(ts).bind(id).execute(&state.db).await?;
    }
    if body.clear_project {
        sqlx::query("UPDATE prompt_notes SET project_id = NULL WHERE id = ?").bind(id).execute(&state.db).await?;
    } else if let Some(p) = body.project_id {
        check_project(state, user.id(), p).await?;
        sqlx::query("UPDATE prompt_notes SET project_id = ? WHERE id = ?").bind(p).bind(id).execute(&state.db).await?;
    }
    Ok(Json(get(state, user.id(), id).await?))
}

/// `DELETE /notes/<id>`
#[delete("/notes/<id>")]
async fn remove(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Status> {
    get(state, user.id(), id).await?;
    sqlx::query("DELETE FROM prompt_notes WHERE id = ?").bind(id).execute(&state.db).await?;
    Ok(Status::NoContent)
}
