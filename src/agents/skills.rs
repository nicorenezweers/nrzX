//! Skills: gespeicherte Prompt-Bausteine je Benutzer (im Composer mit `$name` einfügen).
//! `GET/POST /agents/skills`, `PATCH/DELETE /agents/skills/<id>`.

use crate::auth::CurrentUser;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use nrzx_shared::agents::{NewSkill, Skill};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{delete, get, patch, post, routes, Route, State};

pub fn routes() -> Vec<Route> {
    routes![list, create, update, remove]
}

#[derive(sqlx::FromRow)]
struct SkillRow {
    id: i64,
    name: String,
    description: String,
    prompt: String,
    created_at: i64,
    updated_at: i64,
}

impl SkillRow {
    fn to_json(self) -> Skill {
        Skill { id: self.id, name: self.name, description: self.description, prompt: self.prompt, created_at: self.created_at, updated_at: self.updated_at }
    }
}

/// Name: Buchstaben, Ziffern, `-`, `_` (für `$name`), höchstens 40 Zeichen.
fn check(b: &NewSkill) -> AppResult<(String, String, String)> {
    let name = b.name.trim().trim_start_matches('$').to_string();
    if name.is_empty() || name.chars().count() > 40 || !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(AppError::bad("Name: nur Buchstaben, Ziffern, '-' und '_', höchstens 40 Zeichen."));
    }
    if b.prompt.trim().is_empty() {
        return Err(AppError::bad("Der Text des Skills darf nicht leer sein."));
    }
    Ok((name, b.description.trim().chars().take(200).collect(), b.prompt.trim_end().chars().take(20_000).collect()))
}

#[get("/agents/skills")]
async fn list(state: &State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<Skill>>> {
    let rows: Vec<SkillRow> = sqlx::query_as("SELECT id, name, description, prompt, created_at, updated_at FROM agent_skills WHERE user_id = ? ORDER BY name COLLATE NOCASE")
        .bind(user.id())
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows.into_iter().map(SkillRow::to_json).collect()))
}

#[post("/agents/skills", data = "<body>")]
async fn create(state: &State<AppState>, user: CurrentUser, body: Json<NewSkill>) -> AppResult<Json<Skill>> {
    let (name, description, prompt) = check(&body)?;
    let ts = now();
    let row: SkillRow = sqlx::query_as(
        "INSERT INTO agent_skills (user_id, name, description, prompt, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) \
         RETURNING id, name, description, prompt, created_at, updated_at",
    )
    .bind(user.id())
    .bind(&name)
    .bind(&description)
    .bind(&prompt)
    .bind(ts)
    .bind(ts)
    .fetch_one(&state.db)
    .await
    .map_err(|e| AppError::unique(e, "Ein Skill mit diesem Namen existiert bereits."))?;
    Ok(Json(row.to_json()))
}

#[patch("/agents/skills/<id>", data = "<body>")]
async fn update(state: &State<AppState>, user: CurrentUser, id: i64, body: Json<NewSkill>) -> AppResult<Json<Skill>> {
    let (name, description, prompt) = check(&body)?;
    let row: Option<SkillRow> = sqlx::query_as(
        "UPDATE agent_skills SET name = ?, description = ?, prompt = ?, updated_at = ? WHERE id = ? AND user_id = ? \
         RETURNING id, name, description, prompt, created_at, updated_at",
    )
    .bind(&name)
    .bind(&description)
    .bind(&prompt)
    .bind(now())
    .bind(id)
    .bind(user.id())
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::unique(e, "Ein Skill mit diesem Namen existiert bereits."))?;
    Ok(Json(row.ok_or(AppError::NotFound)?.to_json()))
}

#[delete("/agents/skills/<id>")]
async fn remove(state: &State<AppState>, user: CurrentUser, id: i64) -> AppResult<Status> {
    let r = sqlx::query("DELETE FROM agent_skills WHERE id = ? AND user_id = ?").bind(id).bind(user.id()).execute(&state.db).await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Status::NoContent)
}
