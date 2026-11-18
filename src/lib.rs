//! nrzX – Coding-Agenten (Claude Code, Codex, Gemini CLI, OpenCode, Kilo Code, Grok CLI)
//! im Browser, verbunden mit nrzGit. Aufbau: siehe ARCHITECTURE.md.
//!
//! Kern: `config`, `db`, `error`, `auth`, `hub` (Live-Ereignisse an die Oberfläche), `git`,
//! `markdown`, `highlight`, `web`. Bereiche, je mit eigenen Routen:
//! * `agents` – Agent-Konten, Werkzeuge, Threads, Läufe, Subagenten, Terminals
//! * `workspace` – nrzGit-Verbindung, Projekte, Worktrees, Git, Dateien, Vorschau, Benutzer
//! * `voice` – Sprachsteuerung „Dieter“ (Whisper lokal, Befehle an die Fenster des Rasters)
//! * `team` – mehrere Leute an einem Repository, ihre Agenten sprechen miteinander

pub mod agents;
pub mod auth;
pub mod team;
pub mod config;
pub mod db;
pub mod desktop;
pub mod error;
pub mod fairings;
pub mod git;
pub mod highlight;
pub mod hub;
pub mod platform;
pub mod markdown;
pub mod migrations;
pub mod mobile;
pub mod routes;
pub mod voice;
pub mod web;
pub mod workspace;

use rocket::figment::Figment;
use rocket::{Build, Rocket};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Gemeinsamer Zustand (als Rocket-State verwaltet, auch in Hintergrundaufgaben nutzbar).
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<config::Config>,
    pub hub: hub::Hub,
}

/// Baut die Rocket-Instanz mit der normalen Konfiguration (`Rocket.toml`, `NRZX_*`).
pub async fn build() -> Result<Rocket<Build>, error::AppError> {
    build_with(config::figment()).await
}

/// Baut die Rocket-Instanz aus einer gegebenen Konfiguration (auch für Tests).
pub async fn build_with(figment: Figment) -> Result<Rocket<Build>, error::AppError> {
    let config: config::Config = figment.extract().map_err(|e| error::AppError::Config(e.to_string()))?;
    for dir in [
        config.accounts_dir(),
        config.workspaces_dir(),
        config.scratch_dir(),
        config.tools_dir(),
        config.attachments_dir(),
        config.tmp_dir(),
    ] {
        std::fs::create_dir_all(dir)?;
    }
    let pool = db::connect(&config).await?;
    let figment = figment.join(("secret_key", cookie_secret(&pool).await?));
    let state = AppState { db: pool.clone(), config: Arc::new(config), hub: hub::Hub::new() };

    Ok(rocket::custom(figment)
        .manage(state)
        .manage(pool)
        .attach(fairings::RequestLog)
        // Desktop-Modus: nur Anfragen aus der App (nicht aus einem Browser auf localhost)
        .attach(desktop::AppOnly)
        // Eigene Sicherheits-Header statt Rockets Shield
        .attach(rocket::shield::Shield::new())
        .attach(fairings::SecurityHeaders)
        .attach(fairings::Background)
        .attach(fairings::CloseDatabase)
        .mount("/", routes::static_files::routes())
        .mount("/", agents::routes())
        .mount("/", workspace::routes())
        .mount("/", desktop::routes())
        .mount("/", routes::spa::routes())
        .mount("/api/v1", routes::api::routes())
        .mount("/api/v1", agents::api_routes())
        .mount("/api/v1", workspace::api_routes())
        .mount("/api/v1", voice::api_routes())
        .mount("/api/v1", team::api_routes())
        .mount("/api/v1", mobile::routes())
        .register("/", routes::catchers::all()))
}

/// Schlüssel für private Cookies: einmal erzeugt und in `settings` gespeichert.
async fn cookie_secret(db: &SqlitePool) -> Result<String, error::AppError> {
    let existing = db::setting(db, "cookie_secret", "").await?;
    if !existing.is_empty() {
        return Ok(existing);
    }
    use base64::Engine;
    let secret = base64::engine::general_purpose::STANDARD.encode((0..64).map(|_| rand::random::<u8>()).collect::<Vec<u8>>());
    db::set_setting(db, "cookie_secret", &secret).await?;
    Ok(secret)
}
