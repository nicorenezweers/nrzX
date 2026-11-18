//! `/status` – Stand mit Zeitschätzung als Popup über dem Composer (nicht im Seiten-Chat).
//! Ein kurzer Nebenlauf fasst den Verlauf zusammen und schätzt die Restdauer
//! (`POST /threads/<id>/status`); Laufzeit und Todo-Fortschritt sind Fakten vom Server. Die
//! Schätzung ist farbig markiert und als Schätzung gekennzeichnet.

use super::btw::BTW_MODEL;
use super::data::{duration, use_clock};
use crate::api;
use crate::components::widgets::Markdown;
use crate::i18n::{tr, trf, LANG};
use dioxus::prelude::*;
use nrzx_shared::agents::{StatusAsk, StatusReport};
use std::collections::HashMap;

/// Thread, dessen Status-Popup offen ist.
pub static STATUS_OPEN: GlobalSignal<Option<i64>> = Signal::global(|| None);
/// Letzter Bericht je Thread: (Zeitpunkt, Ergebnis); `None` im Ergebnis = läuft gerade.
static REPORTS: GlobalSignal<HashMap<i64, (i64, Option<Result<StatusReport, String>>)>> = Signal::global(HashMap::new);

/// Popup öffnen und den Stand (neu) erfragen.
pub fn open(thread_id: i64) {
    *STATUS_OPEN.write() = Some(thread_id);
    refresh(thread_id);
}

fn refresh(thread_id: i64) {
    if REPORTS.peek().get(&thread_id).is_some_and(|(_, r)| r.is_none()) {
        return;
    }
    REPORTS.write().insert(thread_id, (crate::util::now(), None));
    // Läuft weiter, auch wenn das Popup schliesst – beim nächsten Öffnen ist der Stand da
    dioxus::core::spawn_forever(async move {
        let body = match BTW_MODEL.peek().clone() {
            Some(m) => StatusAsk { lang: LANG.peek().clone(), agent: Some(m.agent), account_id: m.account_id, model: m.model, effort: m.effort },
            None => StatusAsk { lang: LANG.peek().clone(), ..Default::default() },
        };
        let r = api::post::<StatusReport, _>(&format!("/threads/{thread_id}/status"), &body).await.map_err(|e| e.message);
        REPORTS.write().insert(thread_id, (crate::util::now(), Some(r)));
    });
}

fn close() {
    *STATUS_OPEN.write() = None;
}

/// Popup über dem Composer (im `relative`-Rahmen des Composers eingehängt).
#[component]
pub fn StatusPopup(thread_id: i64) -> Element {
    let now = use_clock();
    let entry = REPORTS.read().get(&thread_id).cloned();
    let asked_at = entry.as_ref().map(|(t, _)| *t).unwrap_or(0);
    let report = entry.and_then(|(_, r)| r);
    let loading = report.is_none();
    rsx! {
        // Klick daneben schliesst
        div { class: "fixed inset-0 z-40", onclick: move |_| close() }
        div {
            class: "nrx-pop absolute bottom-full left-0 right-0 z-50 mb-2 overflow-hidden animate-in",
            tabindex: "-1",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    close();
                }
            },
            onmounted: move |e| {
                spawn(async move {
                    let _ = e.set_focus(true).await;
                });
            },
            // Kopf
            div { class: "flex items-center gap-2 border-b px-4 py-2.5",
                icons::Activity { class: "size-4 text-muted-foreground" }
                span { class: "text-sm font-semibold", {tr("Stand")} }
                if let Some(Ok(r)) = report.as_ref() {
                    if r.active {
                        span { class: "flex items-center gap-1.5 rounded-full bg-info/12 px-2 py-0.5 text-[11.5px] text-info",
                            span { class: "size-1.5 animate-pulse rounded-full bg-info" }
                            match r.running_since {
                                Some(s) => rsx! { {trf("arbeitet seit {0}", &[&duration(now - s)])} },
                                None => rsx! { {tr("arbeitet")} },
                            }
                        }
                    } else {
                        span { class: "rounded-full bg-muted px-2 py-0.5 text-[11.5px] text-muted-foreground", {tr("arbeitet gerade nicht")} }
                    }
                }
                div { class: "flex-1" }
                button {
                    class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40",
                    title: tr("Neu schätzen"),
                    disabled: loading,
                    onclick: move |_| refresh(thread_id),
                    icons::RefreshCw { class: if loading { "size-3.5 animate-spin" } else { "size-3.5" } }
                }
                button { class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground", title: tr("Schliessen (Esc)"), onclick: move |_| close(),
                    icons::X { class: "size-4" }
                }
            }
            div { class: "flex max-h-[60vh] flex-col gap-3 overflow-y-auto px-4 py-3",
                // Die Schätzung – markiert und als Schätzung gekennzeichnet
                div { class: "flex items-center gap-3 rounded-xl border border-warning/40 bg-warning/10 px-3.5 py-2.5",
                    icons::Hourglass { class: "size-5 shrink-0 text-warning" }
                    div { class: "min-w-0 flex-1",
                        div { class: "flex items-center gap-2",
                            span { class: "text-[11px] font-medium uppercase tracking-wide text-warning", {tr("Geschätzte Restdauer")} }
                            span { class: "rounded border border-warning/40 px-1 text-[10px] font-medium text-warning/90", title: tr("Schätzung eines kleinen Modells aus dem Verlauf – kann danebenliegen"), {tr("Schätzung")} }
                        }
                        match report.as_ref() {
                            None => rsx! { div { class: "mt-1 h-6 w-32 animate-pulse rounded-md bg-warning/20" } },
                            Some(Ok(r)) if !r.estimate.is_empty() => rsx! { p { class: "text-lg font-semibold leading-tight text-foreground", "{r.estimate}" } },
                            Some(_) => rsx! { p { class: "text-sm text-muted-foreground", {tr("Keine Schätzung möglich")} } },
                        }
                    }
                }
                // Fakten: Todo-Liste des Agenten
                if let Some(Ok(r)) = report.as_ref() {
                    if r.todos_total > 0 {
                        div { class: "flex flex-col gap-1.5",
                            div { class: "flex items-center gap-2 text-xs text-muted-foreground",
                                icons::ListChecks { class: "size-3.5" }
                                span { {trf("Todo-Liste: {0} von {1} erledigt", &[&r.todos_done, &r.todos_total])} }
                            }
                            div { class: "h-1.5 overflow-hidden rounded-full bg-muted",
                                div { class: "h-full rounded-full bg-success transition-[width]", style: "width: {r.todos_done as f64 / r.todos_total as f64 * 100.0}%" }
                            }
                            if !r.todo_current.is_empty() {
                                p { class: "truncate text-xs text-muted-foreground", title: "{r.todo_current}", {trf("Gerade: {0}", &[&r.todo_current])} }
                            }
                        }
                    }
                }
                // Stand
                match report.as_ref() {
                    None => rsx! {
                        div { class: "flex flex-col gap-2 py-1",
                            div { class: "h-3 w-11/12 animate-pulse rounded bg-muted" }
                            div { class: "h-3 w-4/5 animate-pulse rounded bg-muted" }
                            div { class: "h-3 w-2/3 animate-pulse rounded bg-muted" }
                            p { class: "text-xs text-muted-foreground", {tr("Fasse den Verlauf zusammen …")} }
                        }
                    },
                    Some(Err(e)) => rsx! { p { class: "text-sm text-destructive", "{e}" } },
                    Some(Ok(r)) if !r.error.is_empty() => rsx! { p { class: "text-sm text-destructive", "{r.error}" } },
                    Some(Ok(r)) => rsx! { Markdown { html: r.html.clone(), class: "text-sm [&_p]:my-1 [&_ul]:my-1" } },
                }
                if !loading && asked_at > 0 {
                    p { class: "text-[11px] text-muted-foreground/70", {trf("Stand von vor {0} · der Agent wird nicht unterbrochen", &[&duration(now - asked_at)])} }
                }
            }
        }
    }
}
