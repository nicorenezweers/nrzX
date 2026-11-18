//! Natives CLI eines Threads: Der Knopf links vom Globus oben rechts ([`CliToggle`]) zeigt in diesem
//! Chat statt der Oberfläche das CLI des Agenten selbst (Claude Code, Codex …) – im Arbeitsordner des Threads, mit
//! dessen Konto, Modell und Sitzung (`POST /api/v1/threads/<id>/cli`), eingefärbt wie die App und
//! im Milchglas durchscheinend.
//!
//! Schneller Wechsel: Chat und CLI liegen übereinander und bleiben beide geladen, der Knopf blendet
//! nur um – das CLI läuft weiter, während der Chat zu sehen ist. Es startet schon vorher im
//! Hintergrund: wenn die Maus über den Knopf kommt, beim Öffnen eines Threads, in dem es schon benutzt
//! wurde (im Browser gemerkt), und nach jedem Lauf neu. Startet ein Lauf des Threads, schliesst der
//! Server das CLI (nur ein Programm schreibt in die Sitzung, Exit-Code −2); danach startet es mit
//! dem neuen Stand der Sitzung. Beendet man es selbst (`/exit`), geht es zurück zum Chat.

use crate::api;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::state::THREADS;
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::agents::{TerminalInfo, TerminalSize};
use nrzx_shared::ThreadStatus;
use std::collections::{HashMap, HashSet};

/// Exit-Code eines CLIs, das der Server wegen eines Laufs geschlossen hat (`terminals::EXIT_FOR_RUN`).
const EXIT_FOR_RUN: i32 = -2;
/// Gemerkte Threads mit CLI (Browser).
const USED_KEY: &str = "nrz-cli-threads";

/// Stand des CLIs eines Threads.
#[derive(Clone, PartialEq, Debug)]
pub enum Cli {
    /// Wird gestartet
    Starting,
    /// Läuft im Terminal mit dieser ID
    Open(String),
    /// Ein Lauf arbeitet – danach starten
    Waiting,
}

/// CLI je Thread (läuft weiter, während der Chat zu sehen ist).
pub static CLI: GlobalSignal<HashMap<i64, Cli>> = Signal::global(HashMap::new);
/// Threads, die gerade das CLI statt des Chats zeigen.
pub static SHOWN: GlobalSignal<HashSet<i64>> = Signal::global(HashSet::new);
/// Threads, in denen das CLI benutzt wurde: dort startet es beim Öffnen schon im Hintergrund.
static USED: GlobalSignal<HashSet<i64>> = Signal::global(|| {
    util::storage_get(USED_KEY).and_then(|s| serde_json::from_str::<Vec<i64>>(&s).ok()).unwrap_or_default().into_iter().collect()
});
/// Maus über dem CLI-Knopf: schon starten (nicht gemerkt).
static WARM: GlobalSignal<HashSet<i64>> = Signal::global(HashSet::new);

/// Soll das CLI des Threads geladen sein (sichtbar oder im Hintergrund)?
pub fn wanted(thread_id: i64) -> bool {
    SHOWN.read().contains(&thread_id) || USED.read().contains(&thread_id) || WARM.read().contains(&thread_id)
}

fn save_used() {
    let mut ids: Vec<i64> = USED.peek().iter().copied().collect();
    ids.sort_unstable();
    // Die neuesten Threads (höhere IDs) behalten
    let keep = ids.split_off(ids.len().saturating_sub(24));
    util::storage_set(USED_KEY, &serde_json::to_string(&keep).unwrap_or_default());
}

/// CLI zeigen bzw. zurück zum Chat – nur umblenden, das CLI läuft weiter.
pub fn toggle(thread_id: i64) {
    if SHOWN.peek().contains(&thread_id) {
        SHOWN.write().remove(&thread_id);
        return;
    }
    SHOWN.write().insert(thread_id);
    if USED.write().insert(thread_id) {
        save_used();
    }
}

/// Reiter des CLIs geschlossen: CLI beenden und vergessen.
pub fn close(thread_id: i64) {
    if let Some(Cli::Open(t)) = CLI.peek().get(&thread_id).cloned() {
        spawn(async move {
            let _ = api::delete(&format!("/terminals/{t}")).await;
        });
    }
    forget(thread_id);
}

/// CLI vergessen (selbst beendet oder Start gescheitert): kein Neustart, zurück zum Chat.
fn forget(thread_id: i64) {
    CLI.write().remove(&thread_id);
    SHOWN.write().remove(&thread_id);
    WARM.write().remove(&thread_id);
    if USED.write().remove(&thread_id) {
        save_used();
    }
}

/// CLI im Hintergrund starten (Grösse in Zeichen).
fn start(thread_id: i64, size: (u16, u16)) {
    CLI.write().insert(thread_id, Cli::Starting);
    spawn(async move {
        let body = TerminalSize { cols: size.0, rows: size.1 };
        let mut tries = 0;
        loop {
            match api::post::<TerminalInfo, _>(&format!("/threads/{thread_id}/cli"), &body).await {
                Ok(t) => {
                    if CLI.peek().get(&thread_id) == Some(&Cli::Starting) {
                        CLI.write().insert(thread_id, Cli::Open(t.id));
                    } else {
                        // Inzwischen vergessen: gleich wieder schliessen
                        let _ = api::delete(&format!("/terminals/{}", t.id)).await;
                    }
                    return;
                }
                // Ein Lauf arbeitet (409): danach starten. Kennt die Oberfläche ihn noch nicht,
                // höchstens fünfmal alle 2 s erneut fragen
                Err(e) if e.status == 409 && tries < 5 => {
                    if running(thread_id) {
                        CLI.write().insert(thread_id, Cli::Waiting);
                        return;
                    }
                    tries += 1;
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    if CLI.peek().get(&thread_id) != Some(&Cli::Starting) {
                        return;
                    }
                }
                Err(e) => {
                    let shown = SHOWN.peek().contains(&thread_id);
                    forget(thread_id);
                    if shown {
                        toast_error(e);
                    }
                    return;
                }
            }
        }
    });
}

/// Arbeitet der Agent des Threads gerade?
fn running(thread_id: i64) -> bool {
    THREADS.peek().get(&thread_id).is_some_and(|t| t.status == ThreadStatus::Working)
}

/// Knopf links vom Globus: dieser Chat zeigt das CLI des Agenten statt der Oberfläche (und zurück) –
/// nur für diesen Chat, ohne eigenen Reiter. Schon die Maus darüber startet das CLI im Hintergrund.
#[component]
pub fn CliToggle(thread_id: i64) -> Element {
    let id = thread_id;
    let on = SHOWN.read().contains(&id);
    let agent = THREADS.read().get(&id).map(|t| t.agent.name().to_string()).unwrap_or_default();
    let title = if on { tr("Zurück zum Chat") } else if agent.is_empty() { tr("CLI statt des Chats") } else { trf("{0} CLI statt des Chats", &[&agent]) };
    rsx! {
        button {
            class: if on { "grid size-8 place-items-center rounded-lg bg-accent text-foreground transition-colors" } else { "grid size-8 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-white/[0.09] hover:text-foreground" },
            title: "{title}",
            "aria-pressed": if on { "true" } else { "false" },
            onmouseenter: move |_| {
                if !wanted(id) {
                    WARM.write().insert(id);
                }
            },
            onclick: move |_| toggle(id),
            icons::Terminal { class: "size-4" }
        }
    }
}

/// Das CLI des Agenten (über dem Chat, unsichtbar solange `visible` nicht gesetzt ist): startet,
/// sobald die Fläche gemessen ist und kein Lauf arbeitet, und nach jedem Lauf neu.
#[component]
pub fn NativeCli(thread_id: i64, visible: bool) -> Element {
    let entry = CLI.read().get(&thread_id).cloned();
    let working = THREADS.read().get(&thread_id).is_some_and(|t| t.status == ThreadStatus::Working);
    let agent = THREADS.read().get(&thread_id).map(|t| t.agent.name().to_string()).unwrap_or_default();
    let mut size = use_signal(|| None::<(u16, u16)>);
    // Starten bzw. nach dem Lauf neu starten
    use_effect(move || {
        let entry = CLI.read().get(&thread_id).cloned();
        let working = THREADS.read().get(&thread_id).is_some_and(|t| t.status == ThreadStatus::Working);
        let Some(sz) = size() else { return };
        match entry {
            None | Some(Cli::Waiting) if !working => start(thread_id, sz),
            None if working => {
                CLI.write().insert(thread_id, Cli::Waiting);
            }
            _ => {}
        }
    });
    // Sichtbar: Eingabe ins Terminal; unsichtbar: Fokus dort weg (Tippen gehört dann dem Chat)
    let open_id = match &entry {
        Some(Cli::Open(t)) => Some(t.clone()),
        _ => None,
    };
    use_effect(use_reactive!(|visible, open_id| {
        let Some(term) = open_id else { return };
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        let Some(el) = doc.get_element_by_id(&format!("term-input-{term}")) else { return };
        use wasm_bindgen::JsCast;
        let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() else { return };
        // Fokus im CLI, auf dem CLI-Knopf (der Klick zum Umblenden) oder nirgends: weiter im Chat
        let from_cli = doc.active_element().is_none_or(|a| {
            a.id() == el.id() || a.tag_name() == "BODY" || a.closest("button[aria-pressed]").ok().flatten().is_some()
        });
        if visible {
            let _ = el.focus();
        } else if from_cli {
            let _ = el.blur();
            // Weiter im Composer des Chats
            if let Ok(Some(c)) = doc.query_selector(&format!("#chat-layer-{thread_id} textarea")) {
                if let Ok(c) = c.dyn_into::<web_sys::HtmlElement>() {
                    let _ = c.focus();
                }
            }
        }
    }));
    rsx! {
        div {
            class: "h-full min-h-0",
            // Grösse in Zeichen wie im Terminal (12,5 px Mono, Rand px-4 py-3)
            onresize: move |e| {
                if let Ok(s) = e.get_content_box_size() {
                    if s.width >= 80.0 && s.height >= 80.0 {
                        let cols = ((s.width - 48.0) / 7.8).floor().clamp(20.0, 500.0) as u16;
                        let rows = ((s.height - 40.0) / 17.0).floor().clamp(5.0, 300.0) as u16;
                        if *size.peek() != Some((cols, rows)) {
                            size.set(Some((cols, rows)));
                        }
                    }
                }
            },
            match entry {
                Some(Cli::Open(term)) => {
                    let mine = Cli::Open(term.clone());
                    rsx! {
                        crate::pages::agents::TerminalPane {
                            key: "{term}",
                            id: term.clone(),
                            themed: true,
                            autofocus: visible,
                            on_exit: move |code: i32| {
                                // Nur das eigene Terminal (nicht ein schon ersetztes)
                                if CLI.peek().get(&thread_id) != Some(&mine) {
                                    return;
                                }
                                // Wegen eines Laufs geschlossen bzw. verschwunden (−1): danach neu
                                if code == EXIT_FOR_RUN || code < 0 {
                                    CLI.write().insert(thread_id, Cli::Waiting);
                                } else {
                                    forget(thread_id);
                                }
                            },
                        }
                    }
                }
                _ if working => rsx! {
                    div { class: "grid h-full place-items-center p-6",
                        div { class: "flex max-w-sm flex-col items-center gap-3 text-center text-sm text-muted-foreground",
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                            {tr("Der Agent arbeitet gerade im Chat – das CLI startet nach dem Lauf mit dem neuen Stand.")}
                            button {
                                class: "rounded-md border px-3 py-1.5 text-foreground hover:bg-accent",
                                onclick: move |_| toggle(thread_id),
                                {tr("Zum Chat")}
                            }
                        }
                    }
                },
                _ => rsx! {
                    div { class: "grid h-full place-items-center",
                        div { class: "flex items-center gap-2 text-sm text-muted-foreground",
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                            if agent.is_empty() { {tr("CLI startet …")} } else { {trf("{0} startet …", &[&agent])} }
                        }
                    }
                },
            }
        }
    }
}
