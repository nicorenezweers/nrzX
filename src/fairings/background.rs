//! Startet die Hintergrundarbeiten, sobald der Server läuft (nicht in Tests, `background = false`):
//! abgelaufene Sitzungen löschen und die Aufgaben der Bereiche (Vorschau-Proxy, Aufräumen …).

use crate::AppState;
use rocket::fairing::{Fairing, Info, Kind};
use rocket::{Orbit, Rocket};
use std::time::Duration;

pub struct Background;

#[rocket::async_trait]
impl Fairing for Background {
    fn info(&self) -> Info {
        Info { name: "Hintergrundarbeiten", kind: Kind::Liftoff }
    }

    async fn on_liftoff(&self, rocket: &Rocket<Orbit>) {
        let Some(state) = rocket.state::<AppState>().cloned() else { return };
        if !state.config.background {
            return;
        }
        let db = state.db.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = crate::auth::session::purge_expired(&db).await {
                    tracing::warn!("Sitzungen aufräumen: {e}");
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
        crate::agents::start(state.clone());
        tokio::spawn(crate::mobile::start_all(state.clone()));
        crate::team::start(state.clone());
        crate::workspace::start(state);
    }
}
