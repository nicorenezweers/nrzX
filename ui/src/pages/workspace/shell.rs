//! Linkes Paneel von nrzX: oben „nrzX“ neben der Ampel, Suche und „+ Neue Aufgabe“, die Karte des
//! Agenten im Fokus (Agent, Konto, Zustand), darunter die Aufgaben (Threads) mit Zustandskreis,
//! Titel und einer Zeile darunter (Anfang der letzten Antwort bzw. was gerade passiert). Unten das
//! Projekt für neue Aufgaben (Auswahl; im Thread dessen Projekt, Branch und Titel), ein Update der
//! App (nur, wenn eines da ist) und wie viel vom Abo verbraucht ist.

use super::layout::{PENDING_KEY, THREAD_MENU};
use super::parts::*;
use crate::components::overlay::*;
use crate::i18n::{tr, trf};
use crate::state::{self, tone, Tone, PROJECTS, THREADS};
use crate::Route;
use dioxus::prelude::*;
use nrzx_shared::workspace::{ImportProject, NrzGitRepo};
use nrzx_shared::{ProjectSummary, ThreadStatus, ThreadSummary};

/// Zeile unter dem Titel: was passiert bzw. Anfang der letzten Antwort.
fn subtitle(t: &ThreadSummary) -> String {
    if t.status == ThreadStatus::Waiting && t.children_running > 0 {
        return trf("{0} Subagenten · warten", &[&t.children_running]);
    }
    if !t.snippet.is_empty() {
        return t.snippet.clone();
    }
    // Das Projekt steht schon oben: hier der Branch bzw. wann zuletzt etwas passierte
    match (&t.project_name, t.branch.is_empty()) {
        (Some(_), false) => format!("{} · {}", t.branch, short_ago(t.last_activity_at)),
        _ => short_ago(t.last_activity_at),
    }
}

/// Kreis vor einer Aufgabe: dreht sich beim Arbeiten, Punkt bei Ungelesenem, Haken bei Fertigem
/// (grün) und Abgelegtem, Sprechblase mit „?“ bei einer Rückfrage, Stopp bei Abgebrochenem.
#[component]
fn StatusRing(thread: ThreadSummary) -> Element {
    let tone = tone(&thread);
    match thread.status {
        _ if tone == Tone::Asking => rsx! {
            span { class: "nrx-ic-ask grid place-items-center", icons::MessageCircleQuestion { class: "size-[15px]" } }
        },
        ThreadStatus::Working | ThreadStatus::Waiting => rsx! {
            span { class: "block size-[15px] rounded-full border-2 border-dashed border-foreground/70 animate-[spin_4s_linear_infinite]" }
        },
        ThreadStatus::Error => rsx! { icons::CircleX { class: "size-[15px] text-destructive" } },
        ThreadStatus::Settled => rsx! { icons::CircleCheck { class: "size-[15px] text-muted-foreground" } },
        _ if thread.unread => rsx! {
            span { class: "grid size-[15px] place-items-center rounded-full border-2 border-foreground/80",
                span { class: "size-1.5 rounded-full bg-foreground" }
            }
        },
        _ if tone == Tone::Done => rsx! {
            span { class: "nrx-ic-done grid place-items-center", icons::CircleCheck { class: "size-[15px]" } }
        },
        _ if tone == Tone::Failed => rsx! { icons::CircleStop { class: "size-[15px] text-destructive" } },
        _ => rsx! { span { class: "block size-[15px] rounded-full border-2 border-muted-foreground/70" } },
    }
}

#[component]
pub fn TaskSidebar() -> Element {
    let route = use_route::<Route>();
    let current = match route {
        Route::ThreadPage { id } => Some(id),
        Route::Home {} => state::pending_arrived(),
        _ => None,
    };
    let mut query = use_signal(String::new);
    // Suche: ⌘K (siehe AppLayout)
    let searching = super::tabs::SEARCH_OPEN();
    use_effect(move || {
        if !super::tabs::SEARCH_OPEN() {
            query.set(String::new());
        }
    });
    let q = query().trim().to_lowercase();
    let mut list: Vec<ThreadSummary> = THREADS
        .read()
        .values()
        .filter(|t| t.parent_id.is_none() && t.bot_id.is_none() && !t.research && t.discuss_of.is_none() && !t.hidden)
        .filter(|t| q.is_empty() || t.title.to_lowercase().contains(&q) || t.snippet.to_lowercase().contains(&q) || t.project_name.as_deref().unwrap_or("").to_lowercase().contains(&q))
        .cloned()
        .collect();
    // Angeheftete zuerst, dann laufende, dann nach letzter Aktivität (bzw. festem Platz)
    list.sort_by_key(|x| std::cmp::Reverse((x.pinned, x.status.is_active(), x.sort_key.unwrap_or(x.last_activity_at), x.id)));
    let pending = state::pending_visible().filter(|_| matches!(route, Route::Home {}) && q.is_empty());
    // Nutzung neu laden: beim Start, wenn sich die Zahl laufender Agenten ändert, jede Minute
    let running = use_memo(|| THREADS.read().values().filter(|t| t.status.is_active()).count());
    use_effect(move || {
        let _ = running();
        crate::pages::agents::refresh_usage(false);
    });
    use_future(|| async {
        loop {
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            crate::pages::agents::refresh_usage(false);
        }
    });
    let head = crate::pages::agents::usage_headline();

    rsx! {
        if searching {
            div { class: "shrink-0 px-3 pt-3",
                input {
                    class: "h-9 w-full rounded-lg border bg-transparent px-3 text-[14px] outline-none placeholder:text-muted-foreground focus:border-ring",
                    r#type: "search",
                    autofocus: true,
                    placeholder: tr("Aufgaben durchsuchen"),
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Escape {
                            *super::tabs::SEARCH_OPEN.write() = false;
                        }
                    },
                }
            }
        }
        // Aufgaben
        // Oben etwas Luft bis zur ersten Aufgabe
        div { class: "flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pt-3 pb-2",
            if let Some(p) = pending {
                div { key: "{PENDING_KEY}", class: "flex min-w-0 items-start gap-3 rounded-xl px-3 py-2.5",
                    div { class: "pt-[3px]", span { class: "block size-[15px] rounded-full border-2 border-dashed border-foreground/70 animate-[spin_4s_linear_infinite]" } }
                    div { class: "min-w-0 flex-1",
                        p { class: "truncate text-[15px] font-medium", "{p.title}" }
                        p { class: "truncate text-[13px] text-muted-foreground", {tr("Startet …")} }
                    }
                }
            }
            if list.is_empty() {
                p { class: "px-3 py-8 text-center text-sm text-muted-foreground",
                    if q.is_empty() { {tr("Noch keine Aufgaben.")} } else { {tr("Keine Treffer.")} }
                }
            }
            for t in list {
                TaskRow { key: "{t.id}", active: current == Some(t.id), thread: t.clone() }
            }
        }
        // Unten ein Update der App (nur, wenn eines da ist) und die Nutzung – ein Klick zeigt die
        // Limits je Konto
        div { class: "shrink-0 px-3 pt-2 pb-3",
            super::update::UpdateButton {}
            Dropdown {
                side: Side::Top,
                class: "nrx-pop w-[300px] p-1.5",
                trigger_class: "flex items-center gap-2 rounded-lg px-2 py-1 text-[12.5px] text-muted-foreground transition-colors hover:bg-white/[0.06] hover:text-foreground",
                // Die beiden Limits, die zählen: 5 Stunden und Woche, in der Farbe des Füllstands
                trigger: rsx! {
                    icons::Gauge { class: "size-3.5 shrink-0" }
                    match head {
                        Some((h5, wk)) => rsx! {
                            span { class: "flex items-center gap-1.5 tabular-nums",
                                if let Some(v) = h5 {
                                    span { "5 h " span { class: "font-medium", style: "color: {crate::pages::agents::usage_level_color(v)}", {format!("{v:.0} %")} } }
                                }
                                if h5.is_some() && wk.is_some() {
                                    span { class: "opacity-40", "·" }
                                }
                                if let Some(v) = wk {
                                    span { {tr("Woche")} " " span { class: "font-medium", style: "color: {crate::pages::agents::usage_level_color(v)}", {format!("{v:.0} %")} } }
                                }
                            }
                        },
                        None => rsx! { span { {tr("Nutzung")} } },
                    }
                },
                crate::pages::agents::UsageSection {}
            }
        }
    }
}

/// Projekt für die nächste neue Aufgabe (links über dem Eingabefeld der Startseite, gegenüber der
/// Modell-Auswahl): durchsuchbar, „Ohne Projekt“, „Projekt hinzufügen …“.
#[component]
pub fn ProjectChooser() -> Element {
    let mut projects: Vec<ProjectSummary> = PROJECTS.read().values().cloned().collect();
    projects.sort_by_key(|x| std::cmp::Reverse(x.last_activity_at));
    let selected = NEW_THREAD_PROJECT().filter(|id| projects.iter().any(|p| p.id == *id));
    let current = selected.and_then(|id| projects.iter().find(|p| p.id == id).cloned());
    let pq = use_signal(String::new);
    let found: Vec<ProjectSummary> = projects.iter().filter(|p| search_hit(&pq(), &[&p.name, &p.repo])).cloned().collect();
    let first_found = found.first().map(|p| p.id);
    let (name, branch) = match &current {
        Some(p) => (p.name.clone(), p.default_branch.clone()),
        None => (tr("Ohne Projekt"), String::new()),
    };
    rsx! {
        Dropdown {
            side: Side::Top,
            class: "nrx-pop max-h-80 w-72 overflow-y-auto",
            trigger_class: "flex min-w-0 items-center gap-1.5 rounded-lg px-1.5 py-1 text-[13px] text-muted-foreground transition-colors hover:bg-white/[0.06] hover:text-foreground",
            trigger: rsx! {
                ProjectBadge { name: current.as_ref().map(|p| p.name.clone()), size: 16 }
                span { class: "max-w-56 truncate text-foreground/90", "{name}" }
                if !branch.is_empty() {
                    span { class: "truncate font-mono text-[12px]", "{branch}" }
                }
                icons::ChevronsUpDown { class: "size-3.5 shrink-0 opacity-60" }
            },
            MenuSearch {
                value: pq,
                placeholder: tr("Projekt oder Repository suchen"),
                onenter: move |_| {
                    if let Some(id) = first_found {
                        *NEW_THREAD_PROJECT.write() = Some(id);
                    }
                },
            }
            MenuLabel { {tr("Projekt für neue Aufgaben")} }
            if pq().trim().is_empty() {
                MenuItem { active: selected.is_none(), onclick: move |_| *NEW_THREAD_PROJECT.write() = None,
                    ProjectBadge { name: None, size: 16 }
                    span { class: "flex-1", {tr("Ohne Projekt")} }
                    span { class: "text-xs text-muted-foreground", {tr("Scratch-Ordner")} }
                }
                if !projects.is_empty() {
                    MenuSeparator {}
                }
            } else if found.is_empty() {
                p { class: "px-2 py-1.5 text-sm text-muted-foreground", {tr("Kein Projekt gefunden.")} }
            }
            for p in found {
                MenuItem { key: "{p.id}", active: selected == Some(p.id), onclick: move |_| *NEW_THREAD_PROJECT.write() = Some(p.id),
                    ProjectBadge { name: Some(p.name.clone()), size: 16 }
                    span { class: "min-w-0 flex-1 truncate", "{p.name}" }
                    ProjectStatus { status: p.status.clone() }
                }
            }
            NrzGitHits { query: pq }
            MenuSeparator {}
            MenuItem { onclick: move |_| *ADD_PROJECT.write() = true,
                icons::FolderPlus {}
                {tr("Projekt hinzufügen …")}
            }
        }
    }
}

/// Treffer der Suche aus nrzGit, die noch kein Projekt sind: ein Klick übernimmt das Repository
/// (es wird im Hintergrund geklont) und wählt es für die neue Aufgabe. Die Liste kommt bei jedem
/// Öffnen der Auswahl frisch aus nrzGit – neue Repositories sind sofort da.
#[component]
fn NrzGitHits(query: Signal<String>) -> Element {
    let repos = use_resource(|| async { crate::api::get::<Vec<NrzGitRepo>>("/nrzgit/repos?kind=nrzgit").await.ok() });
    let q = query();
    if q.trim().is_empty() {
        return rsx! {};
    }
    let hits: Vec<NrzGitRepo> = match &*repos.read() {
        None => return rsx! { MenuLabel { {tr("nrzGit wird durchsucht …")} } },
        // Nicht verbunden oder nicht erreichbar: nur die Projekte
        Some(None) => return rsx! {},
        Some(Some(list)) => list.iter().filter(|r| r.project_id.is_none() && search_hit(&q, &[&r.full_name, &r.description])).take(8).cloned().collect(),
    };
    if hits.is_empty() {
        return rsx! {};
    }
    rsx! {
        MenuSeparator {}
        MenuLabel { {tr("Aus nrzGit hinzufügen")} }
        for r in hits {
            MenuItem {
                key: "{r.forge}/{r.full_name}",
                onclick: {
                    let (full, forge) = (r.full_name.clone(), r.forge.clone());
                    move |_| add_repo(full.clone(), forge.clone())
                },
                icons::Plus {}
                span { class: "min-w-0 flex-1 truncate",
                    span { class: "text-muted-foreground", "{r.owner}/" }
                    "{r.name}"
                }
                if r.private {
                    span { class: "text-[11px] text-muted-foreground", {tr("Privat")} }
                }
            }
        }
    }
}

/// Repository aus nrzGit als Projekt übernehmen und für die neue Aufgabe wählen (läuft weiter,
/// auch wenn die Auswahl schon zu ist).
fn add_repo(full: String, forge: String) {
    dioxus::core::spawn_forever(async move {
        match crate::api::post::<ProjectSummary, _>("/projects/import", &ImportProject { repo: full, forge }).await {
            Ok(p) => {
                let id = p.id;
                PROJECTS.write().insert(id, p);
                *NEW_THREAD_PROJECT.write() = Some(id);
            }
            Err(e) => toast_error(e),
        }
    });
}

/// Hintergrund einer Zeile: getönt nach [`Tone`] (gewählt kräftiger), sonst wie bisher.
fn row_class(tone: Tone, active: bool) -> String {
    let base = "flex min-w-0 items-start gap-3 rounded-xl px-3 py-2.5 transition-colors";
    let tint = match tone {
        Tone::Done => "nrx-tone nrx-tone-done",
        Tone::Failed => "nrx-tone nrx-tone-fail",
        Tone::Asking => "nrx-tone nrx-tone-ask",
        Tone::Plain if active => "bg-accent/50",
        Tone::Plain => "hover:bg-accent/30",
    };
    let on = if active && tone != Tone::Plain { " nrx-tone-on" } else { "" };
    format!("{base} {tint}{on}")
}

/// Eine Aufgabe: Kreis, Titel, Zeile darunter. Rechtsklick öffnet das Menü des Threads.
#[component]
fn TaskRow(thread: ThreadSummary, active: bool) -> Element {
    let t = thread.clone();
    let menu = thread.clone();
    // Mit Projekt steht das Projekt oben (nicht die Anfrage – die zeigt der Hinweis beim Darüberfahren)
    let request = thread_title(&t);
    let heading = t.project_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| request.clone());
    let sub = if tone(&t) == Tone::Asking {
        tr("Wartet auf deine Antwort")
    } else if t.status.is_active() && t.snippet.is_empty() { trf("Arbeitet · {0}", &[&short_ago(t.run_started_at.unwrap_or(t.last_activity_at))]) } else { subtitle(&t) };
    rsx! {
        div {
            class: "min-w-0",
            oncontextmenu: move |e: MouseEvent| {
                e.prevent_default();
                let p = e.client_coordinates();
                *THREAD_MENU.write() = Some((menu.clone(), p.x, p.y));
            },
        Link {
            to: Route::ThreadPage { id: t.id },
            title: "{request}",
            class: row_class(tone(&t), active),
            div { class: "pt-[3px]", StatusRing { thread: t.clone() } }
            div { class: "min-w-0 flex-1",
                p { class: if t.unread { "truncate text-[15px] font-semibold" } else { "truncate text-[15px] font-medium" }, "{heading}" }
                p { class: "truncate text-[13px] text-muted-foreground", "{sub}" }
            }
            if t.pinned {
                icons::Pin { class: "mt-1 size-3.5 shrink-0 text-muted-foreground" }
            }
        }
        }
    }
}
