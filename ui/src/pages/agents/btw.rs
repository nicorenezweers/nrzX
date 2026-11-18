//! `/btw` – Seiten-Chat zum Zwischenstand: Fragen an ein Modell, das den Verlauf des Threads
//! zusammenfasst, ohne den Agenten zu unterbrechen. Im Composer mit `/btw <Frage>` oder im Reiter
//! „Zwischenstand“ der rechten Spalte.

use crate::api;
use crate::components::overlay::toast_error;
use crate::components::widgets::Markdown;
use crate::i18n::tr;
use super::composer::Picker;
use super::data::{use_agents, Selection};
use dioxus::prelude::*;
use nrzx_shared::agents::{BtwAsk, BtwEntry};
use std::collections::{HashMap, HashSet};

/// Einträge je Thread.
pub static BTW: GlobalSignal<HashMap<i64, Vec<BtwEntry>>> = Signal::global(HashMap::new);
/// Threads mit laufender Frage.
static BUSY: GlobalSignal<HashSet<i64>> = Signal::global(HashSet::new);
/// Zähler: die Thread-Seite öffnet den Seiten-Chat, wenn er steigt.
pub static BTW_OPEN: GlobalSignal<u64> = Signal::global(|| 0);

const MODEL_KEY: &str = "nrz-btw-model";

/// Gewähltes Modell für den Zwischenstand (`None` = automatisch: Agent des Threads, kleines Modell).
pub static BTW_MODEL: GlobalSignal<Option<Selection>> =
    Signal::global(|| crate::util::storage_get(MODEL_KEY).and_then(|t| serde_json::from_str(&t).ok()));

fn set_model(sel: Option<Selection>) {
    match &sel {
        Some(s) => {
            if let Ok(t) = serde_json::to_string(s) {
                crate::util::storage_set(MODEL_KEY, &t);
            }
        }
        None => crate::util::storage_set(MODEL_KEY, ""),
    }
    *BTW_MODEL.write() = sel;
}

/// Frage stellen (öffnet den Seiten-Chat).
pub fn ask(thread_id: i64, question: String) {
    *BTW_OPEN.write() += 1;
    if BUSY.peek().contains(&thread_id) {
        toast_error(tr("Die letzte Frage läuft noch."));
        return;
    }
    BUSY.write().insert(thread_id);
    // Sofort anzeigen, Antwort folgt
    let pending = BtwEntry { id: -1, thread_id, question: question.clone(), status: "pending".into(), ..Default::default() };
    BTW.write().entry(thread_id).or_default().push(pending);
    spawn(async move {
        let body = match BTW_MODEL.peek().clone() {
            Some(m) => BtwAsk { question, agent: Some(m.agent), account_id: m.account_id, model: m.model, effort: m.effort },
            None => BtwAsk { question, ..Default::default() },
        };
        let r = api::post::<BtwEntry, _>(&format!("/threads/{thread_id}/btw"), &body).await;
        let mut all = BTW.write();
        let list = all.entry(thread_id).or_default();
        list.retain(|e| e.id != -1);
        match r {
            Ok(e) => list.push(e),
            Err(e) => toast_error(e),
        }
        drop(all);
        BUSY.write().remove(&thread_id);
    });
}

/// Seiten-Chat (Reiter der rechten Spalte).
#[component]
pub fn BtwPanel(thread_id: i64) -> Element {
    let mut text = use_signal(String::new);
    let picker = use_signal(|| false);
    let agents = use_agents().unwrap_or_default();
    let chosen = BTW_MODEL();
    // Ohne eigene Wahl: wie automatisch (Agent und Konto des Threads, kleines Modell)
    let shown = chosen.clone().unwrap_or_else(|| {
        let t = crate::state::THREADS.read().get(&thread_id).cloned();
        let mut s = t.as_ref().map(Selection::of_thread).unwrap_or_else(Selection::remembered);
        s.model = if s.agent == nrzx_shared::AgentKind::Claude { "haiku".into() } else { String::new() };
        s.effort = "low".into();
        s.normalized(&agents)
    });
    use_hook(move || {
        if !BTW.peek().contains_key(&thread_id) {
            spawn(async move {
                if let Ok(list) = api::get::<Vec<BtwEntry>>(&format!("/threads/{thread_id}/btw")).await {
                    BTW.write().entry(thread_id).or_insert(list);
                }
            });
        }
    });
    let list = BTW.read().get(&thread_id).cloned().unwrap_or_default();
    let busy = BUSY.read().contains(&thread_id);
    let scroll_id = format!("btw-{thread_id}");
    let sid = scroll_id.clone();
    use_effect(move || {
        let _n = BTW.read().get(&thread_id).map(|l| l.len());
        let sid = sid.clone();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&sid)) {
                el.set_scroll_top(el.scroll_height());
            }
        });
    });
    let mut submit = move || {
        let q = text().trim().to_string();
        if busy {
            return;
        }
        text.set(String::new());
        ask(thread_id, q);
    };
    let clear = move |_| {
        spawn(async move {
            match api::delete(&format!("/threads/{thread_id}/btw")).await {
                Ok(()) => {
                    BTW.write().insert(thread_id, Vec::new());
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let empty = list.is_empty();
    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col",
            div { id: "{scroll_id}", class: "min-h-0 flex-1 overflow-y-auto px-3 py-3",
                if empty {
                    div { class: "flex flex-col items-center gap-2 px-4 py-10 text-center text-sm text-muted-foreground",
                        icons::MessagesSquare { class: "size-6" }
                        p { class: "font-medium text-foreground", {tr("Zwischenstand")} }
                        p { {tr("Frag nebenbei, wie weit der Agent ist – ein kleines Modell liest den Verlauf und antwortet. Der Agent wird nicht unterbrochen.")} }
                        for q in [tr("Wie ist der aktuelle Stand?"), tr("Was ist noch offen?"), tr("Gibt es Probleme?")] {
                            button {
                                class: "mt-1 rounded-full border px-3 py-1 text-xs hover:bg-accent hover:text-foreground disabled:opacity-50",
                                disabled: busy,
                                onclick: move |_| ask(thread_id, q.clone()),
                                "{q}"
                            }
                        }
                    }
                }
                div { class: "flex flex-col gap-4",
                    for e in list {
                        div { key: "{e.id}-{e.created_at}", class: "flex flex-col gap-2",
                            div { class: "ml-auto max-w-[90%] rounded-2xl bg-muted px-3 py-1.5 text-sm whitespace-pre-wrap", "{e.question}" }
                            match e.status.as_str() {
                                "pending" => rsx! {
                                    div { class: "flex items-center gap-2 text-xs text-muted-foreground",
                                        icons::LoaderCircle { class: "size-3.5 animate-spin" }
                                        {tr("Liest den Verlauf …")}
                                    }
                                },
                                "error" => rsx! {
                                    div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs whitespace-pre-wrap", "{e.answer}" }
                                },
                                _ => rsx! {
                                    Markdown { html: e.html.clone(), class: "text-sm" }
                                },
                            }
                        }
                    }
                }
            }
            div { class: "shrink-0 border-t p-2",
                // Modell für den Zwischenstand (auch ein anderer Agent oder ein anderes Konto)
                div { class: "mb-1.5 flex items-center gap-1 text-[11px] text-muted-foreground",
                    span { {tr("Modell")} }
                    Picker {
                        open: picker,
                        sel: shown,
                        agents: agents.clone(),
                        onchange: move |n: Selection| set_model(Some(n)),
                    }
                    if chosen.is_some() {
                        button {
                            class: "rounded-full px-2 py-0.5 hover:bg-accent hover:text-foreground",
                            title: tr("Wieder automatisch: Agent des Threads mit kleinem Modell"),
                            onclick: move |_| set_model(None),
                            {tr("Automatisch")}
                        }
                    } else {
                        span { class: "opacity-70", {tr("(automatisch)")} }
                    }
                }
                div { class: "flex items-end gap-1.5 rounded-xl border bg-card px-2 py-1.5 focus-within:border-ring",
                    textarea {
                        class: "max-h-32 min-h-[1.5rem] flex-1 resize-none bg-transparent px-1 py-0.5 text-sm outline-none",
                        rows: "1",
                        placeholder: tr("Frag zum Zwischenstand …"),
                        value: "{text}",
                        oninput: move |e| text.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter && !e.modifiers().shift() {
                                e.prevent_default();
                                submit();
                            }
                        },
                    }
                    button {
                        class: "grid size-7 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground disabled:opacity-40",
                        disabled: busy,
                        title: tr("Fragen (Enter)"),
                        onclick: move |_| submit(),
                        if busy {
                            icons::LoaderCircle { class: "size-3.5 animate-spin" }
                        } else {
                            icons::ArrowUp { class: "size-3.5" }
                        }
                    }
                }
                if !empty {
                    div { class: "mt-1 flex justify-end",
                        button { class: "text-[11px] text-muted-foreground hover:text-foreground", onclick: clear, {tr("Verlauf leeren")} }
                    }
                }
            }
        }
    }
}
