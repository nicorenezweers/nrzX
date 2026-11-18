//! Subagenten eines Threads für die rechte Spalte: Karte „Lineage · 4 running“ mit Menü, je
//! Subagent Icon, Titel, Laufzeit und Status; ein Klick öffnet den Subagent-Thread.

use super::chat::SubagentRow;
use super::composer::SubagentsDialog;
use super::data::{use_agents, use_clock, Selection};
use crate::api;
use crate::components::overlay::{toast_error, Align, Dropdown, MenuItem};
use crate::i18n::{tr, trf};
use crate::state::{self, THREADS};
use crate::Route;
use dioxus::prelude::*;
use nrzx_shared::ThreadSummary;

/// Subagenten eines Threads („Lineage · 4 running“) für die rechte Spalte.
#[component]
pub fn Lineage(thread_id: i64) -> Element {
    rsx! {
        LineageCard { key: "{thread_id}", thread_id }
    }
}

#[component]
fn LineageCard(thread_id: i64) -> Element {
    let _agents = use_agents();
    let now = use_clock();
    let mut dialog = use_signal(|| false);
    use_hook(move || {
        spawn(async move {
            if let Ok(list) = api::get::<Vec<ThreadSummary>>(&format!("/threads?parent={thread_id}")).await {
                for t in list {
                    state::upsert_thread(t);
                }
            }
        });
    });
    let all = THREADS.read();
    let me = all.get(&thread_id).cloned();
    let parent = me.as_ref().and_then(|t| t.parent_id).and_then(|p| all.get(&p).cloned());
    let mut kids: Vec<ThreadSummary> = all.values().filter(|t| t.parent_id == Some(thread_id)).cloned().collect();
    drop(all);
    // Laufende zuerst, dann neueste
    kids.sort_by_key(|k| (!k.status.is_active(), -k.created_at));
    let running = kids.iter().filter(|k| k.status.is_active()).count();
    let sel = me.as_ref().map(Selection::of_thread).unwrap_or_default();
    let project_id = me.as_ref().and_then(|t| t.project_id);
    let stop_all = {
        let ids: Vec<i64> = kids.iter().filter(|k| k.status.is_active()).map(|k| k.id).collect();
        move |_| {
            let ids = ids.clone();
            spawn(async move {
                for id in ids {
                    match api::action::<ThreadSummary>(&format!("/threads/{id}/stop")).await {
                        Ok(s) => state::upsert_thread(s),
                        Err(e) => toast_error(e),
                    }
                }
            });
        }
    };
    rsx! {
        div { class: "rounded-xl border bg-card text-card-foreground",
            div { class: "flex items-center gap-2 px-3 pt-2.5 pb-1",
                span { class: "flex-1 text-xs text-muted-foreground",
                    {tr("Lineage")}
                    if running > 0 {
                        " · "
                        {trf("{0} laufen", &[&running])}
                    }
                }
                Dropdown {
                    align: Align::End,
                    trigger_class: "grid size-6 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                    trigger: rsx! { icons::Ellipsis { class: "size-4" } },
                    MenuItem { onclick: move |_| dialog.set(true),
                        icons::Workflow {}
                        {tr("Subagenten starten …")}
                    }
                    if running > 0 {
                        MenuItem { onclick: stop_all, destructive: true,
                            icons::CircleStop {}
                            {tr("Alle Subagenten stoppen")}
                        }
                    }
                }
            }
            div { class: "px-1.5 pb-2",
                if let Some(p) = parent {
                    Link {
                        to: Route::ThreadPage { id: p.id },
                        class: "flex items-center gap-2.5 rounded-md px-2 py-1.5 text-sm text-muted-foreground hover:bg-accent/50 hover:text-foreground",
                        icons::CornerLeftUp { class: "size-4" }
                        span { class: "truncate", "{p.title}" }
                    }
                }
                if kids.is_empty() {
                    p { class: "px-2 py-1.5 text-xs text-muted-foreground", {tr("Keine Subagenten.")} }
                }
                for k in kids {
                    SubagentRow { key: "{k.id}", t: k, now }
                }
            }
        }
        SubagentsDialog { open: dialog, thread_id: Some(thread_id), project_id, sel }
    }
}
