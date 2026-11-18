//! Beim Herunterfahren die Datenbank sauber schliessen: laufende Abfragen beenden, WAL
//! zurückschreiben, Verbindungs-Threads stoppen.

use rocket::fairing::{Fairing, Info, Kind};
use rocket::{Orbit, Rocket};
use sqlx::SqlitePool;

pub struct CloseDatabase;

#[rocket::async_trait]
impl Fairing for CloseDatabase {
    fn info(&self) -> Info {
        Info {
            name: "Datenbank schliessen",
            kind: Kind::Shutdown,
        }
    }

    async fn on_shutdown(&self, rocket: &Rocket<Orbit>) {
        if let Some(pool) = rocket.state::<SqlitePool>() {
            pool.close().await;
            tracing::info!("Datenbank geschlossen");
        }
    }
}
