//! Globaler Zustand: angemeldeter Benutzer, Instanz-Status, Threads und Projekte (live über
//! `events.rs`), Nachrichten geöffneter Threads, Vorschau.

use dioxus::prelude::*;
use nrzx_shared::{AuthStatus, Me, Message, ProjectSummary, ThreadStatus, ThreadSummary};
use std::collections::{BTreeMap, HashMap};

/// Angemeldeter Benutzer (None = abgemeldet oder noch nicht geladen, siehe `ME_LOADED`).
pub static ME: GlobalSignal<Option<Me>> = Signal::global(|| None);
pub static ME_LOADED: GlobalSignal<bool> = Signal::global(|| false);
pub static STATUS: GlobalSignal<Option<AuthStatus>> = Signal::global(|| None);

/// Alle Threads des Benutzers (Seitenleiste), nach ID. Live aktualisiert.
pub static THREADS: GlobalSignal<BTreeMap<i64, ThreadSummary>> = Signal::global(BTreeMap::new);
/// Alle Projekte des Benutzers, nach ID. Live aktualisiert.
pub static PROJECTS: GlobalSignal<BTreeMap<i64, ProjectSummary>> = Signal::global(BTreeMap::new);
/// Nachrichten geöffneter Threads (nur Threads, deren Verlauf geladen wurde, bekommen
/// neue Nachrichten aus den Ereignissen angehängt bzw. ersetzt).
pub static MESSAGES: GlobalSignal<HashMap<i64, Vec<Message>>> = Signal::global(HashMap::new);
/// Threads, deren Verlauf vollständig geladen ist (danach live über die Ereignisse).
pub static LOADED: GlobalSignal<std::collections::HashSet<i64>> = Signal::global(std::collections::HashSet::new);
/// Änderungen je Thread: (Zusätze, Löschungen, Dateien).
pub static CHANGES: GlobalSignal<HashMap<i64, (i64, i64, i64)>> = Signal::global(HashMap::new);
/// Zuletzt von einem Agenten geöffnete Vorschau: (Thread, Port, Pfad).
pub static PREVIEW: GlobalSignal<Option<(i64, u16, String)>> = Signal::global(|| None);
/// Bots (Bot-Modus); `None` = noch nicht geladen.
pub static BOTS: GlobalSignal<Option<Vec<nrzx_shared::agents::Bot>>> = Signal::global(|| None);

/// Bots neu laden.
pub async fn load_bots() {
    if let Ok(list) = crate::api::get::<Vec<nrzx_shared::agents::Bot>>("/bots").await {
        *BOTS.write() = Some(list);
    }
}

/// Einen geänderten Bot in die Liste übernehmen.
pub fn put_bot(b: nrzx_shared::agents::Bot) {
    let mut w = BOTS.write();
    let list = w.get_or_insert_with(Vec::new);
    match list.iter_mut().find(|x| x.id == b.id) {
        Some(x) => *x = b,
        None => list.push(b),
    }
    list.sort_by_key(|x| (x.name.to_lowercase(), x.id));
}

/// Gerade gezeigter Thread (die zuletzt geöffneten bleiben eingehängt, nur einer ist zu sehen).
pub static SHOWN_THREAD: GlobalSignal<Option<i64>> = Signal::global(|| None);
/// Team je Projekt: zählt hoch, wenn der Server eine Änderung meldet (der Team-Reiter lädt dann neu).
pub static TEAM_REV: GlobalSignal<HashMap<i64, u64>> = Signal::global(HashMap::new);

/// Neuer Thread, der gerade angelegt wird: Seitenleiste und Startseite zeigen ihn sofort mit
/// der gesendeten Nachricht, bis der Server antwortet und in den echten Chat gewechselt wird.
#[derive(Clone, PartialEq)]
pub struct PendingThread {
    pub project_id: Option<i64>,
    pub title: String,
    pub text: String,
    pub attachments: Vec<nrzx_shared::Attachment>,
    pub agent: nrzx_shared::AgentKind,
    pub created_at: i64,
    /// Threads, die es beim Senden schon gab (ein neu auftauchender mit gleichem Titel ersetzt
    /// den Platzhalter in der Seitenleiste, auch wenn sein Ereignis vor der Antwort kommt)
    pub known: std::collections::HashSet<i64>,
}

pub static PENDING_THREAD: GlobalSignal<Option<PendingThread>> = Signal::global(|| None);

/// Schon in der Liste angekommener echter Thread des Platzhalters (falls ja).
pub fn pending_arrived() -> Option<i64> {
    let p = PENDING_THREAD.read().clone()?;
    THREADS.read().values().find(|t| t.parent_id.is_none() && !p.known.contains(&t.id) && t.title == p.title).map(|t| t.id)
}

/// Platzhalter noch zeigen? (Nein, sobald der echte Thread in der Liste steht.)
pub fn pending_visible() -> Option<PendingThread> {
    let p = PENDING_THREAD.read().clone()?;
    let arrived = THREADS.read().values().any(|t| t.parent_id.is_none() && !p.known.contains(&t.id) && t.title == p.title);
    (!arrived).then_some(p)
}


/// Nach 401: zur Anmeldung (ausser auf der Anmeldung selbst).
pub fn on_unauthorized() {
    let Some(window) = web_sys::window() else { return };
    let path = window.location().pathname().unwrap_or_default();
    if path.starts_with("/user/login") || path.starts_with("/install") {
        return;
    }
    *ME.write() = None;
    let next = crate::api::enc(&crate::util::current_path());
    let _ = window.location().assign(&format!("/user/login?next={next}"));
}

pub async fn load_status() -> Option<AuthStatus> {
    match crate::api::get::<AuthStatus>("/auth/status").await {
        Ok(s) => {
            *STATUS.write() = Some(s.clone());
            Some(s)
        }
        Err(_) => None,
    }
}

/// `/me` laden – ohne Anmeldung `None` (ohne Umleiten).
pub async fn load_me() -> Option<Me> {
    let res = gloo_net::http::Request::get("/api/v1/me").header("Accept", "application/json").send().await;
    let me = match res {
        Ok(r) if r.ok() => r.json::<Me>().await.ok(),
        _ => None,
    };
    *ME.write() = me.clone();
    *ME_LOADED.write() = true;
    me
}

/// Das eine Thema von nrzX: dunkel. Die App stellt ihr Fenster (und das Milchglas) darauf ein.
pub fn apply_theme() {
    crate::util::native("theme:dark");
}

/// Einstellungen öffnen (im Browser; der Rahmen navigiert). In der App öffnen sie sich in einem
/// eigenen Fenster der App (`settings:open`).
pub static OPEN_SETTINGS: GlobalSignal<bool> = Signal::global(|| false);

/// Einstellungen öffnen: in der App ein eigenes Fenster, im Browser die Seite.
pub fn open_settings() {
    if !crate::util::native("settings:open") {
        *OPEN_SETTINGS.write() = true;
    }
}

/// `window.__nrzOpenSettings()` für die App und Strg+, / ⌘, im Fenster.
pub fn install_settings_shortcut() {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let Some(w) = web_sys::window() else { return };
    let open = Closure::<dyn FnMut()>::new(open_settings);
    let _ = js_sys::Reflect::set(&w, &"__nrzOpenSettings".into(), open.as_ref().unchecked_ref());
    open.forget();
    let key = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(|e: web_sys::KeyboardEvent| {
        if e.key() == "," && (e.meta_key() || e.ctrl_key()) && !e.alt_key() && !e.shift_key() {
            e.prevent_default();
            open_settings();
        }
    });
    let _ = w.add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref());
    key.forget();
}

/// `html.nrz-glass`, wenn die App Milchglas hinter das Fenster legt – das Fenster von nrzX ist
/// immer durchsichtig (im Browser gibt es kein Milchglas).
pub fn apply_glass() {
    if !crate::util::glass_available() {
        return;
    }
    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) {
        let _ = el.class_list().add_1("nrz-glass");
    }
}

/// Ausgang einer Aufgabe für die Seitenleiste und die Töne: grün fertig (nur bis zum Ansehen), rot
/// Fehler oder abgebrochen (bleibt nach dem Lesen – Ausgang des letzten Laufs), gelb, solange der
/// Agent wirklich auf eine Antwort wartet (nicht mehr, sobald er selbst entscheidet und weitermacht);
/// laufende und abgelegte Aufgaben bleiben neutral.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tone {
    Plain,
    Done,
    Failed,
    Asking,
}

pub fn tone(t: &ThreadSummary) -> Tone {
    // Arbeitet der Agent (wieder), wartet er nicht auf den Benutzer
    if t.asking && !matches!(t.status, ThreadStatus::Settled | ThreadStatus::Working) {
        return Tone::Asking;
    }
    match t.status {
        ThreadStatus::Working | ThreadStatus::Waiting | ThreadStatus::Settled => Tone::Plain,
        ThreadStatus::Error => Tone::Failed,
        // Fertig und noch nicht angesehen; angesehen wird daraus `Idle` – dann ohne Grün
        ThreadStatus::Done => Tone::Done,
        ThreadStatus::Idle => match t.last_run.as_str() {
            "error" | "stopped" => Tone::Failed,
            _ => Tone::Plain,
        },
    }
}

/// Thread in der Liste setzen/ersetzen.
pub fn upsert_thread(t: ThreadSummary) {
    THREADS.write().insert(t.id, t);
}

/// Nachricht einfügen oder (gleiche ID, z. B. beim Streamen) ersetzen – nur für geladene Threads.
pub fn upsert_message(m: Message) {
    let mut all = MESSAGES.write();
    if let Some(list) = all.get_mut(&m.thread_id) {
        match list.iter_mut().find(|x| x.id == m.id) {
            Some(x) => *x = m,
            None => list.push(m),
        }
    }
}
