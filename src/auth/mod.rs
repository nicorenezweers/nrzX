//! Anmeldung und Request Guards. Die Oberfläche meldet sich mit dem Sitzungs-Cookie an.
//! Guards: [`CurrentUser`] (angemeldet), [`MaybeUser`] (optional), [`AdminUser`] (Administrator).
//! Schutz gegen CSRF: Das Cookie ist `SameSite=Lax`; zusätzlich werden ändernde Anfragen
//! abgewiesen, deren `Origin` nicht zum eigenen Host passt.
//!
//! Der MCP-Server von nrzX (für Subagenten) hat eigene, je Lauf erzeugte Tokens
//! (`run_tokens`, Bereich `agents`).
//!
//! Die Verbindung zu nrzXMobile (`crate::mobile`) ruft die API im selben Prozess über
//! `127.0.0.1` auf und meldet sich mit `Authorization: Bearer <Sitzungs-Token>` an – das gilt nur
//! von dieser Adresse aus.

pub mod password;
pub mod session;

use crate::db::User;
use crate::error::AppError;
use rocket::http::{Method, Status};
use rocket::request::{FromRequest, Outcome, Request};

/// Benutzer der Sitzung – je Anfrage nur einmal geladen.
async fn session_user(req: &Request<'_>) -> Option<User> {
    req.local_cache_async(async {
        let state = req.rocket().state::<crate::AppState>()?;
        let token = session::token_from(req.cookies()).or_else(|| loopback_bearer(req))?;
        session::lookup(&state.db, &token).await.ok().flatten()
    })
    .await
    .clone()
}

/// Sitzungs-Token aus `Authorization: Bearer …` – nur für Anfragen von dieser Maschine selbst.
fn loopback_bearer(req: &Request<'_>) -> Option<String> {
    let local = req.remote().is_some_and(|a| a.ip().is_loopback());
    let forwarded = req.headers().get_one("X-Forwarded-For").is_some() || req.headers().get_one("X-Real-IP").is_some();
    if !local || forwarded {
        return None;
    }
    req.headers().get_one("Authorization")?.strip_prefix("Bearer ").map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Ändernde Anfrage von einer fremden Seite?
pub fn is_cross_origin(req: &Request<'_>) -> bool {
    if matches!(req.method(), Method::Get | Method::Head | Method::Options) {
        return false;
    }
    let Some(origin) = req.headers().get_one("Origin") else {
        return false;
    };
    let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or("");
    // `dx serve` (Entwicklung der Oberfläche) leitet von :8080 weiter und schreibt Host um
    if cfg!(debug_assertions) && origin_host == "localhost:8080" {
        return false;
    }
    let own = [req.headers().get_one("X-Forwarded-Host"), req.headers().get_one("Host")];
    !own.iter().flatten().any(|h| h.eq_ignore_ascii_case(origin_host))
}

pub struct CurrentUser(pub User);

impl CurrentUser {
    pub fn id(&self) -> i64 {
        self.0.id
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for CurrentUser {
    type Error = AppError;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        if is_cross_origin(req) {
            return Outcome::Error((Status::Forbidden, AppError::forbidden("Anfrage von einer fremden Seite abgewiesen.")));
        }
        match session_user(req).await {
            Some(user) => Outcome::Success(CurrentUser(user)),
            None => Outcome::Error((Status::Unauthorized, AppError::Unauthorized)),
        }
    }
}

/// Angemeldet oder anonym (Anmeldeseite, Status).
pub struct MaybeUser(pub Option<User>);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for MaybeUser {
    type Error = std::convert::Infallible;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        Outcome::Success(MaybeUser(session_user(req).await))
    }
}

/// Administrator der Instanz.
pub struct AdminUser(pub User);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AdminUser {
    type Error = AppError;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        match CurrentUser::from_request(req).await {
            Outcome::Success(CurrentUser(u)) if u.is_admin => Outcome::Success(AdminUser(u)),
            Outcome::Success(_) => Outcome::Error((Status::Forbidden, AppError::forbidden("Nur für Administratoren."))),
            Outcome::Error(e) => Outcome::Error(e),
            Outcome::Forward(f) => Outcome::Forward(f),
        }
    }
}
