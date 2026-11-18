//! Live-Ereignisse vom Server (`GET /api/v1/events`, Server-Sent Events) über `web_sys::EventSource`.
//! Einmal nach der Anmeldung mit [`connect`] starten; aktualisiert `state::THREADS`,
//! `PROJECTS`, `MESSAGES`, `CHANGES`, `PREVIEW` und zeigt Meldungen. Bricht die Verbindung ab,
//! verbindet der Browser selbst neu.

use crate::state;
use dioxus::prelude::*;
use crate::i18n::{tr, trf};
use nrzx_shared::{HubEvent, ThreadStatus, ThreadSummary};
use std::cell::RefCell;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

thread_local! {
    static SOURCE: RefCell<Option<(web_sys::EventSource, Closure<dyn FnMut(web_sys::MessageEvent)>)>> = const { RefCell::new(None) };
}

/// Verbindung öffnen (mehrfacher Aufruf ist harmlos).
pub fn connect() {
    if SOURCE.with(|s| s.borrow().is_some()) {
        return;
    }
    let Ok(source) = web_sys::EventSource::new("/api/v1/events") else { return };
    let handler = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        if let Some(text) = e.data().as_string() {
            if let Ok(ev) = serde_json::from_str::<HubEvent>(&text) {
                apply(ev);
            }
        }
    });
    source.set_onmessage(Some(handler.as_ref().unchecked_ref()));
    SOURCE.with(|s| *s.borrow_mut() = Some((source, handler)));
}


fn apply(ev: HubEvent) {
    match ev {
        HubEvent::ThreadUpdated { thread } => {
            let old = state::THREADS.peek().get(&thread.id).cloned();
            notify_status(old.as_ref().map(|t| t.status), &thread);
            crate::sound::on_change(old.as_ref(), &thread);
            state::upsert_thread(thread)
        }
        HubEvent::ThreadDeleted { id } => {
            state::THREADS.write().remove(&id);
            state::MESSAGES.write().remove(&id);
        }
        HubEvent::Message { message } => {
            // Thread noch nicht geladen (z. B. ein neuer Subagent): Verlauf im Hintergrund holen
            if !state::LOADED.peek().contains(&message.thread_id) && !state::MESSAGES.peek().contains_key(&message.thread_id) {
                crate::pages::agents::prefetch_messages(vec![message.thread_id]);
            }
            if message.role == "question" && message.data.get("answer").is_none_or(|a| a.is_null()) && !known(&message) {
                let title = thread_title(message.thread_id);
                crate::notify::send(message.thread_id, &trf("Frage: {0}", &[&title]), &message.content);
            }
            state::upsert_message(message)
        }
        HubEvent::MessagesRemoved { thread_id, from_id } => {
            if let Some(list) = state::MESSAGES.write().get_mut(&thread_id) {
                list.retain(|m| m.id < from_id);
            }
        }
        HubEvent::ProjectUpdated { project } => {
            state::PROJECTS.write().insert(project.id, project);
        }
        HubEvent::ProjectDeleted { id } => {
            state::PROJECTS.write().remove(&id);
        }
        HubEvent::PreviewOpened { thread_id, port, path } => *state::PREVIEW.write() = Some((thread_id, port, path)),
        HubEvent::ChangesUpdated { thread_id, additions, deletions, files } => {
            state::CHANGES.write().insert(thread_id, (additions, deletions, files));
        }
        HubEvent::TeamUpdated { project_id } => *state::TEAM_REV.write().entry(project_id).or_insert(0) += 1,
        // Hinweise des Servers: Fehler als Meldung, alles andere nur im Hintergrund als Mitteilung
        // des Systems (keine Meldung unten rechts)
        HubEvent::Notice { text, error } => {
            if error {
                crate::components::overlay::toast_error(text);
            } else {
                crate::notify::send(0, "nrzX", &text);
            }
        }
    }
}

/// Nachricht schon bekannt (Aktualisierung statt neu)?
fn known(m: &nrzx_shared::Message) -> bool {
    state::MESSAGES.peek().get(&m.thread_id).is_some_and(|l| l.iter().any(|x| x.id == m.id))
}

fn thread_title(id: i64) -> String {
    state::THREADS.peek().get(&id).map(|t| t.title.clone()).unwrap_or_else(|| format!("#{id}"))
}

/// Statuswechsel eines Haupt-Threads melden: fertig, Fehler oder Rate-Limit.
fn notify_status(before: Option<ThreadStatus>, t: &ThreadSummary) {
    if t.parent_id.is_some() || !before.is_some_and(|b| b.is_active()) {
        return;
    }
    match t.status {
        ThreadStatus::Done => crate::notify::send(t.id, &trf("Fertig: {0}", &[&t.title]), &tr("Der Agent ist fertig.")),
        ThreadStatus::Error if t.resume_at.is_some() => {
            let text = if t.resume_auto { tr("Rate-Limit erreicht – macht danach automatisch weiter.") } else { tr("Rate-Limit erreicht.") };
            crate::notify::send(t.id, &trf("Pause: {0}", &[&t.title]), &text)
        }
        ThreadStatus::Error => crate::notify::send(t.id, &trf("Fehler: {0}", &[&t.title]), &tr("Der Lauf ist mit einem Fehler beendet.")),
        _ => {}
    }
}
