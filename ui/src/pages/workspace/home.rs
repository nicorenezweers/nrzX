//! Startseite: Begrüssung, Composer für eine neue Aufgabe (links darüber das Projekt, rechts das
//! Modell), darunter die letzten Aufgaben.

use super::parts::*;
use crate::i18n::{tr, trf};
use crate::state::{ME, PROJECTS};
use dioxus::prelude::*;
use nrzx_shared::ProjectSummary;

/// Begrüssung nach Tageszeit.
fn greeting(name: &str) -> String {
    let hour = js_sys::Date::new_0().get_hours();
    let key = match hour {
        5..=10 => "Guten Morgen, {0}",
        11..=17 => "Hallo, {0}",
        _ => "Guten Abend, {0}",
    };
    trf(key, &[&name])
}

#[component]
pub fn Home() -> Element {
    let me = ME.read().clone();
    let name = me.as_ref().map(|m| m.full_name.split_whitespace().next().unwrap_or(&m.username).to_string()).unwrap_or_default();
    let projects: Vec<ProjectSummary> = PROJECTS.read().values().cloned().collect();
    let selected = NEW_THREAD_PROJECT().filter(|id| projects.iter().any(|p| p.id == *id));
    let not_ready = selected.and_then(|id| projects.iter().find(|p| p.id == id)).is_some_and(|p| p.status != "ready");

    // Gerade gesendeter neuer Thread: schon wie der Chat zeigen. Die Startseite bleibt darunter
    // eingehängt (ihr Composer schliesst das Anlegen ab und wechselt dann in den Thread).
    // Frisch geöffnet: ein Platzhalter von vorhin ist verwaist (sein Composer ist weg)
    use_hook(|| {
        if crate::state::PENDING_THREAD.peek().is_some() {
            spawn(async { *crate::state::PENDING_THREAD.write() = None });
        }
    });
    let pending = crate::state::PENDING_THREAD();
    rsx! {
        if let Some(p) = pending.clone() {
            // Wie die Thread-Seite (mit der rechten Spalte, falls offen): kein Springen beim Wechsel
            div { class: "flex h-full min-h-0",
                div { class: "h-full min-w-0 flex-1", crate::pages::agents::PendingChat { pending: p } }
                if super::parts::right_open() {
                    aside { class: "hidden w-[21rem] shrink-0 border-l bg-sidenav lg:flex" }
                }
            }
        }
        div { class: if pending.is_some() { "hidden" } else { "mx-auto flex min-h-full w-full max-w-3xl flex-col justify-center px-4 py-8" },
            // Begrüssung und Composer mittig im sichtbaren Bereich – sonst nichts
            div { class: "flex flex-col justify-center gap-8",
                div { class: "text-center",
                    h1 { class: "text-2xl font-semibold tracking-tight md:text-3xl", {greeting(&name)} }
                    p { class: "mt-2 text-sm text-muted-foreground", {tr("Woran sollen die Agenten heute arbeiten?")} }
                }
                div { class: "grid gap-3",
                    if not_ready {
                        p { class: "text-xs text-muted-foreground", {tr("Das Projekt wird noch geklont – der Agent startet, sobald es bereit ist.")} }
                    }
                    for sel in std::iter::once(selected) {
                        crate::pages::agents::Composer { key: "{sel:?}", project_id: sel, thread_id: None, status: rsx! { super::shell::ProjectChooser {} } }
                    }
                }
            }
        }
    }
}
