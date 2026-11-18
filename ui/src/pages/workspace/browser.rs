//! Integrierter Browser: rechtes Paneel neben dem Chat, in dem man die Apps sieht, die die Agenten
//! bauen. Oben Adresse (Globus, Pfad bzw. URL – editierbar), „Im Browser öffnen“ und Schliessen,
//! darunter die Seite in einem Rahmen.
//!
//! Ziele:
//! * Server eines Agenten (`localhost:<port>`) – über den Vorschau-Proxy (eigener Port, Token)
//! * Datei im Arbeitsordner (`index.html`, `games/connect4.html`) – über `/raw/<token>/…`: eigener,
//!   leerer Ursprung (Sandbox), ohne Zugriff auf die Sitzung; relative Links der Seite gehen
//! * andere Adressen (`https://…`) direkt (viele Seiten verbieten das Einrahmen)
//!
//! Geöffnet wird er vom Agenten (`open_preview`), über „Lokale Server“, Datei-Chips im Chat oder
//! von Hand (Globus oben im Chat).

use crate::api;
use crate::components::overlay::toast_error;
use crate::i18n::tr;
use crate::util;
use dioxus::prelude::*;
use std::collections::HashMap;
use nrzx_shared::workspace::{PreviewToken, PreviewTokenRequest, RawToken};

/// Was der Browser zeigt.
#[derive(Clone, PartialEq, Debug)]
pub enum Target {
    /// Server eines Agenten: Port und Pfad
    Port(u16, String),
    /// Datei im Arbeitsordner des Threads (relativ)
    File(String),
    /// Beliebige Adresse
    Url(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct BrowserOpen {
    pub thread_id: i64,
    pub target: Target,
    /// Erhöht bei jedem Öffnen (lädt neu, auch bei gleichem Ziel)
    pub rev: u64,
}

/// Offene Browser – jeder Thread hat seinen eigenen (Reiter „Browser“).
pub static BROWSER: GlobalSignal<HashMap<i64, BrowserOpen>> = Signal::global(HashMap::new);

/// Browser eines Threads öffnen bzw. auf ein neues Ziel schicken.
pub fn open(thread_id: i64, target: Target) {
    let rev = BROWSER.peek().get(&thread_id).map(|b| b.rev + 1).unwrap_or(1);
    BROWSER.write().insert(thread_id, BrowserOpen { thread_id, target, rev });
}

pub fn close(thread_id: i64) {
    BROWSER.write().remove(&thread_id);
}

/// Ist der Browser für diesen Thread offen?
pub fn open_for(thread_id: i64) -> bool {
    BROWSER.read().contains_key(&thread_id)
}

/// Eingabe in der Adresszeile → Ziel: `localhost:3000/x`, `:3000`, `https://…`, sonst Datei.
pub fn parse_target(input: &str) -> Option<Target> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let local = lower.trim_start_matches("http://").trim_start_matches("https://");
    for host in ["localhost:", "127.0.0.1:", "0.0.0.0:", ":"] {
        if let Some(rest) = local.strip_prefix(host) {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = digits.parse::<u16>() {
                let path = &rest[digits.len()..];
                return Some(Target::Port(port, if path.is_empty() { "/".into() } else { path.to_string() }));
            }
        }
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(Target::Url(s.to_string()));
    }
    Some(Target::File(s.trim_start_matches("./").trim_start_matches('/').to_string()))
}

/// Text für die Adresszeile.
fn label(t: &Target) -> String {
    match t {
        Target::Port(p, path) => format!("localhost:{p}{path}"),
        Target::File(f) => f.clone(),
        Target::Url(u) => u.clone(),
    }
}

/// Adresse für den Rahmen.
async fn resolve(thread_id: i64, t: &Target, workdir: &str) -> Result<String, String> {
    match t {
        Target::Port(port, path) => {
            let body = PreviewTokenRequest { thread_id, port: *port, path: path.clone() };
            api::post::<PreviewToken, _>("/previews/token", &body).await.map(|t| t.url).map_err(|e| e.message)
        }
        Target::File(f) => {
            let tok = api::action::<RawToken>(&format!("/workspace/{thread_id}/raw-token")).await.map_err(|e| e.message)?;
            let f = relative(f, workdir);
            let path: String = f.split('/').map(|seg| api::enc(seg)).collect::<Vec<_>>().join("/");
            Ok(format!("{}{path}", tok.url))
        }
        Target::Url(u) => Ok(u.clone()),
    }
}

/// Absoluten Pfad im Arbeitsordner relativ machen (Werkzeuge der Agenten melden oft absolute).
fn relative(path: &str, workdir: &str) -> String {
    let wd = workdir.trim_end_matches('/');
    match path.strip_prefix(wd) {
        Some(rest) if !wd.is_empty() => rest.trim_start_matches('/').to_string(),
        _ => path.trim_start_matches("./").to_string(),
    }
}

/// Adresse oder Datei im Browser des Systems (in der App über die App, sonst neuer Tab).
pub fn open_external(url: &str) {
    if util::in_app() {
        util::native(&format!("open-external:{url}"));
    } else {
        super::parts::open_tab(url);
    }
}

/// Rechtes Paneel: Adresszeile und Seite. `workdir` = Arbeitsordner (für „Im Browser öffnen“ bei
/// Dateien und die Anzeige des Pfads).
#[component]
pub fn BrowserPanel(thread_id: i64, workdir: String) -> Element {
    let current = BROWSER.read().get(&thread_id).cloned();
    let mut input = use_signal(String::new);
    let mut src = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let mut reload = use_signal(|| 0u64);
    // Neues Ziel: Adresse auflösen
    let target = current.as_ref().map(|b| (b.target.clone(), b.rev));
    let wd = workdir.clone();
    use_effect(use_reactive!(|target, wd| {
        let Some((t, _)) = target else { return };
        input.set(match &t {
            Target::File(f) => relative(f, &wd),
            other => label(other),
        });
        error.set(None);
        spawn(async move {
            match resolve(thread_id, &t, &wd).await {
                Ok(u) => src.set(Some(u)),
                Err(e) => {
                    src.set(None);
                    error.set(Some(e));
                }
            }
        });
    }));
    let Some(open) = current else { return rsx! {} };
    let external = {
        let wd = workdir.clone();
        let t = open.target.clone();
        move |_| match &t {
            Target::File(f) => open_external(&format!("file://{}/{}", wd.trim_end_matches('/'), relative(f, &wd))),
            Target::Url(u) => open_external(u),
            Target::Port(..) => {
                if let Some(u) = src.peek().clone() {
                    open_external(&u);
                }
            }
        }
    };
    // Anzeige wie im Bild: Ordner grau, Dateiname hell
    let shown = input();
    let (dir, file) = match shown.rfind('/') {
        Some(i) if matches!(open.target, Target::File(_)) => (format!("{}/", &shown[..i]), shown[i + 1..].to_string()),
        _ => (String::new(), shown.clone()),
    };
    let mut editing = use_signal(|| false);
    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            div { class: "flex h-12 shrink-0 items-center gap-2 px-3",
                icons::Globe { class: "size-4 shrink-0 text-muted-foreground" }
                if editing() {
                    input {
                        class: "h-8 min-w-0 flex-1 rounded-md border bg-transparent px-2 font-mono text-[13px] outline-none focus:border-ring",
                        value: "{input}",
                        autofocus: true,
                        oninput: move |e| input.set(e.value()),
                        onblur: move |_| editing.set(false),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                editing.set(false);
                                match parse_target(&input.peek()) {
                                    Some(t) => self::open(thread_id, t),
                                    None => toast_error(tr("Bitte eine Adresse oder Datei angeben.")),
                                }
                            } else if e.key() == Key::Escape {
                                editing.set(false);
                            }
                        },
                    }
                } else {
                    button {
                        class: "min-w-0 flex-1 truncate rounded-md px-1 py-1 text-left font-mono text-[13px] hover:bg-accent/40",
                        title: tr("Adresse oder Datei eingeben"),
                        onclick: move |_| editing.set(true),
                        span { class: "text-muted-foreground", "{dir}" }
                        span { class: "text-foreground", "{file}" }
                    }
                }
                button {
                    class: "grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                    title: tr("Neu laden"),
                    onclick: move |_| reload += 1,
                    icons::RotateCw { class: "size-3.5" }
                }
                button { class: "shrink-0 text-[13px] text-muted-foreground underline underline-offset-4 hover:text-foreground", onclick: external,
                    {tr("Im Browser öffnen")}
                }
                button {
                    class: "grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                    title: tr("Schliessen"),
                    onclick: move |_| super::views::close(thread_id, super::views::View::Browser),
                    icons::X { class: "size-4" }
                }
            }
            div { class: "min-h-0 flex-1 px-3 pb-3",
                div { class: "h-full overflow-hidden rounded-xl border bg-black/30",
                    if let Some(e) = error() {
                        div { class: "grid h-full place-items-center p-6 text-center text-sm text-muted-foreground",
                            div { class: "grid gap-2",
                                icons::Globe { class: "mx-auto size-6" }
                                p { "{e}" }
                            }
                        }
                    } else if let Some(u) = src() {
                        iframe {
                            key: "{u}-{reload}",
                            class: "size-full bg-white",
                            src: "{u}",
                            title: tr("Browser"),
                            "allow": "clipboard-read; clipboard-write; fullscreen",
                        }
                    } else {
                        div { class: "grid h-full place-items-center text-sm text-muted-foreground", {tr("Lädt …")} }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(parse_target("localhost:3000"), Some(Target::Port(3000, "/".into())));
        assert_eq!(parse_target("http://127.0.0.1:5173/app?x=1"), Some(Target::Port(5173, "/app?x=1".into())));
        assert_eq!(parse_target(":8080/a"), Some(Target::Port(8080, "/a".into())));
        assert_eq!(parse_target("https://example.com"), Some(Target::Url("https://example.com".into())));
        assert_eq!(parse_target("./games/connect4.html"), Some(Target::File("games/connect4.html".into())));
        assert_eq!(parse_target("  "), None);
        assert_eq!(relative("/w/repo/games/x.html", "/w/repo/"), "games/x.html");
        assert_eq!(relative("./x.html", "/w/repo"), "x.html");
    }
}
