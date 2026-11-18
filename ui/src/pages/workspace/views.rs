//! Reiter oben über dem Chat-Paneel: zuerst die Chats, die nebeneinander stehen (der offene Thread
//! und die übrigen seiner Reihe – je ein Thread), dann die Ansichten des offenen Threads: ein
//! „Neuer Chat“, der integrierte Browser und beliebig viele Terminals im Arbeitsordner. „+“ öffnet
//! einen neuen Chat als Reiter (Eingabefeld für einen neuen Thread im selben Projekt); ist er
//! gesendet, steht der neue Thread als Chat in der Reihe. Ihr X schliesst Reiter (ein Chat verlässt
//! nur die Reihe, der Thread bleibt); der Globus oben im Chat öffnet den Browser, das
//! Terminal-Zeichen daneben ein Terminal. Das CLI des Agenten ist kein Reiter, sondern ein Modus des
//! Chats: der Knopf links vom Globus blendet in diesem Chat zwischen Chat und CLI um (`cli::SHOWN`).
//!
//! Alle offenen Ansichten bleiben geladen (übereinander, nur die gewählte ist sichtbar): Seiten
//! im Browser, Terminals und das CLI laufen weiter, während man woanders ist. Jeder Thread hat
//! seine eigenen; ein Klick auf einen anderen Chat der Reihe öffnet dessen Thread. Welche Chats
//! zusammen stehen, merkt sich der Browser (`nrz-chat-tabs`).

use crate::api;
use crate::i18n::tr;
use crate::state::THREADS;
use crate::util;
use crate::Route;
use dioxus::prelude::*;
use std::collections::{HashMap, HashSet};

/// Ein Reiter im Thread.
#[derive(Clone, PartialEq, Debug)]
pub enum View {
    /// Chat dieses Threads
    Chat,
    /// Anderer Chat derselben Reihe (ein Klick öffnet diesen Thread)
    Other(i64),
    /// Noch nicht gesendeter neuer Chat (`NewChatPane`)
    NewChat,
    /// CLI des Agenten selbst (`cli::NativeCli`)
    Cli,
    /// Integrierter Browser (`browser::BrowserPanel`)
    Browser,
    /// Terminal mit dieser ID (`POST /api/v1/terminals`)
    Term(String),
}

/// Gewählte Ansicht je Thread (das CLI zählt über `cli::SHOWN`).
static CHOSEN: GlobalSignal<HashMap<i64, View>> = Signal::global(HashMap::new);

/// Terminals je Thread (Reihenfolge des Öffnens).
pub static TERMINALS: GlobalSignal<HashMap<i64, Vec<String>>> = Signal::global(HashMap::new);

/// Threads mit offenem Reiter „Neuer Chat“.
static NEW_CHAT: GlobalSignal<HashSet<i64>> = Signal::global(HashSet::new);

/// Chats, die als Reiter nebeneinander stehen: je Reihe die Threads in Reiter-Reihenfolge.
static ROWS: GlobalSignal<Vec<Vec<i64>>> = Signal::global(load_rows);
const ROWS_KEY: &str = "nrz-chat-tabs";

/// Gespeichert als `1,5,9;3,4`.
fn load_rows() -> Vec<Vec<i64>> {
    util::storage_get(ROWS_KEY)
        .unwrap_or_default()
        .split(';')
        .map(|r| r.split(',').filter_map(|s| s.parse().ok()).collect::<Vec<i64>>())
        .filter(|r| r.len() > 1)
        .collect()
}

fn save_rows(rows: Vec<Vec<i64>>) {
    let rows: Vec<Vec<i64>> = rows.into_iter().filter(|r| r.len() > 1).collect();
    let text = rows.iter().map(|r| r.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join(";");
    if text.is_empty() {
        util::storage_remove(ROWS_KEY);
    } else {
        util::storage_set(ROWS_KEY, &text);
    }
    *ROWS.write() = rows;
}

/// Chats in der Reihe des Threads, er selbst eingeschlossen (gelöschte Threads fallen weg).
pub fn chats(id: i64) -> Vec<i64> {
    let threads = THREADS.read();
    match ROWS.read().iter().find(|r| r.contains(&id)) {
        Some(r) => r.iter().copied().filter(|t| *t == id || threads.contains_key(t)).collect(),
        None => vec![id],
    }
}

/// Neuen Chat `new` rechts an die Reihe von `id` hängen.
fn join(id: i64, new: i64) {
    let mut rows = ROWS.peek().clone();
    for r in rows.iter_mut() {
        r.retain(|t| *t != new);
    }
    match rows.iter_mut().find(|r| r.contains(&id)) {
        Some(r) => r.push(new),
        None => rows.push(vec![id, new]),
    }
    save_rows(rows);
}

/// Chat aus seiner Reihe nehmen (der Thread bleibt).
fn leave(t: i64) {
    let mut rows = ROWS.peek().clone();
    for r in rows.iter_mut() {
        r.retain(|x| *x != t);
    }
    save_rows(rows);
}

/// Thread öffnen (anderer Chat der Reihe).
fn open_thread(id: i64) {
    dioxus::router::router().push(Route::ThreadPage { id });
}

/// Reiter des Threads in ihrer Reihenfolge.
pub fn list(id: i64) -> Vec<View> {
    let mut v: Vec<View> = chats(id).into_iter().map(|t| if t == id { View::Chat } else { View::Other(t) }).collect();
    if NEW_CHAT.read().contains(&id) {
        v.push(View::NewChat);
    }
    if super::browser::open_for(id) {
        v.push(View::Browser);
    }
    v.extend(TERMINALS.read().get(&id).into_iter().flatten().cloned().map(View::Term));
    v
}

/// Sichtbare Ansicht (eine geschlossene zählt als Chat).
pub fn active(id: i64) -> View {
    if super::cli::SHOWN.read().contains(&id) {
        return View::Cli;
    }
    let v = CHOSEN.read().get(&id).cloned().unwrap_or(View::Chat);
    if !matches!(v, View::Chat | View::Cli | View::Other(_)) && list(id).contains(&v) {
        v
    } else {
        View::Chat
    }
}

/// Platz des sichtbaren Reiters.
pub fn active_index(id: i64) -> usize {
    let a = active(id);
    list(id).iter().position(|v| *v == a).unwrap_or(0)
}

/// Ansicht zeigen (ein anderer Chat der Reihe: dessen Thread öffnen).
pub fn activate(id: i64, v: View) {
    if let View::Other(t) = v {
        open_thread(t);
        return;
    }
    let cli_on = super::cli::SHOWN.peek().contains(&id);
    if v == View::Cli {
        if !cli_on {
            super::cli::toggle(id);
        }
        return;
    }
    if cli_on {
        super::cli::toggle(id);
    }
    CHOSEN.write().insert(id, v);
}

/// Darf der Reiter zu? Der eigene Chat nur, wenn noch ein anderer in der Reihe steht.
pub fn closable(id: i64, v: &View) -> bool {
    *v != View::Chat || chats(id).len() > 1
}

/// Reiter schliessen; war er sichtbar, kommt der links daneben. Der eigene Chat verlässt die Reihe
/// (der Nachbar wird geöffnet), ein anderer Chat ebenso – die Threads bleiben.
pub fn close(id: i64, v: View) {
    if !closable(id, &v) {
        return;
    }
    let views = list(id);
    let at = views.iter().position(|x| *x == v);
    let was_active = active(id) == v;
    match &v {
        View::Chat => {
            let row = chats(id);
            let i = row.iter().position(|t| *t == id).unwrap_or(0);
            let next = if i == 0 { row.get(1) } else { row.get(i - 1) }.copied();
            leave(id);
            if let Some(next) = next {
                open_thread(next);
            }
            return;
        }
        View::Other(t) => {
            leave(*t);
            return;
        }
        View::NewChat => {
            NEW_CHAT.write().remove(&id);
        }
        View::Cli => super::cli::close(id),
        View::Browser => super::browser::close(id),
        View::Term(t) => {
            if let Some(l) = TERMINALS.write().get_mut(&id) {
                l.retain(|x| x != t);
            }
            let t = t.clone();
            spawn(async move {
                let _ = api::delete(&format!("/terminals/{t}")).await;
            });
        }
    }
    // Links daneben – aber im Thread bleiben (statt eines anderen Chats der eigene)
    if was_active {
        let next = at.and_then(|i| views.get(i.saturating_sub(1)).cloned()).filter(|n| !matches!(n, View::Other(_))).unwrap_or(View::Chat);
        activate(id, next);
    }
}

/// „+“: neuen Chat als Reiter öffnen und zeigen – gleich mit dem Cursor im Eingabefeld.
pub fn new_chat(id: i64) {
    use wasm_bindgen::JsCast;
    NEW_CHAT.write().insert(id);
    activate(id, View::NewChat);
    // Feld des Composers (`composer-new-<slot>`), sobald es eingehängt ist
    spawn(async move {
        for _ in 0..20 {
            gloo_timers::future::TimeoutFuture::new(16).await;
            let field = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(&format!("composer-new-{id}")))
                .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok());
            if let Some(f) = field {
                let _ = f.focus();
                return;
            }
        }
    });
}

/// Der neue Chat aus dem Reiter von `id` ist gesendet: der Entwurf geht zu, der neue Thread steht
/// rechts in der Reihe und wird gezeigt.
fn chat_created(id: i64, new: i64) {
    NEW_CHAT.write().remove(&id);
    CHOSEN.write().remove(&id);
    join(id, new);
    open_thread(new);
}

/// Neues Terminal im Arbeitsordner öffnen und zeigen.
pub async fn new_terminal(id: i64) -> Result<(), String> {
    let v = api::post::<serde_json::Value, _>("/terminals", &serde_json::json!({ "threadId": id })).await.map_err(|e| e.message)?;
    let tid = match &v["id"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    if tid.is_empty() {
        return Err(tr("Das Terminal konnte nicht geöffnet werden."));
    }
    TERMINALS.write().entry(id).or_default().push(tid.clone());
    activate(id, View::Term(tid));
    Ok(())
}

/// Browser zeigen – ist er noch zu, mit `target` öffnen.
pub fn show_browser(id: i64, target: super::browser::Target) {
    if !super::browser::open_for(id) {
        super::browser::open(id, target);
    }
    activate(id, View::Browser);
}

/// Reiter „Neuer Chat“: Eingabefeld für einen neuen Thread im Projekt dieses Threads (wie die
/// Startseite, ohne Projektwahl). Gesendet, steht der neue Chat rechts in der Reihe.
#[component]
pub fn NewChatPane(thread_id: i64) -> Element {
    let id = thread_id;
    let (project_id, project_name) = THREADS.read().get(&id).map(|t| (t.project_id, t.project_name.clone())).unwrap_or_default();
    let project = project_name.unwrap_or_else(|| tr("Ohne Projekt"));
    rsx! {
        div { class: "mx-auto flex min-h-0 w-full max-w-3xl flex-1 flex-col justify-center gap-6 px-4 py-8",
            h2 { class: "text-center text-xl font-semibold tracking-tight", {tr("Neuer Chat")} }
            crate::pages::agents::Composer {
                project_id,
                thread_id: None,
                slot: id as usize,
                on_created: move |new: i64| chat_created(id, new),
                status: rsx! {
                    span { class: "flex min-w-0 items-center gap-1.5 text-[13px] text-muted-foreground",
                        icons::Folder { class: "size-3.5 shrink-0" }
                        span { class: "truncate", "{project}" }
                    }
                },
            }
        }
    }
}
