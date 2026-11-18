//! Desktop-Modus (macOS-App): Die App startet den Server mit einem zufälligen Token
//! (`NRZX_DESKTOP_TOKEN`, bei jedem Start neu) und öffnet `GET /__desktop/login?token=…`.
//! Das meldet ohne Passwort an – beim allerersten Start wird dafür ein lokales
//! Administratorkonto angelegt (Name aus `NRZX_DESKTOP_USER`, Passwort zufällig).
//!
//! Ohne gesetztes Token gibt es die Route nicht (404). Im Browser gilt weiter die normale
//! Anmeldung.
//!
//! Nur in der App ([`AppOnly`]): Mit Token beantwortet der Server nur Anfragen aus dem WebView der
//! App. Die App hängt an ihren User-Agent das Merkmal `nrzXApp/<marker>` (aus dem Token
//! abgeleitet, bei jedem Start neu). Ein Browser auf `http://127.0.0.1:<port>` bekommt nur eine
//! Hinweisseite – nrzX lebt in der App. Ausgenommen ist `/mcp` (eigene Tokens je Lauf, für die
//! Agenten-CLIs).

use crate::auth::session;
use crate::db::{self, User};
use crate::error::{AppError, AppResult};
use crate::AppState;
use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::uri::Origin;
use rocket::http::{CookieJar, Method, Status};
use rocket::response::content::RawHtml;
use rocket::response::Redirect;
use rocket::{get, routes, Data, Request, Route, State};
use sha2::{Digest, Sha256};

pub fn routes() -> Vec<Route> {
    routes![login, blocked]
}

/// Merkmal im User-Agent der App (aus dem Token abgeleitet – das Token selbst bleibt geheim).
/// Gleiche Rechnung in macos/src/server.rs (`app_marker`).
pub fn app_marker(token: &str) -> String {
    let hash = Sha256::digest(format!("nrzx-app:{token}").as_bytes());
    hex::encode(&hash[..16])
}

/// Fairing: im Desktop-Modus nur Anfragen aus der App zulassen (siehe Moduldoku).
pub struct AppOnly;

#[rocket::async_trait]
impl Fairing for AppOnly {
    fn info(&self) -> Info {
        Info { name: "Nur aus der App", kind: Kind::Request }
    }

    async fn on_request(&self, req: &mut Request<'_>, _: &mut Data<'_>) {
        let Some(state) = req.rocket().state::<AppState>() else { return };
        let token = &state.config.desktop_token;
        if token.len() < 32 {
            return;
        }
        let path = req.uri().path();
        if path == "/mcp" || path.starts_with("/mcp/") || path == "/__desktop/blocked" {
            return;
        }
        let want = format!("nrzXApp/{}", app_marker(token));
        let ok = req.headers().get_one("User-Agent").is_some_and(|ua| ua.split_whitespace().any(|p| p == want));
        if !ok {
            req.set_method(Method::Get);
            req.set_uri(Origin::parse("/__desktop/blocked").expect("feste Adresse"));
        }
    }
}

/// Hinweis für Browser, wenn nrzX in der App läuft.
#[get("/__desktop/blocked")]
fn blocked() -> (Status, RawHtml<&'static str>) {
    (
        Status::Forbidden,
        RawHtml(
            r#"<!doctype html><html><head><meta charset="utf-8"><title>nrzX</title><style>
:root{color-scheme:light dark}html,body{margin:0;height:100%;font:14px -apple-system,BlinkMacSystemFont,"Segoe UI",Inter,sans-serif;background:#18181b;color:#e4e4e7}
.w{height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:8px;text-align:center;padding:0 24px}
p{margin:0;color:#a1a1aa;max-width:420px;line-height:1.5}</style></head>
<body><div class="w"><strong>nrzX läuft in der App</strong><p>Diese Adresse ist nur für die nrzX-App da und lässt sich nicht im Browser öffnen. Öffne nrzX über das Programm.</p></div></body></html>"#,
        ),
    )
}

#[get("/__desktop/login?<token>")]
async fn login(state: &State<AppState>, cookies: &CookieJar<'_>, token: &str) -> AppResult<Redirect> {
    let expected = &state.config.desktop_token;
    // Vergleich über die Hashes: gleich lang, keine frühe Rückkehr nach dem ersten Unterschied
    if expected.len() < 32 || Sha256::digest(token.as_bytes()) != Sha256::digest(expected.as_bytes()) {
        return Err(AppError::NotFound);
    }
    let user = desktop_user(state).await?;
    let token = session::create(&state.db, user.id, "nrzX für macOS").await?;
    session::set_cookie(cookies, token, false);
    Ok(Redirect::to("/"))
}

/// Erster aktiver Administrator – oder ein neues lokales Konto, wenn es noch keines gibt.
async fn desktop_user(state: &AppState) -> AppResult<User> {
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE is_active = 1 ORDER BY is_admin DESC, id ASC LIMIT 1")
            .fetch_optional(&state.db)
            .await?;
    if let Some(id) = existing {
        return db::user_by_id(&state.db, id).await?.ok_or(AppError::NotFound);
    }
    let name = clean_name(&state.config.desktop_user);
    let password = session::new_token();
    let user = crate::workspace::users::create_account(
        &state.db,
        &name,
        &format!("{name}@nrzx.local"),
        &password,
        state.config.desktop_full_name.trim(),
        Some(true),
    )
    .await?;
    tracing::info!("Desktop: lokales Konto {name} angelegt");
    Ok(user)
}

/// macOS-Kurzname in einen gültigen Benutzernamen umwandeln.
fn clean_name(raw: &str) -> String {
    let mut name: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    name = name.trim_start_matches(['.', '-']).chars().take(40).collect();
    if name.is_empty() {
        "nrzx".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::clean_name;

    #[test]
    fn names() {
        assert_eq!(clean_name("nico"), "nico");
        assert_eq!(clean_name(".Nico René"), "Nico-Ren-");
        assert_eq!(clean_name(""), "nrzx");
    }
}
