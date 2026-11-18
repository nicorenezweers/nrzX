//! Deep Research (Knopf neben „Bots“): eine Frage gründlich im Netz recherchieren lassen – der
//! Agent schreibt ein Research Paper (`paper.md` in seinem Ordner, siehe `src/agents/research.rs`).
//! Die Startseite sieht aus wie „Neue Aufgabe“; statt eines Chats zeigt die Seite einer Recherche
//! das Paper, das live wächst – solange gesucht wird, mit Fortschritt (Suchen, gelesene Quellen).
//! Rechts daneben das Gespräch mit einem Bot, der das Paper sieht (ein eigener Thread, `discuss_of`).
//!
//! * `Research` – Startseite (Frage eingeben)
//! * `ResearchPage` – Paper, Fortschritt, Gespräch
//! * `ResearchList` – Recherchen in der Seitenleiste (statt der Aufgaben)

use super::parts::*;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::state::{self, MESSAGES, THREADS};
use crate::util;
use crate::Route;
use dioxus::prelude::*;
use nrzx_shared::agents::{ResearchPaper, ToolCall};
use nrzx_shared::{ThreadStatus, ThreadSummary};

/// Zuletzt offene Recherche (Ziel des Knopfs).
const LAST_KEY: &str = "nrz-last-research";
/// Gespräch rechts offen (gemerkt).
const DISCUSS_KEY: &str = "nrz-research-discuss";

static DISCUSS_OPEN: GlobalSignal<bool> = Signal::global(|| util::storage_get(DISCUSS_KEY).as_deref() != Some("0"));

/// Wohin der Knopf „Deep Research“ führt: zur zuletzt offenen Recherche, sonst zur Startseite.
pub fn research_route() -> Route {
    let last = util::storage_get(LAST_KEY).and_then(|s| s.parse::<i64>().ok());
    match last {
        Some(id) if THREADS.peek().get(&id).is_some_and(|t| t.research) => Route::ResearchPage { id },
        _ => Route::Research {},
    }
}

// ------------------------------------------------------------------ Startseite

/// Startseite: wie „Neue Aufgabe“ – Frage eingeben, der Agent recherchiert.
#[component]
pub fn Research() -> Element {
    let nav = use_navigator();
    rsx! {
        div { class: "mx-auto flex min-h-full w-full max-w-3xl flex-col justify-center px-4 py-8",
            div { class: "flex flex-col justify-center gap-8",
                div { class: "text-center",
                    div { class: "mx-auto mb-4 grid size-12 place-items-center rounded-2xl bg-white/[0.06] text-foreground [&_svg]:size-6", icons::Telescope {} }
                    h1 { class: "text-2xl font-semibold tracking-tight md:text-3xl", {tr("Deep Research")} }
                    p { class: "mt-2 text-sm text-muted-foreground", {tr("Eine Frage gründlich im Netz recherchieren lassen – mit Quellen, als Research Paper.")} }
                }
                crate::pages::agents::Composer {
                    project_id: None,
                    thread_id: None,
                    research: true,
                    slot: 0usize,
                    on_created: move |id: i64| {
                        nav.push(Route::ResearchPage { id });
                    },
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Seitenleiste

/// Recherchen in der Seitenleiste: laufende zuerst, dann nach letzter Aktivität.
#[component]
pub fn ResearchList() -> Element {
    let route = use_route::<Route>();
    let current = match route {
        Route::ResearchPage { id } => Some(id),
        _ => None,
    };
    let mut list: Vec<ThreadSummary> = THREADS.read().values().filter(|t| t.research && t.parent_id.is_none()).cloned().collect();
    list.sort_by_key(|t| std::cmp::Reverse((t.status.is_active(), t.last_activity_at, t.id)));
    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pt-3 pb-2",
            if list.is_empty() {
                p { class: "px-3 py-8 text-center text-sm text-muted-foreground", {tr("Noch keine Recherchen.")} }
            }
            for t in list {
                ResearchItem { key: "{t.id}", active: current == Some(t.id), thread: t.clone() }
            }
        }
    }
}

#[component]
fn ResearchItem(thread: ThreadSummary, active: bool) -> Element {
    let t = thread.clone();
    let menu = thread.clone();
    let working = t.status.is_active();
    let sub = if working {
        trf("Recherchiert · {0}", &[&short_ago(t.run_started_at.unwrap_or(t.last_activity_at))])
    } else if t.status == ThreadStatus::Error {
        tr("Fehlgeschlagen")
    } else {
        short_ago(t.last_activity_at)
    };
    rsx! {
        div {
            class: "min-w-0",
            oncontextmenu: move |e: MouseEvent| {
                e.prevent_default();
                let p = e.client_coordinates();
                *super::layout::THREAD_MENU.write() = Some((menu.clone(), p.x, p.y));
            },
            Link {
                to: Route::ResearchPage { id: t.id },
                class: if active { "flex min-w-0 items-start gap-3 rounded-xl bg-accent/50 px-3 py-2.5" } else { "flex min-w-0 items-start gap-3 rounded-xl px-3 py-2.5 transition-colors hover:bg-accent/30" },
                span { class: "pt-[3px] text-muted-foreground [&_svg]:size-[15px]",
                    if working {
                        span { class: "block size-[15px] rounded-full border-2 border-dashed border-foreground/70 animate-[spin_4s_linear_infinite]" }
                    } else if t.unread {
                        span { class: "grid size-[15px] place-items-center rounded-full border-2 border-foreground/80",
                            span { class: "size-1.5 rounded-full bg-foreground" }
                        }
                    } else {
                        icons::ScrollText {}
                    }
                }
                div { class: "min-w-0 flex-1",
                    p { class: if t.unread { "truncate text-[15px] font-semibold" } else { "truncate text-[15px] font-medium" }, title: "{t.title}", "{t.title}" }
                    p { class: "truncate text-[13px] text-muted-foreground", "{sub}" }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Recherche

#[component]
pub fn ResearchPage(id: i64) -> Element {
    rsx! {
        for rid in std::iter::once(id) {
            ResearchView { key: "{rid}", id: rid }
        }
    }
}

/// Schritte der Recherche aus dem Verlauf: Suchen und gelesene Seiten (Werkzeug „web“).
fn web_steps(id: i64) -> Vec<ToolCall> {
    MESSAGES
        .read()
        .get(&id)
        .map(|l| l.iter().filter(|m| m.role == "tool").filter_map(|m| serde_json::from_value::<ToolCall>(m.data.clone()).ok()).filter(|c| c.kind == "web").collect())
        .unwrap_or_default()
}

fn is_search(c: &ToolCall) -> bool {
    c.name.to_lowercase().contains("search")
}

#[component]
fn ResearchView(id: i64) -> Element {
    use_hook(move || util::storage_set(LAST_KEY, &id.to_string()));
    // Verlauf laden (Fortschritt und letzte Antwort); live kommen weitere Nachrichten dazu
    use_hook(move || crate::pages::agents::prefetch_messages(vec![id]));
    let thread = THREADS.read().get(&id).cloned();
    let chat = THREADS.read().values().filter(|t| t.discuss_of == Some(id)).map(|t| t.id).max();
    let open = DISCUSS_OPEN();

    // Paper laden: sofort, dann alle 3 s, solange recherchiert wird (sonst selten – auch das
    // Gespräch darf es ändern)
    let mut paper = use_signal(|| None::<ResearchPaper>);
    use_future(move || async move {
        loop {
            let running = THREADS.peek().get(&id).is_some_and(|t| t.status.is_active());
            if let Ok(p) = api::get::<ResearchPaper>(&format!("/threads/{id}/paper")).await {
                if paper.peek().as_ref().map(|x| &x.markdown) != Some(&p.markdown) {
                    paper.set(Some(p));
                }
            }
            gloo_timers::future::TimeoutFuture::new(if running { 3_000 } else { 15_000 }).await;
        }
    });

    // Fertig und angezeigt: gelesen
    let mut marking = use_signal(|| false);
    use_effect(move || {
        let Some(t) = THREADS.read().get(&id).cloned() else { return };
        if t.unread && !t.status.is_active() && !*marking.peek() {
            marking.set(true);
            spawn(async move {
                if let Ok(s) = api::action::<ThreadSummary>(&format!("/threads/{id}/read")).await {
                    state::upsert_thread(s);
                }
                marking.set(false);
            });
        }
    });

    // Gespräch: anlegen, sobald es offen sein soll; es gilt als gezeigt (gelesen, nach unten)
    let mut creating = use_signal(|| false);
    use_effect(move || {
        let wanted = DISCUSS_OPEN();
        let exists = THREADS.read().values().any(|t| t.discuss_of == Some(id));
        let is_research = THREADS.read().get(&id).is_some_and(|t| t.research);
        if wanted && !exists && is_research && !*creating.peek() {
            creating.set(true);
            spawn(async move {
                match api::action::<ThreadSummary>(&format!("/threads/{id}/discuss")).await {
                    Ok(s) => state::upsert_thread(s),
                    Err(e) => toast_error(e),
                }
                creating.set(false);
            });
        }
    });
    let shown_chat = chat.filter(|_| open);
    use_effect(use_reactive!(|shown_chat| {
        if *state::SHOWN_THREAD.peek() != shown_chat {
            *state::SHOWN_THREAD.write() = shown_chat;
        }
    }));
    use_drop(move || {
        if state::SHOWN_THREAD.peek().is_some() {
            *state::SHOWN_THREAD.write() = None;
        }
    });

    let Some(t) = thread.filter(|t| t.research) else {
        return rsx! {
            EmptyState { title: tr("Diese Recherche gibt es nicht mehr."),
                Link { to: Route::Research {}, class: "text-sm text-foreground hover:underline", {tr("Neue Recherche")} }
            }
        };
    };
    let active = t.status.is_active();
    let steps = web_steps(id);
    let searches = steps.iter().filter(|c| is_search(c)).count();
    let reads = steps.len() - searches;
    let p = paper();
    let html = p.as_ref().map(|p| p.html.clone()).filter(|h| !h.trim().is_empty());
    let status = if active {
        trf("Recherchiert seit {0}", &[&short_ago(t.run_started_at.unwrap_or(t.last_activity_at))])
    } else if t.status == ThreadStatus::Error {
        tr("Fehlgeschlagen")
    } else {
        trf("Fertig · {0}", &[&short_ago(t.last_activity_at)])
    };
    // Letzte Antwort des Agenten (Zusammenfassung bzw. Fehler, falls es kein Paper gibt)
    let last_answer = MESSAGES.read().get(&id).and_then(|l| l.iter().rev().find(|m| m.role == "assistant" && !m.content.trim().is_empty()).cloned());
    let stop = move |_| {
        spawn(async move {
            match api::action::<ThreadSummary>(&format!("/threads/{id}/stop")).await {
                Ok(s) => state::upsert_thread(s),
                Err(e) => toast_error(e),
            }
        });
    };
    let toggle_discuss = move |_| {
        let v = !DISCUSS_OPEN();
        util::storage_set(DISCUSS_KEY, if v { "1" } else { "0" });
        *DISCUSS_OPEN.write() = v;
    };
    let head_btn = "inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[13px] transition-colors [&_svg]:size-4";
    rsx! {
        div { class: "flex h-full min-h-0",
            // Paper (statt eines Chats)
            div { class: "flex min-w-0 flex-1 flex-col",
                div { class: "flex shrink-0 items-center gap-3 border-b border-white/[0.08] px-4 py-3 md:px-6",
                    div { class: "grid size-9 shrink-0 place-items-center rounded-xl bg-white/[0.06] [&_svg]:size-[18px]",
                        if active {
                            span { class: "size-4 rounded-full border-2 border-foreground/70 border-t-transparent animate-spin" }
                        } else {
                            icons::Telescope {}
                        }
                    }
                    div { class: "min-w-0 flex-1",
                        div { class: "truncate font-semibold", title: "{t.title}", "{t.title}" }
                        div { class: "truncate text-xs text-muted-foreground",
                            "{status}"
                            if !steps.is_empty() {
                                " · "
                                {trf("{0} Suchen, {1} Quellen gelesen", &[&searches, &reads])}
                            }
                        }
                    }
                    if active {
                        button { class: "{head_btn} text-muted-foreground hover:bg-white/[0.09] hover:text-foreground", title: tr("Recherche stoppen"), onclick: stop,
                            icons::Square {}
                            span { class: "hidden sm:inline", {tr("Stoppen")} }
                        }
                    }
                    button {
                        class: if open { "{head_btn} bg-white/[0.12] text-foreground" } else { "{head_btn} text-muted-foreground hover:bg-white/[0.09] hover:text-foreground" },
                        title: tr("Mit einem Bot über das Paper reden"),
                        onclick: toggle_discuss,
                        icons::MessagesSquare {}
                        span { class: "hidden sm:inline", {tr("Diskutieren")} }
                    }
                }
                div { class: "min-h-0 flex-1 overflow-y-auto",
                    div { class: "mx-auto w-full max-w-3xl px-6 pt-8 pb-16",
                        if active {
                            Progress { steps: steps.clone(), has_paper: html.is_some() }
                        }
                        match html {
                            Some(h) => rsx! { crate::components::widgets::Markdown { html: h, class: "text-[16px] leading-relaxed" } },
                            None if active => rsx! {
                                p { class: "text-center text-sm text-muted-foreground", {tr("Das Research Paper erscheint hier, sobald der Agent es anlegt – danach wächst es mit.")} }
                            },
                            None => rsx! {
                                div { class: "rounded-xl border border-white/10 bg-white/[0.03] p-5 text-sm",
                                    p { class: "font-medium", {tr("Es gibt (noch) kein Research Paper.")} }
                                    if let Some(m) = last_answer {
                                        div { class: "mt-3 text-muted-foreground",
                                            crate::components::widgets::Markdown { html: m.html.clone() }
                                        }
                                    }
                                }
                            },
                        }
                    }
                }
            }
            // Gespräch mit einem Bot, der das Paper sieht
            if open {
                aside { class: "flex w-[26rem] max-w-[45%] shrink-0 flex-col border-l border-white/[0.08]",
                    div { class: "flex shrink-0 items-center gap-2 border-b border-white/[0.08] px-4 py-3",
                        icons::Bot { class: "size-4 text-muted-foreground" }
                        span { class: "min-w-0 flex-1 truncate text-sm font-medium", {tr("Über das Paper reden")} }
                        button {
                            class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-white/10 hover:text-foreground",
                            title: tr("Schliessen"),
                            onclick: toggle_discuss,
                            icons::X { class: "size-4" }
                        }
                    }
                    div { class: "relative min-h-0 flex-1",
                        match chat {
                            Some(c) => rsx! {
                                div { key: "{c}", class: "absolute inset-0", crate::pages::agents::ChatView { thread_id: c } }
                            },
                            None => rsx! { Loading {} },
                        }
                    }
                }
            }
        }
    }
}

/// Fortschritt, solange recherchiert wird: was er gerade tut, die letzten Suchen und Quellen.
#[component]
fn Progress(steps: Vec<ToolCall>, has_paper: bool) -> Element {
    let recent: Vec<ToolCall> = steps.iter().rev().take(6).cloned().collect();
    let phase = if has_paper {
        tr("Schreibt das Paper weiter …")
    } else if steps.is_empty() {
        tr("Plant die Recherche …")
    } else {
        tr("Sucht und liest Quellen …")
    };
    rsx! {
        div { class: "mb-8 rounded-2xl border border-white/10 bg-white/[0.03] p-4",
            div { class: "flex items-center gap-2.5 text-sm font-medium",
                span { class: "size-3.5 shrink-0 rounded-full border-[1.5px] border-foreground/70 border-t-transparent animate-spin" }
                "{phase}"
            }
            if !recent.is_empty() {
                ul { class: "mt-3 grid gap-1.5",
                    for (i , c) in recent.into_iter().enumerate() {
                        li { key: "{i}", class: "flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground [&_svg]:size-3.5 [&_svg]:shrink-0",
                            if is_search(&c) {
                                icons::Search {}
                            } else {
                                icons::FileText {}
                            }
                            span { class: "min-w-0 flex-1 truncate", title: "{c.target}", if c.target.is_empty() { "{c.summary}" } else { "{c.target}" } }
                        }
                    }
                }
            }
        }
    }
}
