//! Rahmen der App wie ein Coding-Agent-Client: links die Seitenleiste (nrzX, Canvas-Modus,
//! Suche, Filter, „Projekt hinzufügen“, „Neuer Thread“, letzte Threads, Gruppen „In Arbeit“ und
//! „Abgelegt“, unten ein Update der App (nur, wenn eines da ist) und die Nutzung),
//! oben die Kopfzeile „Projekt / Thread“ mit drei Knöpfen für die Spalten und dem Ring der
//! Nutzungslimits. Die Einstellungen füllen das ganze Fenster (eigene Navigation, X zurück).
//! Lädt Status, Benutzer, Threads und Projekte und startet die Live-Ereignisse; leitet zur
//! Einrichtung bzw. Anmeldung weiter.

use super::parts::*;
use super::projects::AddProjectDialog;
use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::i18n::{self, LANG, tr, trf};
use crate::state::{self, ME, ME_LOADED, PROJECTS, THREADS};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::agents::ThreadPatch;
use nrzx_shared::workspace::Stats;
use nrzx_shared::{ThreadStatus, ThreadSummary};

/// Rahmen aller Seiten (ausser Anmeldung und Einrichtung).
#[component]
pub fn AppLayout() -> Element {
    let nav = use_navigator();
    let route = use_route::<Route>();
    let boot = use_resource(|| async {
        let status = state::load_status().await;
        let me = state::load_me().await;
        if let Some(m) = &me {
            if !m.language.is_empty() && *LANG.peek() != m.language {
                i18n::set_language(&m.language, true).await;
            }
            load_projects().await;
            load_threads().await;
            crate::events::connect();
            // Verläufe laufender und der neuesten Threads im Hintergrund laden: beim Wechsel
            // zwischen Threads und Agenten sind die Nachrichten sofort da
            let mut list: Vec<ThreadSummary> = THREADS.peek().values().cloned().collect();
            list.sort_by_key(|t| (std::cmp::Reverse(t.status.is_active()), std::cmp::Reverse(t.last_activity_at)));
            crate::pages::agents::prefetch_messages(list.into_iter().take(12).map(|t| t.id).collect());
        }
        (status, me.is_some())
    });

    super::parts::remember_new_thread_project();
    // Klick auf eine Benachrichtigung oder in der Menüleiste: Thread öffnen (-1 = neuer Thread)
    use_effect(move || {
        if let Some(id) = crate::notify::OPEN_THREAD() {
            *crate::notify::OPEN_THREAD.write() = None;
            if id > 0 {
                nav.push(Route::ThreadPage { id });
            } else if id == -1 {
                // Aus der Menüleiste: neuer Thread
                nav.push(Route::Home {});
            }
        }
    });

    // Einstellungen öffnen (⌘, im Menü der App, Strg+, unter Windows und im Browser)
    use_effect(move || {
        if state::OPEN_SETTINGS() {
            *state::OPEN_SETTINGS.write() = false;
            nav.push(settings_route());
        }
    });

    // Geöffneter Thread bleibt geladen (die letzten vier); welcher gerade zu sehen ist
    use_effect(use_reactive!(|route| {
        let shown = match route {
            Route::ThreadPage { id } => Some(id),
            _ => None,
        };
        if *crate::state::SHOWN_THREAD.peek() != shown {
            *crate::state::SHOWN_THREAD.write() = shown;
        }
        if let Route::ThreadPage { id } = route {
            let mut k = KEPT.peek().clone();
            k.retain(|x| *x != id);
            k.push(id);
            while k.len() > KEEP_THREADS {
                k.remove(0);
            }
            *KEPT.write() = k;
        }
    }));

    // Letzte Seite ausserhalb der Einstellungen: dorthin führt ihr X zurück
    use_effect(use_reactive!(|route| {
        if !matches!(route, Route::Settings { .. }) {
            *RETURN_TO.write() = Some(route.clone());
        }
    }));

    // Weiterleiten: noch nicht eingerichtet → Einrichtung; nicht angemeldet → Anmelden
    use_effect(move || {
        if let Some((Some(status), signed_in)) = boot.read().clone() {
            if status.needs_setup {
                nav.replace(Route::Setup {});
            } else if !signed_in {
                nav.replace(Route::Login { next: util::current_path() });
            }
        }
    });

    // Sekundentakt für „Arbeitet 25s“; nach einer Unterbrechung Listen neu laden
    use_future(|| async {
        let mut n = 0u32;
        loop {
            gloo_timers::future::TimeoutFuture::new(1000).await;
            *NOW.write() = util::now();
            n += 1;
            if n.is_multiple_of(120) && ME.peek().is_some() {
                load_threads().await;
                load_projects().await;
            }
        }
    });

    // ⌘K (bzw. Strg+K): Suche der Aufgabenleiste – klappt sie dazu aus
    use_hook(|| {
        use wasm_bindgen::{closure::Closure, JsCast};
        let down = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
            if (e.meta_key() || e.ctrl_key()) && !e.alt_key() && e.key().eq_ignore_ascii_case("k") {
                e.prevent_default();
                if !*LEFT.peek() {
                    toggle(&LEFT, "nrz-left");
                }
                let v = !*super::tabs::SEARCH_OPEN.peek();
                *super::tabs::SEARCH_OPEN.write() = v;
            }
        });
        if let Some(w) = web_sys::window() {
            let _ = w.add_event_listener_with_callback("keydown", down.as_ref().unchecked_ref());
        }
        down.forget();
    });

    // Seitenwechsel: Sheets auf dem Handy schliessen
    use_effect(use_reactive!(|route| {
        let _ = &route;
        *MOBILE_NAV.write() = false;
        *MOBILE_RIGHT.write() = false;
    }));

    if boot.read().is_none() || !ME_LOADED() || ME.read().is_none() {
        return rsx! {
            div { class: "grid h-dvh place-items-center", Loading {} }
        };
    }
    // Einstellungen: das ganze Fenster (eigene Navigation links, oben rechts X zurück)
    if matches!(route, Route::Settings { .. }) {
        return rsx! {
            // Eigenes Fenster: oben eine Reihe für die Ampel (zieht das Fenster), darunter das Paneel
            div { class: "nrx-backdrop flex h-dvh flex-col overflow-hidden px-2 pb-2",
                // Ampel über eine kompakte Toolbar: Mitte 20 pt
                div { class: "h-2 shrink-0 [.nrz-mac_&]:h-10", "data-drag": "1" }
                div { class: "nrx-panel flex min-h-0 min-w-0 flex-1 overflow-hidden", Outlet::<Route> {} }
            }
            AddProjectDialog {}
        };
    }
    // Drei Glas-Paneele über dem Hintergrund: links die Aufgaben, in der Mitte der Chat (die
    // Thread-Seite zeichnet Chat, Browser und rechte Spalte selbst als eigene Paneele)
    let current = match route {
        Route::ThreadPage { id } => Some(id),
        _ => None,
    };
    let tab_row = super::tabs::has_row(&route);
    // Eingeklappt ohne Reiter (Thread oder Bot): die Fläche des Chats reicht neben den Knöpfen bis oben
    let big_top = !LEFT() && !super::tabs::route_has_tabs(&route);
    // Paneel der übrigen Seiten; beim Bot hängt das gewählte Gespräch als Reiter daran
    let attach = match route {
        Route::BotPage { id } => super::tabs::bot_panel_attach(id),
        _ if LEFT() => "",
        _ => "md:rounded-tr-none nrx-notch-big",
    };
    // Zuletzt geöffnete Threads bleiben geladen (wie in einer App): zurück ohne Neuladen – Seiten
    // im Browser, Terminals, CLI und die Stelle im Chat bleiben
    let mut kept = KEPT();
    if let Some(id) = current {
        if !kept.contains(&id) {
            kept.push(id);
        }
    }
    kept.retain(|id| THREADS.read().contains_key(id));
    rsx! {
        // Wie in der Skizze: links die Aufgabenleiste mit ihrem Reiter „Neue Aufgabe“ (daneben die
        // Ampel), rechts der Chat – mit Reiter-Reihe, wenn Reiter offen sind, sonst bis ganz oben.
        // Die freie Fläche der Reihen zieht das Fenster.
        div { class: "nrx-backdrop flex h-dvh gap-2 overflow-hidden px-2 pb-2",
            if LEFT() {
                div { class: "hidden w-[336px] shrink-0 flex-col md:flex",
                    // Ampel, Knopf der Seitenleiste, „Bots“ und „Deep Research“, rechts der Reiter „Neue Aufgabe“
                    div { class: "nrx-row flex shrink-0 items-end", "data-drag": "1",
                        div { class: "nrx-lead mb-1 flex shrink-0 items-center gap-1",
                            super::tabs::SidebarToggle {}
                            super::tabs::BotButton {}
                            super::tabs::ResearchButton {}
                        }
                        div { class: "min-w-0 flex-1 self-stretch", "data-drag": "1" }
                        super::tabs::TaskTab {}
                    }
                    aside { class: "nrx-panel nrx-notch-tr flex min-h-0 flex-1 flex-col overflow-hidden rounded-tr-none",
                        if super::tabs::bot_mode(&route) {
                            super::bots::BotList { query: String::new() }
                        } else if super::tabs::research_mode(&route) {
                            super::research::ResearchList {}
                        } else {
                            super::shell::TaskSidebar {}
                        }
                    }
                }
            }
            if MOBILE_NAV() {
                div { class: "fixed inset-0 z-50 bg-black/60 md:hidden", onclick: move |_| *MOBILE_NAV.write() = false }
                aside { class: "nrz-pane fixed inset-y-0 left-0 z-50 flex w-[85vw] max-w-80 flex-col border-r bg-sidenav shadow-xl animate-in md:hidden",
                    if super::tabs::bot_mode(&route) {
                        super::bots::BotList { query: String::new() }
                    } else if super::tabs::research_mode(&route) {
                        super::research::ResearchList {}
                    } else {
                        super::shell::TaskSidebar {}
                    }
                }
            }
            // Variablen für die Aussparung im Umriss des Paneels unter dem gewählten Reiter
            div { class: "flex min-w-0 flex-1 flex-col", style: super::tabs::row_vars(&route),
                if tab_row {
                    div { class: "nrx-row hidden shrink-0 items-end md:flex", "data-drag": "1",
                        // Eingeklappt: keine Leiste – Ampel, Knopf der Seitenleiste (an derselben Stelle
                        // wie sonst) und „Neue Aufgabe“ frei auf dem Fensterhintergrund; rechts daneben
                        // reicht die Fläche des Chats bis oben (darin die Kopfleiste), bzw. es folgen
                        // die Reiter des Threads
                        if !LEFT() {
                            div { class: "nrx-lead mb-1 flex shrink-0 items-center gap-1",
                                super::tabs::SidebarToggle {}
                                super::tabs::NewTaskButton {}
                                super::tabs::BotButton {}
                                super::tabs::ResearchButton {}
                            }
                            if big_top {
                                div { class: "w-[26px] shrink-0 self-stretch", "data-drag": "1" }
                                div { class: "nrx-tab nrx-tab-big nrx-fl", "data-drag": "1",
                                    span { class: "nrx-line" }
                                    CenterBar { thread: current, edge: true, top: true, inline: true }
                                }
                            } else {
                                div { class: "w-5 shrink-0" }
                            }
                        }
                        if !big_top {
                            super::tabs::ThreadTabs {}
                        }
                    }
                }
                // Ohne Reiter-Reihe (bzw. schmal): Streifen über dem Paneel, zieht ebenfalls das Fenster
                div { class: if tab_row { "h-2 shrink-0 md:hidden" } else { "h-2 shrink-0" }, "data-drag": "1" }
                div { class: "flex min-h-0 flex-1",
                    for tid in kept {
                        div { key: "{tid}", class: if current == Some(tid) { "flex min-h-0 min-w-0 flex-1" } else { "hidden" },
                            super::thread::ThreadView { id: tid }
                        }
                    }
                    if current.is_none() {
                        section { class: "nrx-panel flex min-w-0 flex-1 flex-col overflow-hidden {attach}",
                            // eingeklappt steht die Kopfleiste oben im Reiter; Bots und Recherchen haben ihren eigenen Kopf
                            if LEFT() && !super::tabs::bot_mode(&route) && !super::tabs::research_mode(&route) {
                                CenterBar { thread: None, edge: true, top: !tab_row }
                            }
                            main { id: "nrz-main", class: "min-h-0 flex-1 overflow-y-auto", Outlet::<Route> {} }
                        }
                    }
                }
            }
        }
        AddProjectDialog {}
        ThreadMenu {}
        ThreadStatsDialog {}
        ProjectMenu {}
        super::bots::BotEditor {}
    }
}

// ------------------------------------------------------------------ Seitenleiste


/// So viele zuletzt geöffnete Threads bleiben geladen.
const KEEP_THREADS: usize = 4;
/// Zuletzt geöffnete Threads (der neueste zuletzt) – ihre Ansichten bleiben eingehängt.
static KEPT: GlobalSignal<Vec<i64>> = Signal::global(Vec::new);

/// Seite vor den Einstellungen (Ziel ihres X).
pub static RETURN_TO: GlobalSignal<Option<Route>> = Signal::global(|| None);

/// Zuletzt offener Abschnitt der Einstellungen.
pub fn settings_route() -> Route {
    let section = util::storage_get("nrz-settings-section").filter(|s| !s.is_empty()).unwrap_or_else(|| "profile".into());
    Route::Settings { section }
}

/// Einstellungen schliessen: zurück zur Seite davor (sonst Startseite).
pub fn close_settings(nav: dioxus::router::Navigator) {
    let to = RETURN_TO.peek().clone().filter(|r| !matches!(r, Route::Settings { .. })).unwrap_or(Route::Home {});
    nav.push(to);
}

/// Offenes Kontextmenü eines Threads (Rechtsklick oder „…“) mit Position im Fenster.
pub(super) static THREAD_MENU: GlobalSignal<Option<(ThreadSummary, f64, f64)>> = Signal::global(|| None);
/// Offenes Kontextmenü eines Projekts (Rechtsklick auf den Projektkopf): Id, Name, Position.
static PROJECT_MENU: GlobalSignal<Option<(i64, String, f64, f64)>> = Signal::global(|| None);
/// Schlüssel des Platzhalters in der Liste.
pub(super) const PENDING_KEY: &str = "pending";

/// Kontextmenü eines Threads – einmal im Rahmen eingebunden.
#[component]
fn ThreadMenu() -> Element {
    let nav = use_navigator();
    let route = use_route::<Route>();
    let Some((t, x, y)) = THREAD_MENU() else { return rsx! {} };
    let (w, h) = web_sys::window()
        .map(|w| (w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1200.0), w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(800.0)))
        .unwrap_or((1200.0, 800.0));
    let left = x.min(w - 236.0).max(8.0);
    let top = y.min(h - 400.0).max(8.0);
    let id = t.id;
    let current = matches!(route, Route::ThreadPage { id: c } if c == id);
    let close = || *THREAD_MENU.write() = None;
    let settled = t.status == ThreadStatus::Settled;
    let (pinned, hidden, read_only, freestyle) = (t.pinned, t.hidden, t.read_only, t.freestyle);
    let patch = move |p: ThreadPatch| {
        close();
        spawn(async move {
            match api::patch::<ThreadSummary, _>(&format!("/threads/{id}"), &p).await {
                Ok(s) => state::upsert_thread(s),
                Err(e) => toast_error(e),
            }
        });
    };
    let title = thread_title(&t);
    rsx! {
        div {
            class: "fixed inset-0 z-[55]",
            onclick: move |_| close(),
            oncontextmenu: move |e| {
                e.prevent_default();
                close();
            },
        }
        div {
            class: "nrx-pop fixed z-[56] w-60 overflow-hidden p-1.5 animate-in",
            style: "left:{left}px;top:{top}px",
            role: "menu",
            tabindex: "-1",
            onmounted: move |e| async move {
                let _ = e.set_focus(true).await;
            },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    close();
                }
            },
            MenuLabel { span { class: "block truncate", "{title}" } }
            MenuItem {
                onclick: move |_| {
                    close();
                    nav.push(Route::ThreadPage { id });
                },
                icons::MessageSquare {}
                {tr("Öffnen")}
            }
            MenuItem { onclick: move |_| patch(ThreadPatch { pinned: Some(!pinned), ..Default::default() }),
                if pinned {
                    icons::PinOff {}
                    {tr("Lösen")}
                } else {
                    icons::Pin {}
                    {tr("Anheften")}
                }
            }
            MenuItem { onclick: move |_| patch(ThreadPatch { hidden: Some(!hidden), ..Default::default() }),
                if hidden {
                    icons::Eye {}
                    {tr("Wieder einblenden")}
                } else {
                    icons::EyeOff {}
                    {tr("Ausblenden")}
                }
            }
            MenuSeparator {}
            // Modus des Threads – gilt ab dem nächsten Lauf
            MenuItem { onclick: move |_| patch(ThreadPatch { read_only: Some(!read_only), ..Default::default() }),
                icons::Eye {}
                span { class: "flex-1", {tr("Nur lesen")} }
                if read_only {
                    icons::Check { class: "text-muted-foreground" }
                }
            }
            if !read_only {
                MenuItem { onclick: move |_| patch(ThreadPatch { freestyle: Some(!freestyle), ..Default::default() }),
                    icons::Sparkles {}
                    span { class: "flex-1", "FreeStyle" }
                    if freestyle {
                        icons::Check { class: "text-muted-foreground" }
                    }
                }
            }
            MenuItem {
                onclick: move |_| {
                    close();
                    *THREAD_STATS.write() = Some(id);
                },
                icons::ChartColumn {}
                {tr("Statistik")}
            }
            MenuSeparator {}
            MenuItem {
                onclick: move |_| {
                    close();
                    spawn(async move {
                        let path = if settled { format!("/threads/{id}/reopen") } else { format!("/threads/{id}/settle") };
                        match api::action::<ThreadSummary>(&path).await {
                            Ok(s) => state::upsert_thread(s),
                            Err(e) => toast_error(e),
                        }
                    });
                },
                if settled {
                    icons::ArchiveRestore {}
                    {tr("Wieder öffnen")}
                } else {
                    icons::Archive {}
                    {tr("Schliessen (ablegen)")}
                }
            }
            MenuSeparator {}
            MenuItem {
                destructive: true,
                onclick: move |_| {
                    close();
                    spawn(async move {
                        let ask = Ask::new(tr("Thread löschen?"), tr("Der Verlauf dieses Threads und seiner Subagenten wird gelöscht, ein eigener Worktree ebenfalls. Branches und Commits bleiben erhalten."))
                            .confirm(tr("Löschen"))
                            .destructive();
                        if !confirm(ask).await {
                            return;
                        }
                        match api::delete(&format!("/threads/{id}")).await {
                            Ok(()) => {
                                THREADS.write().remove(&id);
                                if current {
                                    nav.push(Route::Home {});
                                }
                            }
                            Err(e) => toast_error(e),
                        }
                    });
                },
                icons::Trash2 {}
                {tr("Löschen")}
            }
        }
    }
}

/// Thread, dessen Statistik gerade offen ist.
static THREAD_STATS: GlobalSignal<Option<i64>> = Signal::global(|| None);

/// Statistik eines Threads (aus dem Kontextmenü) – einmal im Rahmen eingebunden.
#[component]
fn ThreadStatsDialog() -> Element {
    let mut open = use_signal(|| false);
    let id = THREAD_STATS();
    // Nur schreiben, wenn sich etwas ändert: Jedes `set`/`write` weckt die Leser, auch bei gleichem
    // Wert – sonst lösen sich die beiden Effekte gegenseitig endlos aus (App hing beim Start).
    use_effect(move || {
        let want = THREAD_STATS().is_some();
        if *open.peek() != want {
            open.set(want);
        }
    });
    use_effect(move || {
        if !open() && THREAD_STATS.peek().is_some() {
            *THREAD_STATS.write() = None;
        }
    });
    let Some(id) = id else { return rsx! {} };
    let title = THREADS.peek().get(&id).map(thread_title).unwrap_or_default();
    rsx! {
        Modal { open, title: tr("Statistik"), description: title, class: "max-w-2xl",
            ThreadStatsBody { key: "{id}", id }
        }
    }
}

#[component]
fn ThreadStatsBody(id: i64) -> Element {
    let stats = use_resource(move || async move { api::get::<nrzx_shared::workspace::ThreadStats>(&format!("/threads/{id}/stats")).await });
    let s = match stats.read().clone() {
        None => return rsx! { Loading {} },
        Some(Err(e)) => return rsx! { Callout { text: e.message, error: true } },
        Some(Ok(s)) => s,
    };
    let n = util::number;
    let subs = s.subagents > 0;
    type Getter = fn(&nrzx_shared::workspace::TokenSums) -> String;
    let rows: Vec<(String, bool, Getter)> = vec![
        (tr("Läufe"), false, |x| util::number(x.runs)),
        (tr("Eingabe gesamt"), true, |x| util::number(x.input_tokens)),
        (tr("davon neu"), false, |x| util::number(x.fresh_input())),
        (tr("davon Cache gelesen"), false, |x| util::number(x.cache_read_tokens)),
        (tr("davon Cache geschrieben"), false, |x| util::number(x.cache_write_tokens)),
        (tr("Ausgabe gesamt"), true, |x| util::number(x.output_tokens)),
        (tr("davon Denken"), false, |x| util::number(x.reasoning_tokens)),
        (tr("Tokens gesamt"), true, |x| util::number(x.total_tokens())),
        (tr("Kosten"), true, |x| format!("$ {:.4}", x.cost_usd)),
        (tr("Laufzeit"), false, |x| duration(x.seconds)),
    ];
    let th = "px-2 py-1.5 text-right font-medium text-muted-foreground whitespace-nowrap";
    let td = "px-2 py-1.5 text-right tabular-nums whitespace-nowrap";
    rsx! {
        div { class: "grid grid-cols-3 gap-2",
            div { class: "rounded-lg border bg-card px-3 py-2",
                p { class: "text-xs text-muted-foreground", {tr("Nachrichten")} }
                p { class: "mt-0.5 text-lg font-semibold tabular-nums", {n(s.user_messages + s.agent_messages)} }
            }
            div { class: "rounded-lg border bg-card px-3 py-2",
                p { class: "text-xs text-muted-foreground", {tr("Werkzeugaufrufe")} }
                p { class: "mt-0.5 text-lg font-semibold tabular-nums", {n(s.tool_calls)} }
            }
            div { class: "rounded-lg border bg-card px-3 py-2",
                p { class: "text-xs text-muted-foreground", {tr("Subagenten")} }
                p { class: "mt-0.5 text-lg font-semibold tabular-nums", {n(s.subagents)} }
            }
        }
        div { class: "mt-4 overflow-x-auto rounded-lg border",
            table { class: "w-full text-xs",
                thead { class: "border-b bg-muted/40",
                    tr {
                        th { class: "px-2 py-1.5 text-left font-medium text-muted-foreground", "" }
                        th { class: th, {tr("Dieser Thread")} }
                        if subs {
                            th { class: th, {tr("Mit Subagenten")} }
                        }
                    }
                }
                tbody {
                    for (label, strong, get) in rows {
                        tr { class: "border-b last:border-0",
                            td { class: if strong { "px-2 py-1.5 font-medium whitespace-nowrap" } else { "px-2 py-1.5 pl-5 text-muted-foreground whitespace-nowrap" }, "{label}" }
                            td { class: if strong { "{td} font-medium" } else { "{td} text-muted-foreground" }, {get(&s.own)} }
                            if subs {
                                td { class: if strong { "{td} font-medium" } else { "{td} text-muted-foreground" }, {get(&s.with_subagents)} }
                            }
                        }
                    }
                }
            }
        }
        if !s.models.is_empty() {
            p { class: "mt-4 mb-2 text-sm font-medium", {tr("Je Modell")} }
            div { class: "grid gap-1.5",
                for m in s.models.iter().cloned() {
                    div { key: "{m.agent.key()}-{m.model}", class: "flex items-center gap-2 text-xs",
                        crate::pages::agents::AgentIcon { kind: m.agent, size: 14 }
                        span { class: "min-w-0 flex-1 truncate", {crate::pages::agents::model_label(m.agent, &m.model)} }
                        span { class: "tabular-nums text-muted-foreground", {trf("{0} Läufe", &[&n(m.sums.runs)])} }
                        span { class: "w-28 text-right tabular-nums", {trf("{0} Tokens", &[&n(m.sums.total_tokens())])} }
                        span { class: "w-20 text-right tabular-nums text-muted-foreground", {cost(m.sums.cost_usd)} }
                    }
                }
            }
        }
    }
}

/// Kontextmenü eines Projekts in der Seitenleiste – einmal im Rahmen eingebunden.
#[component]
fn ProjectMenu() -> Element {
    let nav = use_navigator();
    let route = use_route::<Route>();
    let Some((id, name, x, y)) = PROJECT_MENU() else { return rsx! {} };
    let (w, h) = web_sys::window()
        .map(|w| (w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1200.0), w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(800.0)))
        .unwrap_or((1200.0, 800.0));
    let left = x.min(w - 236.0).max(8.0);
    let top = y.min(h - 170.0).max(8.0);
    let close = || *PROJECT_MENU.write() = None;
    // Liegt die offene Seite in diesem Projekt? Dann nach dem Löschen zur Startseite.
    let inside = match route {
        Route::ProjectPage { id: p } => p == id,
        Route::ThreadPage { id: t } => THREADS.peek().get(&t).is_some_and(|t| t.project_id == Some(id)),
        _ => false,
    };
    let label = name.clone();
    rsx! {
        div {
            class: "fixed inset-0 z-[55]",
            onclick: move |_| close(),
            oncontextmenu: move |e| {
                e.prevent_default();
                close();
            },
        }
        div {
            class: "nrx-pop fixed z-[56] w-60 overflow-hidden p-1.5 animate-in",
            style: "left:{left}px;top:{top}px",
            role: "menu",
            tabindex: "-1",
            onmounted: move |e| async move {
                let _ = e.set_focus(true).await;
            },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    close();
                }
            },
            MenuLabel { span { class: "block truncate", "{label}" } }
            MenuItem {
                onclick: move |_| {
                    close();
                    *MOBILE_NAV.write() = false;
                    nav.push(Route::ProjectPage { id });
                },
                icons::Folder {}
                {tr("Öffnen")}
            }
            MenuItem {
                onclick: move |_| {
                    close();
                    *NEW_THREAD_PROJECT.write() = Some(id);
                    *MOBILE_NAV.write() = false;
                    nav.push(Route::Home {});
                },
                icons::Plus {}
                {tr("Neuer Thread in diesem Projekt")}
            }
            MenuSeparator {}
            MenuItem {
                destructive: true,
                onclick: move |_| {
                    close();
                    let name = name.clone();
                    spawn(async move {
                        let ask = Ask::new(
                            trf("Projekt „{0}“ löschen?", &[&name]),
                            tr("Alle Threads des Projekts und die Arbeitsordner auf dem Server werden gelöscht, laufende Agenten angehalten. Das Repository in nrzGit bleibt erhalten."),
                        )
                        .confirm(tr("Löschen"))
                        .destructive();
                        if !confirm(ask).await {
                            return;
                        }
                        match api::delete(&format!("/projects/{id}")).await {
                            Ok(()) => {
                                PROJECTS.write().remove(&id);
                                THREADS.write().retain(|_, t| t.project_id != Some(id));
                                if *NEW_THREAD_PROJECT.peek() == Some(id) {
                                    *NEW_THREAD_PROJECT.write() = None;
                                }
                                if inside {
                                    nav.push(Route::Home {});
                                }
                            }
                            Err(e) => toast_error(e),
                        }
                    });
                },
                icons::Trash2 {}
                {tr("Projekt löschen")}
            }
        }
    }
}

// ------------------------------------------------------------------ Statistik

/// Dauer „3 h 12 min“, „4 min 5 s“.
fn duration(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min {s} s")
    } else {
        format!("{s} s")
    }
}

fn cost(c: f64) -> String {
    format!("$ {c:.4}")
}

/// Genaue Zahlen (unter Einstellungen → Nutzung).
#[component]
pub fn StatsBody() -> Element {
    let stats = use_resource(|| async { api::get::<Stats>(&format!("/me/stats?tz={}", crate::pages::agents::tz_offset())).await });
    let s = match stats.read().clone() {
        None => return rsx! { Loading {} },
        Some(Err(e)) => return rsx! { Callout { text: e.message, error: true } },
        Some(Ok(s)) => s,
    };
    let n = util::number;
    let tile = |label: String, value: String, sub: String| {
        rsx! {
            div { class: "rounded-lg border bg-card px-4 py-3",
                p { class: "text-xs text-muted-foreground", "{label}" }
                p { class: "mt-1 text-xl font-semibold tabular-nums", "{value}" }
                if !sub.is_empty() {
                    p { class: "mt-0.5 text-[11px] text-muted-foreground tabular-nums", "{sub}" }
                }
            }
        }
    };
    let t = s.total.clone();
    let avg = if t.runs > 0 { t.seconds / t.runs } else { 0 };
    // Token-Tabelle: Zeilen × Zeiträume
    let win = |key: &str| s.windows.iter().find(|w| w.key == key).map(|w| w.sums.clone()).unwrap_or_default();
    let cols = [win("today"), win("7d"), win("30d"), win("all")];
    type Getter = fn(&nrzx_shared::workspace::TokenSums) -> String;
    let rows: Vec<(String, bool, Getter)> = vec![
        (tr("Läufe"), false, |x| util::number(x.runs)),
        (tr("Eingabe gesamt"), true, |x| util::number(x.input_tokens)),
        (tr("davon neu"), false, |x| util::number(x.fresh_input())),
        (tr("davon Cache gelesen"), false, |x| util::number(x.cache_read_tokens)),
        (tr("davon Cache geschrieben"), false, |x| util::number(x.cache_write_tokens)),
        (tr("Ausgabe gesamt"), true, |x| util::number(x.output_tokens)),
        (tr("davon Denken"), false, |x| util::number(x.reasoning_tokens)),
        (tr("Tokens gesamt"), true, |x| util::number(x.total_tokens())),
        (tr("Kosten"), true, |x| format!("$ {:.4}", x.cost_usd)),
        (tr("Laufzeit"), false, |x| duration(x.seconds)),
    ];
    let th = "px-2 py-1.5 text-right font-medium text-muted-foreground whitespace-nowrap";
    let td = "px-2 py-1.5 text-right tabular-nums whitespace-nowrap";
    rsx! {
        div { class: "grid grid-cols-2 gap-2 sm:grid-cols-4",
            {tile(tr("Projekte"), n(s.projects), String::new())}
            {tile(tr("Threads"), n(s.threads), trf("{0} gerade aktiv", &[&n(s.threads_active)]))}
            {tile(tr("Subagenten"), n(s.subagents), String::new())}
            {tile(tr("Läufe"), n(t.runs), trf("{0} fertig · {1} Fehler · {2} gestoppt", &[&n(s.runs_done), &n(s.runs_error), &n(s.runs_stopped)]))}
            {tile(tr("Nachrichten"), n(s.user_messages + s.agent_messages), trf("{0} von dir · {1} vom Agenten", &[&n(s.user_messages), &n(s.agent_messages)]))}
            {tile(tr("Werkzeugaufrufe"), n(s.tool_calls), String::new())}
            {tile(tr("Laufzeit"), duration(t.seconds), trf("Ø {0} je Lauf", &[&duration(avg)]))}
            {tile(tr("Kosten"), cost(t.cost_usd), trf("{0} Läufe heute", &[&n(s.runs_today)]))}
        }
        p { class: "mt-5 mb-2 text-sm font-medium", {tr("Tokens")} }
        div { class: "overflow-x-auto rounded-lg border",
            table { class: "w-full text-xs",
                thead { class: "border-b bg-muted/40",
                    tr {
                        th { class: "px-2 py-1.5 text-left font-medium text-muted-foreground", "" }
                        th { class: th, {tr("Heute")} }
                        th { class: th, {tr("7 Tage")} }
                        th { class: th, {tr("30 Tage")} }
                        th { class: th, {tr("Gesamt")} }
                    }
                }
                tbody {
                    for (label, strong, get) in rows {
                        tr { class: "border-b last:border-0",
                            td { class: if strong { "px-2 py-1.5 font-medium whitespace-nowrap" } else { "px-2 py-1.5 pl-5 text-muted-foreground whitespace-nowrap" }, "{label}" }
                            for c in cols.iter() {
                                td { class: if strong { "{td} font-medium" } else { "{td} text-muted-foreground" }, {get(c)} }
                            }
                        }
                    }
                }
            }
        }
        p { class: "mt-1.5 text-[11px] text-muted-foreground",
            {tr("Eingabe enthält Tokens aus dem Cache, Ausgabe die Denk-Tokens. Kosten melden nur Claude und OpenCode; Cache- und Denk-Tokens werden erst ab dieser Version gezählt.")}
        }
        if !s.agents.is_empty() {
            p { class: "mt-5 mb-2 text-sm font-medium", {tr("Je Agent")} }
            div { class: "overflow-x-auto rounded-lg border",
                table { class: "w-full text-xs",
                    thead { class: "border-b bg-muted/40",
                        tr {
                            th { class: "px-2 py-1.5 text-left font-medium text-muted-foreground", {tr("Agent")} }
                            th { class: th, {tr("Threads")} }
                            th { class: th, {tr("Läufe")} }
                            th { class: th, {tr("Eingabe")} }
                            th { class: th, {tr("Cache gelesen")} }
                            th { class: th, {tr("Ausgabe")} }
                            th { class: th, {tr("Denken")} }
                            th { class: th, {tr("Kosten")} }
                            th { class: th, {tr("Laufzeit")} }
                        }
                    }
                    tbody {
                        for a in s.agents.iter().cloned() {
                            tr { key: "{a.agent.key()}", class: "border-b last:border-0",
                                td { class: "px-2 py-1.5",
                                    div { class: "flex items-center gap-2 whitespace-nowrap",
                                        crate::pages::agents::AgentIcon { kind: a.agent, size: 14 }
                                        {a.agent.name()}
                                    }
                                }
                                td { class: td, {n(a.threads)} }
                                td { class: td, {n(a.sums.runs)} }
                                td { class: td, {n(a.sums.input_tokens)} }
                                td { class: td, {n(a.sums.cache_read_tokens)} }
                                td { class: td, {n(a.sums.output_tokens)} }
                                td { class: td, {n(a.sums.reasoning_tokens)} }
                                td { class: td, {cost(a.sums.cost_usd)} }
                                td { class: td, {duration(a.sums.seconds)} }
                            }
                        }
                    }
                }
            }
        }
        if !s.models.is_empty() {
            p { class: "mt-5 mb-2 text-sm font-medium", {tr("Je Modell")} }
            div { class: "overflow-x-auto rounded-lg border",
                table { class: "w-full text-xs",
                    thead { class: "border-b bg-muted/40",
                        tr {
                            th { class: "px-2 py-1.5 text-left font-medium text-muted-foreground", {tr("Modell")} }
                            th { class: th, {tr("Läufe")} }
                            th { class: th, {tr("Eingabe")} }
                            th { class: th, {tr("Cache gelesen")} }
                            th { class: th, {tr("Ausgabe")} }
                            th { class: th, {tr("Kosten")} }
                            th { class: th, {tr("Ø Tokens/Lauf")} }
                        }
                    }
                    tbody {
                        for m in s.models.iter().cloned() {
                            tr { key: "{m.agent.key()}-{m.model}", class: "border-b last:border-0",
                                td { class: "px-2 py-1.5",
                                    div { class: "flex items-center gap-2 whitespace-nowrap",
                                        crate::pages::agents::AgentIcon { kind: m.agent, size: 14 }
                                        {crate::pages::agents::model_label(m.agent, &m.model)}
                                    }
                                }
                                td { class: td, {n(m.sums.runs)} }
                                td { class: td, {n(m.sums.input_tokens)} }
                                td { class: td, {n(m.sums.cache_read_tokens)} }
                                td { class: td, {n(m.sums.output_tokens)} }
                                td { class: td, {cost(m.sums.cost_usd)} }
                                td { class: td, {n(if m.sums.runs > 0 { m.sums.total_tokens() / m.sums.runs } else { 0 })} }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Kopfzeile

// ------------------------------------------------------------------ Kopfleiste des Chats

pub(super) const TOP_BTN: &str = "grid size-8 place-items-center rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-30 [&_svg]:size-4";

/// Kopfleiste im mittleren Paneel: links (leise) wo man ist, rechts im Thread Browser, Sprechen
/// bzw. „Mute“ und „Auflegen“, die rechte Spalte (Umgebung, Git, Dateien …) und `>_` (das CLI des
/// Agenten). `edge`: Paneel liegt am rechten Fensterrand (Platz für die Fensterknöpfe unter Windows).
/// `top`: keine Reiter darüber, die Leiste liegt oben am Fenster – in der Mac-App dann so hoch, dass
/// ihre Mitte auf der Höhe der Ampel liegt.
#[component]
pub fn CenterBar(thread: Option<i64>, edge: bool, top: bool, #[props(default)] inline: bool) -> Element {
    let route = use_route::<Route>();
    let right_on = thread.is_some() && if narrow() { MOBILE_RIGHT() } else { right_open() };
    let active = |on: bool| if on { "bg-accent text-foreground" } else { "" };
    rsx! {
        header {
            class: format!(
                "flex h-14 shrink-0 items-center gap-2 px-3 {} {} {}",
                if edge { "[.nrz-win_&]:pr-[150px]" } else { "" },
                if top { "nrx-topbar" } else { "" },
                // Im Reiter über dem Paneel (eingeklappt): so breit wie er, Höhe vom Reiter
                if inline { "!h-full min-w-0 flex-1 pr-1.5" } else { "" },
            ),
            "data-drag": "1",
            button { class: "{TOP_BTN} md:hidden", title: tr("Menü"), onclick: move |_| *MOBILE_NAV.write() = true,
                icons::Menu {}
            }
            // Projekt und Titel stehen in der Seitenleiste – hier nur schmal (ohne Seitenleiste)
            div { class: "flex min-w-0 flex-1 items-center gap-2 text-muted-foreground",
                div { class: "md:hidden", Breadcrumb { route: route.clone() } }
            }
            if let Some(id) = thread {
                div { class: "flex shrink-0 items-center gap-1",
                    // Ohne Reiter: „+“ (neuer Chat) hier
                    if !super::tabs::has_tabs(id) {
                        super::tabs::NewChatButton { thread_id: id, top: true }
                    }
                    super::cli::CliToggle { thread_id: id }
                    super::tabs::TerminalButton { thread_id: id }
                    BrowserButton { key: "{id}", thread_id: id }
                    span { class: "mx-1 h-5 w-px bg-border" }
                    if RIGHT_FEATURE() {
                        button {
                            class: "{TOP_BTN} {active(right_on)}",
                            title: tr("Umgebung: Änderungen, Git, Dateien, Notizen"),
                            onclick: move |_| {
                                if narrow() {
                                    let v = !MOBILE_RIGHT();
                                    *MOBILE_RIGHT.write() = v;
                                } else {
                                    let open = (*RIGHT_FEATURE.peek() && *RIGHT.peek()) || *RIGHT_TEMP.peek();
                                    if *RIGHT_TEMP.peek() {
                                        *RIGHT_TEMP.write() = false;
                                    }
                                    if open == *RIGHT.peek() {
                                        toggle(&RIGHT, "nrz-right");
                                    }
                                }
                            },
                            if right_on {
                                icons::PanelRightClose {}
                            } else {
                                icons::PanelRightOpen {}
                            }
                        }
                    }

                }
            }
        }
    }
}

/// Globus oben im Chat: öffnet den integrierten Browser – mit der Seite des Chats, sobald im
/// Arbeitsordner ein Server lauscht (alle 15 Sekunden nachgesehen), sonst leer zum Eintippen.
#[component]
fn BrowserButton(thread_id: i64) -> Element {
    let mut tick = use_signal(|| 0u64);
    let ports = use_resource(move || async move {
        let _ = tick();
        api::get::<Vec<nrzx_shared::workspace::PreviewPort>>(&format!("/previews?threadId={thread_id}")).await.unwrap_or_default()
    });
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(15_000).await;
            tick += 1;
        }
    });
    let port = ports.read().clone().unwrap_or_default().into_iter().find(|p| p.this_thread).map(|p| p.port);
    let open = super::views::active(thread_id) == super::views::View::Browser;
    rsx! {
        button {
            class: if open { format!("{TOP_BTN} bg-accent text-foreground") } else { TOP_BTN.to_string() },
            title: match port {
                Some(p) => trf("Browser: Seite des Chats (:{0})", &[&p]),
                None => tr("Browser: Seite oder Datei ansehen"),
            },
            // Reiter „Browser“ zeigen (bzw. zurück zum Chat)
            onclick: move |_| {
                if super::views::active(thread_id) == super::views::View::Browser {
                    super::views::activate(thread_id, super::views::View::Chat);
                } else {
                    let target = match port {
                        Some(p) => super::browser::Target::Port(p, "/".into()),
                        None => super::browser::Target::File("index.html".into()),
                    };
                    super::views::show_browser(thread_id, target);
                }
            },
            span { class: "relative",
                icons::Globe {}
                if port.is_some() {
                    span { class: "absolute -right-0.5 -top-0.5 size-1.5 rounded-full bg-success" }
                }
            }
        }
    }
}

/// „Projekt / Thread-Titel“ bzw. der Name der Seite.
#[component]
fn Breadcrumb(route: Route) -> Element {
    let sep = rsx! { span { class: "text-muted-foreground/60", "/" } };
    match route {
        Route::ThreadPage { id } => {
            let t = THREADS.read().get(&id).cloned();
            let title = t.as_ref().map(thread_title).unwrap_or_else(|| trf("Thread #{0}", &[&id]));
            let project = t.as_ref().and_then(|t| t.project_id.zip(t.project_name.clone()));
            rsx! {
                div { class: "flex min-w-0 items-center gap-2 text-[15px]",
                    if let Some((pid, name)) = project {
                        Link { to: Route::ProjectPage { id: pid }, class: "hidden min-w-0 shrink items-center gap-2 text-muted-foreground hover:text-foreground sm:flex",
                            ProjectBadge { name: Some(name.clone()), size: 18 }
                            span { class: "truncate", "{name}" }
                        }
                        span { class: "hidden sm:inline", {sep} }
                    }
                    span { class: "truncate font-semibold", "{title}" }
                }
            }
        }
        Route::ProjectPage { id } => {
            let name = PROJECTS.read().get(&id).map(|p| p.name.clone()).unwrap_or_default();
            rsx! {
                div { class: "flex min-w-0 items-center gap-2 text-[15px]",
                    Link { to: Route::Projects {}, class: "text-muted-foreground hover:text-foreground", {tr("Projekte")} }
                    {sep}
                    ProjectBadge { name: Some(name.clone()), size: 18 }
                    span { class: "truncate font-semibold", "{name}" }
                }
            }
        }
        other => {
            let label = match other {
                Route::Home {} => tr("Neuer Thread"),
                Route::Projects {} => tr("Projekte"),
                Route::Settings { .. } => tr("Einstellungen"),
                _ => String::new(),
            };
            rsx! { span { class: "truncate text-[15px] font-semibold", "{label}" } }
        }
    }
}
