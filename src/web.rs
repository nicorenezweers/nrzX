//! Kleine Helfer für Routen: öffentliche Basis-Adresse, Seitennummern, Namen prüfen.

use crate::error::{AppError, AppResult};
use crate::AppState;
use rocket::request::{FromRequest, Outcome, Request};

/// Öffentliche Basis-Adresse ohne `/` am Ende, z. B. `http://umbrel.local:9300`.
pub struct BaseUrl(pub String);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for BaseUrl {
    type Error = std::convert::Infallible;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        Outcome::Success(BaseUrl(base_url(req)))
    }
}

pub fn base_url(req: &Request<'_>) -> String {
    if let Some(state) = req.rocket().state::<AppState>() {
        let root = state.config.root_url.trim().trim_end_matches('/');
        if !root.is_empty() {
            return root.to_string();
        }
    }
    let headers = req.headers();
    let proto = headers
        .get_one("X-Forwarded-Proto")
        .map(|p| p.split(',').next().unwrap_or("http").trim().to_string())
        .unwrap_or_else(|| "http".into());
    let host = headers.get_one("X-Forwarded-Host").or_else(|| headers.get_one("Host")).unwrap_or("localhost");
    format!("{proto}://{}", host.split(',').next().unwrap_or(host).trim())
}

/// `?page=&limit=` mit Vorgaben (Seite 1, 30 Einträge, höchstens 200).
pub fn paging(page: Option<i64>, limit: Option<i64>) -> (i64, i64) {
    (page.unwrap_or(1).max(1), limit.unwrap_or(30).clamp(1, 200))
}

/// Benutzername prüfen: Buchstaben, Ziffern, `-`, `_`, `.`; höchstens 40 Zeichen.
pub fn check_name(name: &str, what: &str) -> AppResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with(['.', '-']);
    if ok {
        Ok(())
    } else {
        Err(AppError::bad(format!("{what}: nur Buchstaben, Ziffern, '-', '_' und '.', höchstens 40 Zeichen.")))
    }
}
