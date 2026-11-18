//! Kleine Bausteine des Arbeitsbereichs: Projekt-Abzeichen, kurze Zeitangaben („7m“),
//! Thread-Zustand, Abschnitts-Navigation der Einstellungen, Ein-/Ausblenden der Spalten (im
//! Browser gemerkt), Laden von Threads und Projekten, Sekundentakt.

use crate::Route;
use crate::api;
use crate::i18n::tr;
use crate::state::{PROJECTS, THREADS};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::{ProjectSummary, ThreadStatus, ThreadSummary};

// ------------------------------------------------------------------ Spalten und Dialoge

/// Linke Seitenleiste (Desktop) sichtbar.
pub static LEFT: GlobalSignal<bool> = Signal::global(|| pref("nrz-left", true));
/// Rechte Spalte der Thread-Seite (Desktop) sichtbar.
/// Startet immer eingeklappt (der Knopf oben rechts klappt sie auf).
pub static RIGHT: GlobalSignal<bool> = Signal::global(|| false);
/// Rechte Spalte überhaupt anbieten (Einstellungen → Erscheinungsbild; Standard an – das Panel mit
/// Umgebung, Lokale Server, Nutzung …): dann gibt es oben rechts einen Knopf dafür.
pub static RIGHT_FEATURE: GlobalSignal<bool> = Signal::global(|| pref("nrz-feature-right", true));
/// Rechte Spalte vorübergehend offen, obwohl sie abgeschaltet ist (z. B. für /btw).
pub static RIGHT_TEMP: GlobalSignal<bool> = Signal::global(|| false);

/// Rechte Spalte der Thread-Seite jetzt zeigen?
pub fn right_open() -> bool {
    (RIGHT_FEATURE() && RIGHT()) || RIGHT_TEMP()
}

/// Rechte Spalte schliessen (die vorübergehende bzw. die eingeschaltete).
pub fn close_right() {
    if *RIGHT_TEMP.peek() {
        *RIGHT_TEMP.write() = false;
    } else {
        toggle(&RIGHT, "nrz-right");
    }
}
/// Handy: Seitenleiste als Sheet offen.
pub static MOBILE_NAV: GlobalSignal<bool> = Signal::global(|| false);
/// Handy: rechte Spalte als Sheet offen.
pub static MOBILE_RIGHT: GlobalSignal<bool> = Signal::global(|| false);
/// Dialog „Projekt hinzufügen“ offen (aus der Seitenleiste, der Startseite und der Projektliste).
pub static ADD_PROJECT: GlobalSignal<bool> = Signal::global(|| false);
/// Vorgewähltes Projekt für einen neuen Thread (Startseite). Bleibt gewählt – auch für den
/// nächsten neuen Chat und nach dem Neustart (gespeichert von `remember_new_thread_project`).
pub static NEW_THREAD_PROJECT: GlobalSignal<Option<i64>> =
    Signal::global(|| util::storage_get(NEW_THREAD_PROJECT_KEY).and_then(|v| v.parse().ok()));
const NEW_THREAD_PROJECT_KEY: &str = "nrz-new-thread-project";

/// Gewähltes Projekt für neue Threads speichern, sobald es sich ändert (einmal im Layout).
pub fn remember_new_thread_project() {
    use_effect(|| match NEW_THREAD_PROJECT() {
        Some(id) => util::storage_set(NEW_THREAD_PROJECT_KEY, &id.to_string()),
        None => util::storage_remove(NEW_THREAD_PROJECT_KEY),
    });
}
/// Sekundentakt für „Arbeitet 25s“ (nur kleine Komponenten lesen ihn).
pub static NOW: GlobalSignal<i64> = Signal::global(util::now);
/// Ansichten neu laden (Arbeitsordner geändert: Commit, Branch, Worktree).
pub static GIT_REFRESH: GlobalSignal<u64> = Signal::global(|| 0);

fn pref(key: &str, default: bool) -> bool {
    match util::storage_get(key).as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => default,
    }
}

/// Spalte umschalten und merken.
pub fn toggle(signal: &'static GlobalSignal<bool>, key: &str) {
    let v = !*signal.peek();
    *signal.write() = v;
    util::storage_set(key, if v { "1" } else { "0" });
}

/// Schmaler Bildschirm (Handy, unter `lg`)?
pub fn narrow() -> bool {
    web_sys::window().and_then(|w| w.inner_width().ok()).and_then(|v| v.as_f64()).is_some_and(|w| w < 1024.0)
}

pub fn git_refresh() {
    *GIT_REFRESH.write() += 1;
}

// ------------------------------------------------------------------ Laden

/// Threads (`GET /api/v1/threads`, Bereich agents) nach `state::THREADS`. Versteht eine Liste
/// oder `{ items: [...] }`.
pub async fn load_threads() {
    let Ok(v) = api::get::<serde_json::Value>("/threads").await else { return };
    let list = if v.is_array() { v } else { v["items"].clone() };
    if let Ok(items) = serde_json::from_value::<Vec<ThreadSummary>>(list) {
        *THREADS.write() = items.into_iter().map(|t| (t.id, t)).collect();
    }
}

/// Projekte (`GET /api/v1/projects`) nach `state::PROJECTS`.
pub async fn load_projects() {
    if let Ok(items) = api::get::<Vec<ProjectSummary>>("/projects").await {
        *PROJECTS.write() = items.into_iter().map(|p| (p.id, p)).collect();
    }
}

// ------------------------------------------------------------------ Zeit und Zustand

/// „jetzt“, „7m“, „2h“, „3d“, „5w“.
pub fn short_ago(ts: i64) -> String {
    let d = (util::now() - ts).max(0);
    if d < 60 {
        tr("jetzt")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86_400 {
        format!("{}h", d / 3600)
    } else if d < 14 * 86_400 {
        format!("{}d", d / 86_400)
    } else {
        format!("{}w", d / (7 * 86_400))
    }
}

/// Dauer kurz: „25s“, „3m 10s“, „1h 4m“.
pub fn duration(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

/// Sekunden seit `since` – liest den Sekundentakt.
#[component]
pub fn Elapsed(since: i64) -> Element {
    let d = NOW() - since;
    rsx! { "{duration(d)}" }
}

/// Zustand eines Threads rechts oben in der Liste: Spinner + „Arbeitet 25s“, „Wartet 18s“,
/// „Fertig“, „Fehler“ – sonst die Zeit seit der letzten Aktivität.
#[component]
pub fn ThreadState(thread: ThreadSummary) -> Element {
    let since = thread.run_started_at.unwrap_or(thread.last_activity_at);
    match thread.status {
        ThreadStatus::Working => rsx! {
            span { class: "inline-flex shrink-0 items-center gap-1 text-xs text-info",
                icons::LoaderCircle { class: "size-3.5 animate-spin" }
                {tr("Arbeitet")}
                " "
                Elapsed { since }
            }
        },
        ThreadStatus::Waiting => rsx! {
            span { class: "inline-flex shrink-0 items-center gap-1 text-xs text-muted-foreground",
                icons::CircleDashed { class: "size-3.5 animate-[spin_3s_linear_infinite]" }
                {tr("Wartet")}
                " "
                Elapsed { since }
            }
        },
        ThreadStatus::Done => rsx! {
            span { class: "inline-flex shrink-0 items-center gap-1 text-xs font-medium text-success",
                icons::CircleCheck { class: "size-3.5" }
                {tr("Fertig")}
            }
        },
        ThreadStatus::Error => rsx! {
            span { class: "inline-flex shrink-0 items-center gap-1 text-xs font-medium text-destructive",
                icons::CircleAlert { class: "size-3.5" }
                {tr("Fehler")}
            }
        },
        _ => rsx! {
            span { class: "shrink-0 text-xs tabular-nums text-muted-foreground", title: util::datetime(thread.last_activity_at), {short_ago(thread.last_activity_at)} }
        },
    }
}

// ------------------------------------------------------------------ Projekt-Abzeichen

/// Kürzel eines Projekts: Anfangsbuchstaben der ersten zwei Wörter, sonst die ersten zwei Zeichen.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let s: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

/// Farbton (0–359) aus dem Namen.
pub fn hue(name: &str) -> u32 {
    name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360
}

/// Farbiges Kürzel eines Projekts (wie „PN“ im Bild); ohne Projekt ein gestricheltes Kästchen.
#[component]
pub fn ProjectBadge(name: Option<String>, #[props(default = 18)] size: u32) -> Element {
    let font = (size as f32 * 0.5).round() as u32;
    match name {
        Some(n) => {
            let h = hue(&n);
            rsx! {
                span {
                    class: "inline-grid shrink-0 place-items-center rounded-[5px] font-bold tracking-tight",
                    style: "width: {size}px; height: {size}px; font-size: {font}px; background: hsl({h} 65% 50% / 0.18); color: hsl({h} 80% 68%)",
                    {initials(&n)}
                }
            }
        }
        None => rsx! {
            span { class: "inline-block shrink-0 rounded-[5px] border border-dashed border-muted-foreground/60", style: "width: {size}px; height: {size}px" }
        },
    }
}

/// Zustand eines Projekts als kleines Abzeichen.
#[component]
pub fn ProjectStatus(#[props(into)] status: String) -> Element {
    match status.as_str() {
        "cloning" => rsx! {
            span { class: "inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium text-muted-foreground",
                icons::LoaderCircle { class: "size-3 animate-spin" }
                {tr("Wird geklont")}
            }
        },
        "error" => rsx! {
            span { class: "inline-flex items-center gap-1 rounded-full border border-destructive/40 bg-destructive/10 px-2 py-0.5 text-[11px] font-medium text-destructive",
                icons::CircleAlert { class: "size-3" }
                {tr("Fehler")}
            }
        },
        _ => rsx! {},
    }
}

// ------------------------------------------------------------------ Einstellungen

/// Eintrag der Abschnitts-Navigation.
#[derive(Clone, PartialEq)]
pub struct NavEntry {
    pub label: String,
    pub to: Route,
    pub active: bool,
    pub icon: Element,
}

/// Linke Navigation einer Einstellungsseite (auf dem Handy waagrecht) – wie in nrzGit.
#[component]
pub fn SectionNav(entries: Vec<NavEntry>, #[props(into, optional)] heading: Option<String>) -> Element {
    rsx! {
        nav { class: "flex gap-1 overflow-x-auto lg:flex-col lg:overflow-visible",
            if let Some(h) = heading {
                p { class: "hidden px-2.5 pb-1 text-xs font-medium uppercase tracking-wider text-muted-foreground lg:block", "{h}" }
            }
            for e in entries {
                Link {
                    to: e.to.clone(),
                    class: if e.active {
                        "flex shrink-0 items-center gap-2.5 rounded-md bg-accent px-2.5 py-1.5 text-sm font-medium [&_svg]:size-4"
                    } else {
                        "flex shrink-0 items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground hover:bg-accent/50 hover:text-foreground [&_svg]:size-4"
                    },
                    {e.icon.clone()}
                    "{e.label}"
                }
            }
        }
    }
}

/// Spinner im Knopf, solange `busy`.
#[component]
pub fn BusyIcon(busy: bool) -> Element {
    rsx! {
        if busy {
            icons::LoaderCircle { class: "size-4 animate-spin" }
        }
    }
}

/// Externer Link (neuer Tab, ohne Referrer).
#[component]
pub fn ExternalLink(#[props(into)] href: String, #[props(into, optional)] class: Option<String>, children: Element) -> Element {
    rsx! {
        a { href: "{href}", target: "_blank", rel: "noopener noreferrer", class: class.unwrap_or_default(), {children} }
    }
}

/// Adresse in neuem Tab öffnen.
pub fn open_tab(url: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.open_with_url_and_target(url, "_blank");
    }
}

/// Anzeigename eines Threads.
pub fn thread_title(t: &ThreadSummary) -> String {
    if t.title.trim().is_empty() { tr("Neuer Thread") } else { t.title.clone() }
}
