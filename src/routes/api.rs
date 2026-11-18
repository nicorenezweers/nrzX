//! Allgemeine API: `GET /health`, `POST /markdown` (Vorschau), `GET /events` (Live-Ereignisse),
//! `POST /client/error` (Absturz der Oberfläche ins Protokoll).

use crate::auth::CurrentUser;
use crate::markdown;
use nrzx_shared::{Health, MarkdownRequest, MarkdownResponse};
use rocket::serde::json::Json;
use rocket::{get, post, routes, Route};

pub fn routes() -> Vec<Route> {
    routes![health, render_markdown, client_error, crate::hub::events]
}

#[get("/health")]
fn health() -> Json<Health> {
    Json(Health { status: "ok".into(), name: "nrzX".into(), version: env!("CARGO_PKG_VERSION").into() })
}

#[post("/markdown", data = "<body>")]
fn render_markdown(_user: CurrentUser, body: Json<MarkdownRequest>) -> Json<MarkdownResponse> {
    Json(MarkdownResponse { html: markdown::render_plain(&body.text) })
}

/// Absturz der Oberfläche (Rust-Panic im WebView, per `navigator.sendBeacon`): landet im Protokoll
/// (`~/Library/Logs/nrzX/nrzx.log`), damit sich jeder Absturz finden und beheben lässt.
#[post("/client/error", data = "<body>")]
fn client_error(user: CurrentUser, body: String) -> rocket::http::Status {
    let text: String = body.chars().take(4000).collect();
    tracing::error!(user = user.id(), "Oberfläche abgestürzt: {text}");
    rocket::http::Status::NoContent
}
