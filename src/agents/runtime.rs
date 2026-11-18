//! Laufzeit-Zustand des Bereichs je Instanz (nicht in der Datenbank): laufende Agenten,
//! Warteschlange (Semaphore mit `config.max_parallel_runs` Plätzen), offene Terminals,
//! Installationen und der eigene Port (für die MCP-Adresse der Läufe).
//!
//! Der Zustand hängt am Datenordner der Instanz – so bleiben mehrere Instanzen in einem
//! Prozess (Integrationstests) getrennt, ohne `AppState` zu erweitern.

use crate::AppState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::{broadcast, watch, OwnedSemaphorePermit, Semaphore};

/// Ein laufender (oder wartender) Lauf eines Threads.
pub struct ActiveRun {
    pub run_id: i64,
    /// `true` senden = stoppen
    pub stop: watch::Sender<bool>,
    /// Zwischenruf (ohne „Gestoppt“): 1 = im nächsten passenden Moment beenden, 2 = sofort
    pub soft: watch::Sender<u8>,
    /// Platz in der Warteschlange – wird abgegeben, solange der Agent auf Subagenten wartet
    pub permit: Arc<tokio::sync::Mutex<Option<OwnedSemaphorePermit>>>,
}

pub struct Runtime {
    pub permits: Arc<Semaphore>,
    /// Thread-ID → aktiver Lauf
    pub active: Mutex<HashMap<i64, ActiveRun>>,
    pub terminals: Mutex<HashMap<String, Arc<super::terminals::Terminal>>>,
    pub jobs: Mutex<HashMap<String, Arc<super::tools::Job>>>,
    /// Neueste Versionen der Werkzeuge (nach `GET /tools/latest`)
    pub latest: Mutex<HashMap<String, String>>,
    /// Zwischengespeicherte Versionen: Programmpfad → (Zeitpunkt, Version)
    pub versions: Mutex<HashMap<PathBuf, (std::time::Instant, String)>>,
    /// Thread-ID, deren Lauf gerade endete (für `wait_for_subagents`)
    pub finished: broadcast::Sender<i64>,
    port: AtomicU16,
}

static RUNTIMES: LazyLock<Mutex<HashMap<PathBuf, Arc<Runtime>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Zustand der Instanz (beim ersten Aufruf angelegt).
pub fn get(state: &AppState) -> Arc<Runtime> {
    let key = state.config.data_dir.clone();
    let mut all = RUNTIMES.lock().unwrap_or_else(|e| e.into_inner());
    all.entry(key)
        .or_insert_with(|| {
            let port = crate::config::figment().extract_inner::<u16>("port").unwrap_or(9300);
            Arc::new(Runtime {
                permits: Arc::new(Semaphore::new(state.config.max_parallel_runs.max(1))),
                active: Mutex::new(HashMap::new()),
                terminals: Mutex::new(HashMap::new()),
                jobs: Mutex::new(HashMap::new()),
                latest: Mutex::new(HashMap::new()),
                versions: Mutex::new(HashMap::new()),
                finished: broadcast::channel(256).0,
                port: AtomicU16::new(port),
            })
        })
        .clone()
}

impl Runtime {
    /// Port des eigenen Servers (für `http://127.0.0.1:<port>/mcp`).
    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }

    pub fn set_port(&self, port: u16) {
        if port != 0 {
            self.port.store(port, Ordering::Relaxed);
        }
    }

    pub fn is_active(&self, thread_id: i64) -> bool {
        self.active.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&thread_id)
    }

    pub fn active_run(&self, thread_id: i64) -> Option<i64> {
        self.active.lock().unwrap_or_else(|e| e.into_inner()).get(&thread_id).map(|a| a.run_id)
    }

    pub fn permit_slot(&self, thread_id: i64) -> Option<Arc<tokio::sync::Mutex<Option<OwnedSemaphorePermit>>>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner()).get(&thread_id).map(|a| a.permit.clone())
    }
}

/// Request-Guard: merkt sich den Port, auf dem Rocket läuft (für die MCP-Adresse der Läufe).
pub struct LocalPort;

#[rocket::async_trait]
impl<'r> rocket::request::FromRequest<'r> for LocalPort {
    type Error = std::convert::Infallible;

    async fn from_request(req: &'r rocket::Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
        if let Some(state) = req.rocket().state::<AppState>() {
            get(state).set_port(req.rocket().config().port);
        }
        rocket::request::Outcome::Success(LocalPort)
    }
}
