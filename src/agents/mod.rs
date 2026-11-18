//! Agenten: Agent-Konten (mehrere Abos je Anbieter, je mit eigenem Home-Verzeichnis),
//! Werkzeuge (CLIs installieren/aktualisieren), Threads und Nachrichten, Läufe (CLI starten,
//! Ausgabe streamen und vereinheitlichen, stoppen), Subagenten mit eigenem MCP-Server und
//! Terminals (PTY, auch für die Anmeldung in den CLIs).
//!
//! Eingehängt in `lib.rs`: `api_routes()` unter `/api/v1`, `routes()` unter `/`.
//! `start()` läuft einmal nach dem Start.
//!
//! * [`kinds`] – Agent-Arten (Programme, Modelle, Stufen, Anmeldearten), `GET /agents`
//! * [`tools`] – CLIs und Docker CLI installieren, Programme finden
//! * [`accounts`] – Agent-Konten mit eigenem Home, Anmeldung, Status
//! * [`threads`] – Threads, Nachrichten, Wechsel von Agent/Konto/Modell
//! * [`runs`] – Läufe ausführen (Warteschlange, Prozess, Aufruf je CLI, MCP-Konfiguration)
//! * [`parse`] – Ausgabe der CLIs vereinheitlichen
//! * [`subagents`] und [`mcp`] – Subagenten, MCP-Server `POST /mcp`
//! * [`terminals`] – PTY-Terminals
//! * [`resume`] – nach einem Rate-Limit automatisch weitermachen
//! * [`chains`] – KI-Ketten: bei einem Limit mit dem nächsten Konto weitermachen, `GET/PUT /chains`
//! * [`bots`] – Bot-Modus: Bots mit Gedächtnis, Arbeitsordner und Routinen, `GET /bots`
//! * [`research`] – Deep Research: recherchieren, Research Paper schreiben, darüber reden, `GET /threads/<id>/paper`
//! * [`notes`] – Notizblock für vorbereitete Prompts, `GET /notes`
//! * [`usage`] – Nutzung der Konten (Tokens, Kosten, Limits der Abos), `GET /usage`
//! * [`attachments`], [`files`], [`skills`] – Anhänge, Dateiliste für `@`, Prompt-Bausteine

pub mod accounts;
pub mod attachments;
pub mod bots;
pub mod btw;
pub mod chains;
pub mod edits;
pub mod env;
pub mod files;
pub mod gitgateway;
pub mod kinds;
pub mod mcp;
pub mod messages;
pub mod notes;
pub mod parse;
pub mod questions;
pub mod queue;
pub mod research;
pub mod resume;
pub mod runs;
pub mod runtime;
pub mod skills;
pub mod subagents;
pub mod terminals;
pub mod threads;
pub mod tools;
pub mod usage;

use crate::error::AppResult;
use crate::AppState;
use rocket::Route;
use std::time::Duration;

/// Routen unter `/api/v1`.
pub fn api_routes() -> Vec<Route> {
    let mut r = Vec::new();
    r.extend(kinds::routes());
    r.extend(tools::routes());
    r.extend(accounts::routes());
    r.extend(threads::routes());
    r.extend(files::routes());
    r.extend(skills::routes());
    r.extend(attachments::api_routes());
    r.extend(terminals::routes());
    r.extend(usage::routes());
    r.extend(resume::routes());
    r.extend(chains::routes());
    r.extend(notes::routes());
    r.extend(questions::routes());
    r.extend(btw::routes());
    r.extend(queue::routes());
    r.extend(bots::routes());
    r.extend(research::routes());
    r
}

/// Routen unter `/` (ausserhalb der API, z. B. der MCP-Server für Subagenten).
pub fn routes() -> Vec<Route> {
    let mut r = mcp::routes();
    r.extend(attachments::routes());
    r
}

/// Hintergrundarbeiten starten (nur wenn `config.background`): hängengebliebene Läufe nach
/// einem Neustart als abgebrochen markieren, beendete Terminals wegräumen.
/// Port, auf dem dieser Server lauscht (für Aufrufe an die eigene API, z. B. von `crate::mobile`).
pub fn runtime_port(state: &AppState) -> u16 {
    runtime::get(state).port()
}

pub fn start(state: AppState) {
    tokio::spawn(async move {
        if let Err(e) = runs::recover(&state).await {
            tracing::warn!("Läufe aufräumen: {e}");
        }
        resume::spawn_loop(state.clone());
        bots::spawn_loop(state.clone());
        // Aus nrzCode übernommene Anmeldungen mit nrzCode abgleichen
        accounts::spawn_login_sync(state.clone());
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            terminals::reap(&state);
        }
    });
}

/// Alle Läufe eines Threads (und seiner Subagenten) beenden – z. B. bevor der Bereich
/// `workspace` einen Thread oder ein Projekt löscht.
pub async fn stop_thread(state: &AppState, thread_id: i64) -> AppResult<()> {
    runs::stop_tree(state, thread_id).await
}

/// Aktuellen Stand eines Threads als `HubEvent::ThreadUpdated` an seinen Benutzer senden
/// (z. B. nachdem `workspace` Branch oder Worktree geändert hat).
pub async fn publish_thread(state: &AppState, thread_id: i64) -> AppResult<()> {
    threads::publish(state, thread_id).await
}
