//! Einstellungen der Instanz (Administratoren): Registrierung, Vorgabe-Adresse von nrzGit und
//! zur Anzeige Werte aus der Konfiguration (`max_parallel_runs`, Vorschau-Port, Datenordner).
//!
//! Routen: `GET/PUT /settings/instance`.

use crate::auth::AdminUser;
use crate::db;
use crate::error::AppResult;
use crate::AppState;
use nrzx_shared::workspace::InstanceSettings;
use rocket::serde::json::Json;
use rocket::{get, put, routes, Route, State};

pub fn routes() -> Vec<Route> {
    routes![get_settings, put_settings]
}

async fn current(state: &AppState) -> AppResult<InstanceSettings> {
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&state.db).await?;
    Ok(InstanceSettings {
        registration_open: super::users::registration_open(&state.db).await?,
        nrzgit_url: super::nrzgit::default_url(state).await?,
        max_parallel_runs: state.config.max_parallel_runs as i64,
        preview_port: state.config.preview_port as i64,
        data_dir: state.config.data_dir.to_string_lossy().to_string(),
        version: env!("CARGO_PKG_VERSION").into(),
        users,
    })
}

/// `GET /settings/instance`
#[get("/settings/instance")]
async fn get_settings(state: &State<AppState>, _admin: AdminUser) -> AppResult<Json<InstanceSettings>> {
    Ok(Json(current(state).await?))
}

/// `PUT /settings/instance` – Registrierung und Vorgabe-Adresse von nrzGit (leer = aus der Konfiguration).
#[put("/settings/instance", data = "<body>")]
async fn put_settings(state: &State<AppState>, _admin: AdminUser, body: Json<InstanceSettings>) -> AppResult<Json<InstanceSettings>> {
    db::set_setting(&state.db, "registration_open", if body.registration_open { "true" } else { "false" }).await?;
    let url = if body.nrzgit_url.trim().is_empty() { String::new() } else { super::nrzgit::normalize_url(&body.nrzgit_url)? };
    db::set_setting(&state.db, "nrzgit_url", &url).await?;
    Ok(Json(current(state).await?))
}
