//! Rahmen der App (`AppLayout`: Seitenleiste mit Threads nach Zustand, Kopfzeile mit Knöpfen
//! für die Spalten), Anmeldung, Startseite, Projekte, Thread-Seite mit rechter Spalte (Umgebung,
//! Worktree, Branch, Commit, Änderungen, Vorschau, Subagenten) und Terminal, Einstellungen.
//!
//! * `layout` – `AppLayout`, Seitenleiste, Kopfzeile, Benutzermenü, Statistik
//! * `auth` – `Login` (mit Registrieren), `Setup`
//! * `bots` – Bot-Modus: `Bots`, `BotPage`, Liste in der Seitenleiste, Editor (Profil, Gedächtnis, Routinen)
//! * `research` – Deep Research: `Research` (Frage), `ResearchPage` (Paper, Fortschritt, Gespräch), Liste in der Seitenleiste
//! * `home` – `Home` (Begrüssung, Composer)
//! * `projects` – `Projects`, `ProjectPage`, Dialog „Projekt hinzufügen“
//! * `dieter` – Einstellungen der Spracherkennung und des Vorlesens (für „Sprechen“)
//! * `thread` – `ThreadPage` (Ansichten als Reiter: Chat, CLI, Browser, Terminals; rechte Spalte)
//! * `views` – Ansichten eines Threads (Reiter oben), `tabs` – Reihe oben im Fenster
//! * `git` – Branch, Änderungen, Commit, Pull Request
//! * `sidepanel` – rechte Spalte mit Reitern (Dateien, Suche, Übersicht, Notizblock, Team)
//! * `team` – Reiter „Team“: Mitglieder, Agenten der anderen, gemeinsame Nachrichten
//! * `editor` – Dateibaum und Editor („Open in editor“)
//! * `settings`, `admin` – `Settings` mit Abschnitten
//! * `parts` – kleine Bausteine (Projekt-Abzeichen, Zeiten, Spalten, Laden)
//! * `update` – Knopf für ein Update der App unten in der Seitenleiste (nur, wenn eines da ist)

mod auth;
pub mod bots;
pub mod browser;
pub mod cli;
pub mod dieter;
mod editor;
mod git;
mod home;
pub mod layout;
pub mod parts;
mod projects;
pub mod research;
mod settings;
mod shell;
pub mod tabs;
pub mod views;
mod sidepanel;
mod team;
mod thread;
pub mod update;

pub use auth::{Login, Setup};
pub use bots::{BotPage, Bots};
pub use home::Home;
pub use layout::AppLayout;
pub use projects::{ProjectPage, Projects};
pub use research::{Research, ResearchPage};
pub use settings::Settings;
pub use thread::ThreadPage;

use crate::Route;
use crate::i18n::tr;
use dioxus::prelude::*;

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let path = format!("/{}", segments.join("/"));
    rsx! {
        div { class: "flex min-h-screen items-center justify-center bg-background p-6",
            div { class: "max-w-sm text-center",
                img { src: "/static/icon.png", class: "mx-auto mb-4 size-12 rounded-xl", alt: "" }
                h1 { class: "text-xl font-semibold", {tr("Diese Seite gibt es nicht.")} }
                p { class: "mt-2 break-all font-mono text-xs text-muted-foreground", "{path}" }
                Link { to: Route::Home {}, class: "mt-6 inline-flex h-9 items-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground hover:bg-primary/90",
                    icons::House { class: "size-4" }
                    {tr("Zur Startseite")}
                }
            }
        }
    }
}
