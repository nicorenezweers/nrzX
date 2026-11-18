//! Ereignis-Hub: Alles, was die Oberfläche live sehen soll (Thread-Status, neue Nachrichten
//! beim Streamen, Vorschau, Änderungen), geht mit [`publish`] an den Benutzer. Die Oberfläche
//! hört über `GET /api/v1/events` (Server-Sent Events) zu – ein Strom je Browser-Tab.

use crate::auth::CurrentUser;
use crate::AppState;
use nrzx_shared::HubEvent;
use rocket::response::stream::{Event, EventStream};
use rocket::tokio::select;
use rocket::tokio::sync::broadcast;
use rocket::tokio::time::{interval, Duration};
use rocket::{get, Shutdown, State};

/// Sender für (Benutzer-ID, Ereignis). Empfänger filtern nach ihrem Benutzer.
#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<(i64, HubEvent)>,
}

impl Hub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Hub { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(i64, HubEvent)> {
        self.tx.subscribe()
    }

    /// Ereignis an einen Benutzer senden (ohne Zuhörer: verworfen).
    pub fn send(&self, user_id: i64, event: HubEvent) {
        let _ = self.tx.send((user_id, event));
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

/// Kurzform: `hub::publish(state, user_id, HubEvent::…)`.
pub fn publish(state: &AppState, user_id: i64, event: HubEvent) {
    state.hub.send(user_id, event);
}

/// `GET /api/v1/events` – Server-Sent Events für den angemeldeten Benutzer.
/// Jedes Ereignis ist JSON (`HubEvent`); alle 20 s ein Kommentar, damit Proxys offen bleiben.
#[get("/events")]
pub fn events(state: &State<AppState>, user: CurrentUser, mut end: Shutdown) -> EventStream![Event + 'static] {
    let mut rx = state.hub.subscribe();
    let me = user.id();
    EventStream! {
        let mut ping = interval(Duration::from_secs(20));
        loop {
            select! {
                msg = rx.recv() => match msg {
                    Ok((uid, ev)) if uid == me => yield Event::json(&ev),
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = ping.tick() => yield Event::comment("ping"),
                _ = &mut end => break,
            }
        }
    }
}
