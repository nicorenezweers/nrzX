//! Einstellungen – in der App in einem eigenen Fenster (Menü „nrzX → Einstellungen …“, ⌘,),
//! im Browser als ganze Seite. Bewusst knapp: Übernahme aus nrzCode, Profil (Name, E-Mail,
//! Passwort, Benachrichtigungen), Nutzung, nrzGit, Agenten (`agents::AccountsSettings`), KI-Ketten
//! (`agents::ChainsSettings`), Werkzeuge (`agents::ToolsSettings`) und Sprechen (Mikrofon,
//! Spracherkennung, Vorlesen).

use super::layout::{close_settings, StatsBody};
use super::parts::*;
use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::components::widgets::*;
use crate::i18n::{tr, trf};
use crate::state::{self, ME};
use dioxus::prelude::*;
use nrzx_shared::Me;
use nrzx_shared::workspace::{NrzCodeImportPreview, NrzCodeImportRequest, NrzCodeImportResult, NrzGitCandidate, NrzGitConnect, NrzGitStatus, PasswordChange, ProfileUpdate};

#[component]
pub fn Settings(section: String) -> Element {
    let Some(me) = ME.read().clone() else {
        return rsx! { div { class: "mx-auto max-w-xl p-6", Callout { text: tr("Bitte anmelden.") } } };
    };
    let known = ["import", "profile", "usage", "nrzgit", "agents", "chains", "tools", "voice"];
    let section = if known.contains(&section.as_str()) { section } else { "import".to_string() };
    let entry = |key: &str, label: String, icon: Element| NavEntry { label, to: Route::Settings { section: key.to_string() }, active: section == key, icon };
    let entries = vec![
        entry("import", tr("Von nrzCode übernehmen"), rsx! { icons::Import {} }),
        entry("profile", tr("Profil"), rsx! { icons::User {} }),
        entry("usage", tr("Nutzung"), rsx! { icons::Gauge {} }),
        entry("nrzgit", "nrzGit".to_string(), rsx! { icons::GitPullRequest {} }),
        entry("agents", tr("Agenten"), rsx! { icons::Bot {} }),
        entry("chains", tr("KI-Ketten"), rsx! { icons::Workflow {} }),
        entry("tools", tr("Werkzeuge"), rsx! { icons::Wrench {} }),
        entry("voice", tr("Sprechen"), rsx! { icons::Mic {} }),
    ];
    let title = entries.iter().find(|e| e.active).map(|e| e.label.clone()).unwrap_or_default();
    // Abschnitt merken (⌘, und „Einstellungen“ öffnen ihn wieder)
    use_effect(use_reactive!(|section| crate::util::storage_set("nrz-settings-section", &section)));
    let nav = use_navigator();
    // Die Ampel liegt im Fenster der Einstellungen über dem Paneel – hier kein Platz mehr dafür
    let head = "flex h-14 shrink-0 items-center px-4";
    let bar = "flex h-14 shrink-0 items-center gap-3 border-b px-4 md:px-6";
    let content = match section.as_str() {
        "usage" => rsx! { UsageSection {} },
        "nrzgit" => rsx! { NrzGitSection {} },
        "agents" => rsx! { crate::pages::agents::AccountsSettings {} },
        "chains" => rsx! { crate::pages::agents::ChainsSettings {} },
        "tools" => rsx! { crate::pages::agents::ToolsSettings {} },
        "voice" => rsx! { super::dieter::DieterSettings {} },
        "profile" => rsx! {
            ProfileSection { me: me.clone() }
            PasswordSection {}
            NotifySection {}
        },
        _ => rsx! { ImportSection {} },
    };
    rsx! {
        div { class: "flex h-full min-h-0 w-full",
            // Links: Abschnitte wie eine Seitenleiste
            aside { class: "hidden w-60 shrink-0 flex-col border-r border-sidenav-border bg-sidenav md:flex",
                div { class: head, "data-drag": "1",
                    span { class: "truncate text-[14px] font-semibold tracking-tight", {tr("Einstellungen")} }
                }
                div { class: "min-h-0 flex-1 overflow-y-auto px-2 pt-2 pb-3",
                    SettingsNav { entries: entries.clone(), heading: me.username.clone() }
                }
            }
            div { class: "flex min-w-0 flex-1 flex-col [.nrz-glass_&]:bg-background",
                header { class: bar, "data-drag": "1",
                    h1 { class: "min-w-0 flex-1 truncate text-[15px] font-semibold", "{title}" }
                    button {
                        class: "grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground [&_svg]:size-[18px]",
                        title: tr("Einstellungen schliessen"),
                        // In der App ein eigenes Fenster: schliessen; im Browser zurück zur Seite davor
                        onclick: move |_| {
                            if !crate::util::in_app() || !crate::util::native("settings:close") {
                                close_settings(nav);
                            }
                        },
                        icons::X {}
                    }
                }
                main { class: "min-h-0 flex-1 overflow-y-auto",
                    div { class: "mx-auto max-w-4xl space-y-6 p-4 md:p-8",
                        // Schmal: Abschnitte als Leiste über dem Inhalt
                        div { class: "md:hidden", SectionNav { entries } }
                        {content}
                    }
                }
            }
        }
    }
}

/// Abschnitte untereinander (links in den Einstellungen).
#[component]
fn SettingsNav(entries: Vec<NavEntry>, heading: String) -> Element {
    rsx! {
        nav { class: "grid gap-0.5",
            p { class: "truncate px-2.5 pb-1 text-xs font-medium text-muted-foreground", "{heading}" }
            for e in entries {
                Link {
                    to: e.to.clone(),
                    class: if e.active {
                        "flex items-center gap-2.5 rounded-lg bg-sidenav-accent px-2.5 py-1.5 text-[13px] font-medium text-foreground [&_svg]:size-4"
                    } else {
                        "flex items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-[13px] text-muted-foreground hover:bg-sidenav-accent hover:text-foreground [&_svg]:size-4"
                    },
                    {e.icon.clone()}
                    "{e.label}"
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Nutzung

/// Limits, Kosten und Tokens (wie im Fenster der Menüleiste) und die genaue Statistik.
#[component]
fn UsageSection() -> Element {
    rsx! {
        Section {
            title: tr("Nutzung"),
            description: tr("Limits der Abos (wo der Anbieter sie meldet), Kosten und Tokens aller Läufe – auch laufender und gelöschter Threads."),
            crate::pages::agents::UsageBoard { compact: false }
        }
        Section { title: tr("Statistik"), description: tr("Genaue Zahlen zu Projekten, Threads, Läufen, Tokens, Kosten und Laufzeit."),
            StatsBody {}
        }
    }
}

// ------------------------------------------------------------------ Profil

#[component]
fn ProfileSection(me: Me) -> Element {
    let mut full_name = use_signal(|| me.full_name.clone());
    let mut email = use_signal(|| me.email.clone());
    let mut busy = use_signal(|| false);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let res = api::put::<Me, _>("/me/profile", &ProfileUpdate { full_name: full_name(), email: email() }).await;
            busy.set(false);
            match res {
                Ok(m) => *ME.write() = Some(m),
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        Section { title: tr("Profil"), description: tr("Name und E-Mail erscheinen als Autor der Commits, die du oder deine Agenten anlegen."),
            form { class: "grid max-w-lg gap-4", onsubmit: submit,
                p { class: "text-sm",
                    span { class: "font-medium", "{me.username}" }
                }
                Field { label: tr("Voller Name"),
                    input { class: INPUT, autocomplete: "name", value: "{full_name}", oninput: move |e| full_name.set(e.value()) }
                }
                Field { label: tr("E-Mail"),
                    input { class: INPUT, r#type: "email", autocomplete: "email", required: true, value: "{email}", oninput: move |e| email.set(e.value()) }
                }
                div {
                    button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy(),
                        BusyIcon { busy: busy() }
                        {tr("Speichern")}
                    }
                }
            }
        }
    }
}

#[component]
fn PasswordSection() -> Element {
    let mut current = use_signal(String::new);
    let mut new_pw = use_signal(String::new);
    let mut repeat = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        if new_pw() != repeat() {
            toast_error(tr("Die neuen Passwörter stimmen nicht überein."));
            return;
        }
        busy.set(true);
        spawn(async move {
            let res = api::put::<serde_json::Value, _>("/me/password", &PasswordChange { current_password: current(), new_password: new_pw() }).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    current.set(String::new());
                    new_pw.set(String::new());
                    repeat.set(String::new());
                }
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        Section { title: tr("Passwort"), description: tr("Nach dem Ändern sind alle anderen Sitzungen abgemeldet."),
            form { class: "grid max-w-lg gap-4", onsubmit: submit,
                Field { label: tr("Bisheriges Passwort"),
                    input { class: INPUT, r#type: "password", autocomplete: "current-password", required: true, value: "{current}", oninput: move |e| current.set(e.value()) }
                }
                Field { label: tr("Neues Passwort"), hint: tr("Mindestens 8 Zeichen."),
                    input { class: INPUT, r#type: "password", autocomplete: "new-password", required: true, value: "{new_pw}", oninput: move |e| new_pw.set(e.value()) }
                }
                Field { label: tr("Neues Passwort wiederholen"),
                    input { class: INPUT, r#type: "password", autocomplete: "new-password", required: true, value: "{repeat}", oninput: move |e| repeat.set(e.value()) }
                }
                div {
                    button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy(),
                        BusyIcon { busy: busy() }
                        {tr("Passwort ändern")}
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ nrzGit

#[component]
fn NrzGitSection() -> Element {
    let mut version = use_signal(|| 0u32);
    // Nur nrzGit
    let status = use_resource(move || async move {
        let _ = version();
        api::get::<NrzGitStatus>("/nrzgit?kind=nrzgit").await
    });
    match status.read().clone() {
        None => rsx! { Loading {} },
        Some(Err(e)) => rsx! { Callout { text: e.message, error: true } },
        Some(Ok(s)) => rsx! {
            NrzGitForm { key: "{version}", status: s.clone(), onchange: move |_| version += 1 }
            Section { title: tr("So geht’s"), description: tr("Einmal einrichten – danach arbeiten alle Agenten mit nrzGit."),
                ol { class: "grid gap-3 text-sm",
                    Step { n: 1, text: tr("Öffne nrzGit und gehe zu Einstellungen → MCP (der Knopf „Token in nrzGit erstellen“ führt direkt dorthin).") }
                    Step { n: 2, text: tr("Erstelle ein Token für nrzX und kopiere es – es wird nur einmal angezeigt.") }
                    Step { n: 3, text: tr("Füge Adresse und Token oben ein und klicke auf „Verbinden“. nrzX prüft das Token sofort.") }
                    Step { n: 4, text: tr("Fertig: Unter „Projekt hinzufügen“ erscheinen deine Repositories. Alle Agenten können Repositories anlegen, pushen, Pull Requests und Releases erstellen und Images in die Registry von nrzGit bauen.") }
                }
            }
        },
    }
}

#[component]
fn NrzGitForm(status: NrzGitStatus, onchange: EventHandler<()>) -> Element {
    let s = status.clone();
    let kind = "nrzgit";
    let forge = "nrzGit";
    let mut url = use_signal(|| if s.url.is_empty() { s.default_url.clone() } else { s.url.clone() });
    let mut public_url = use_signal(|| s.public_url.clone());
    let mut token = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut searching = use_signal(|| false);
    let mut found = use_signal(|| None::<Vec<NrzGitCandidate>>);
    // Link „Token erstellen“ passt sich der Adresse für den Browser an (sonst der Verbindungsadresse)
    let typed = if public_url().trim().is_empty() { url() } else { public_url() };
    let typed = typed.trim().trim_end_matches('/').trim_end_matches("/mcp").to_string();
    let token_page = "user/settings/mcp";
    let token_link = if typed.is_empty() {
        String::new()
    } else if typed.starts_with("http://") || typed.starts_with("https://") {
        format!("{typed}/{token_page}")
    } else {
        format!("http://{typed}/{token_page}")
    };
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let body = NrzGitConnect { url: url(), token: token(), public_url: public_url(), kind: kind.to_string() };
            let res = api::put::<NrzGitStatus, _>("/nrzgit", &body).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    token.set(String::new());
                    state::load_me().await;
                    onchange.call(());
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let verify = move |_| {
        busy.set(true);
        spawn(async move {
            match api::action::<NrzGitStatus>(&format!("/nrzgit/verify?kind={kind}")).await {
                Ok(s) if s.connected => {}
                Ok(s) => toast_error(s.error),
                Err(e) => toast_error(e),
            }
            busy.set(false);
            state::load_me().await;
            onchange.call(());
        });
    };
    let disconnect = move |_| {
        spawn(async move {
            let ask = Ask::new(
                trf("Verbindung zu {0} trennen?", &[&forge]),
                trf("Das Token wird gelöscht. Agenten können danach dort nicht mehr pushen, und neue Projekte aus {0} gehen erst wieder nach dem Verbinden.", &[&forge]),
            )
            .confirm(tr("Trennen"))
            .destructive();
            if !confirm(ask).await {
                return;
            }
            match api::delete(&format!("/nrzgit?kind={kind}")).await {
                Ok(()) => {}
                Err(e) => toast_error(e),
            }
            state::load_me().await;
            onchange.call(());
        });
    };
    let search = move |_| {
        searching.set(true);
        spawn(async move {
            match api::get::<Vec<NrzGitCandidate>>(&format!("/nrzgit/candidates?url={}", api::enc(&url()))).await {
                Ok(list) => {
                    // Nur Adressen von nrzGit zeigen
                    let list: Vec<NrzGitCandidate> = list.into_iter().filter(|c| !c.reachable || c.kind == kind).collect();
                    // Erste erreichbare Adresse gleich übernehmen
                    if let Some(c) = list.iter().find(|c| c.reachable) {
                        if !url().trim().is_empty() && url() != c.url && public_url().trim().is_empty() && !url().contains("_app_1") {
                            // Bisherige Adresse (vom Browser) bleibt für Links erhalten
                            public_url.set(url());
                        }
                        url.set(c.url.clone());
                    }
                    found.set(Some(list));
                }
                Err(e) => toast_error(e),
            }
            searching.set(false);
        });
    };
    let has_saved = !s.url.is_empty();
    let (placeholder, public_placeholder, token_placeholder) = ("http://nrz-git_app_1:8380", "http://umbrel.local:8390", "nrzg_…");
    rsx! {
        Section {
            title: forge.to_string(),
            description: tr("nrzX klont, pusht und legt Pull Requests mit deinem Zugriffstoken an – die Agenten bekommen nrzGit zusätzlich als MCP-Server."),
            actions: rsx! {
                if s.connected {
                    span { class: "inline-flex items-center gap-1.5 rounded-full bg-success/15 px-2.5 py-1 text-xs font-medium text-success",
                        icons::CircleCheck { class: "size-3.5" }
                        {tr("Verbunden")}
                        if s.has_mcp { " · MCP" }
                    }
                } else if has_saved {
                    span { class: "inline-flex items-center gap-1.5 rounded-full bg-destructive/15 px-2.5 py-1 text-xs font-medium text-destructive",
                        icons::CircleAlert { class: "size-3.5" }
                        {tr("Gestört")}
                    }
                } else {
                    span { class: "inline-flex items-center gap-1.5 rounded-full bg-muted px-2.5 py-1 text-xs font-medium text-muted-foreground", {tr("Nicht verbunden")} }
                }
            },
            if has_saved {
                dl { class: "mb-5 grid gap-x-6 gap-y-3 text-sm sm:grid-cols-2",
                    div {
                        dt { class: "text-xs text-muted-foreground", {tr("Adresse")} }
                        dd { class: "mt-0.5 break-all",
                            ExternalLink { href: s.url.clone(), class: "hover:underline", "{s.url}" }
                        }
                    }
                    div {
                        dt { class: "text-xs text-muted-foreground", {trf("Benutzer in {0}", &[&forge])} }
                        dd { class: "mt-0.5", if s.username.is_empty() { "–" } else { "{s.username}" } }
                    }
                    div {
                        dt { class: "text-xs text-muted-foreground", {tr("Token")} }
                        dd { class: "mt-0.5 font-mono", "{s.token_hint}" }
                    }
                    div {
                        dt { class: "text-xs text-muted-foreground", {tr("Zuletzt geprüft")} }
                        dd { class: "mt-0.5",
                            if let Some(ts) = s.verified_at { RelTime { ts } } else { "–" }
                        }
                    }
                }
                if !s.error.is_empty() {
                    div { class: "mb-5", Callout { text: tr(&s.error), error: true } }
                }
            }
            form { class: "grid max-w-xl gap-4", onsubmit: submit,
                Field {
                    label: trf("Adresse von {0} (vom Server aus)", &[&forge]),
                    hint: tr("Der Server von nrzX verbindet sich selbst. Im Docker-Netz von Umbrel ist das meist http://nrz-git_app_1:8380 – nrzGit zeigt alle internen Adressen unter Einstellungen → MCP."),
                    div { class: "flex gap-2",
                        input { class: INPUT, r#type: "url", required: true, placeholder: placeholder, value: "{url}", oninput: move |e| url.set(e.value()) }
                        button { r#type: "button", class: "{btn(BTN_OUTLINE)} shrink-0", disabled: searching(), onclick: search, title: tr("Übliche Adressen vom Server aus durchprobieren"),
                            BusyIcon { busy: searching() }
                            if !searching() { icons::Radar {} }
                            {tr("Automatisch suchen")}
                        }
                    }
                }
                if let Some(list) = found() {
                    div { class: "grid gap-1 rounded-lg border p-2",
                        for c in list {
                            div { key: "{c.url}", class: "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm",
                                if c.reachable {
                                    icons::CircleCheck { class: "size-4 shrink-0 text-success" }
                                } else {
                                    icons::CircleX { class: "size-4 shrink-0 text-muted-foreground" }
                                }
                                div { class: "min-w-0 flex-1",
                                    p { class: "truncate font-mono text-[13px]", "{c.url}" }
                                    p { class: "truncate text-xs text-muted-foreground",
                                        {tr(&c.label)}
                                        " · "
                                        if c.reachable { {trf("nrzGit {0}", &[&c.version])} } else { {tr(&c.error)} }
                                    }
                                }
                                if c.reachable {
                                    button { r#type: "button", class: "{btn(BTN_GHOST)} {BTN_SM}",
                                        onclick: {
                                            let u = c.url.clone();
                                            move |_| url.set(u.clone())
                                        },
                                        {tr("Übernehmen")}
                                    }
                                }
                            }
                        }
                    }
                }
                Field { label: tr("Adresse im Browser (optional)"), hint: trf("Für Links zu {0} (Repositories, Pull Requests, Token erstellen), wenn die Adresse oben intern ist – z. B. {1}.", &[&forge, &public_placeholder]),
                    input { class: INPUT, r#type: "url", placeholder: public_placeholder, value: "{public_url}", oninput: move |e| public_url.set(e.value()) }
                }
                Field {
                    label: tr("Zugriffstoken"),
                    hint: if has_saved {
                        tr("Leer lassen, um das gespeicherte Token zu behalten.")
                    } else {
                        tr("Beginnt mit nrzg_ – wird nie wieder angezeigt.")
                    },
                    input { class: "{INPUT} font-mono", r#type: "password", autocomplete: "off", placeholder: token_placeholder, value: "{token}", oninput: move |e| token.set(e.value()) }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy(),
                        BusyIcon { busy: busy() }
                        if has_saved { {tr("Speichern und prüfen")} } else { {tr("Verbinden")} }
                    }
                    if !token_link.is_empty() {
                        ExternalLink { href: token_link.clone(), class: btn(BTN_OUTLINE),
                            icons::KeyRound {}
                            {tr("Token in nrzGit erstellen")}
                            icons::ExternalLink { class: "size-3.5 text-muted-foreground" }
                        }
                    }
                    if has_saved {
                        button { r#type: "button", class: btn(BTN_GHOST), disabled: busy(), onclick: verify,
                            icons::RefreshCw {}
                            {tr("Prüfen")}
                        }
                        button { r#type: "button", class: "{btn(BTN_GHOST)} text-destructive", onclick: disconnect,
                            icons::Unlink {}
                            {tr("Trennen")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Step(n: u32, #[props(into)] text: String) -> Element {
    rsx! {
        li { class: "flex gap-3",
            span { class: "grid size-6 shrink-0 place-items-center rounded-full bg-muted text-xs font-semibold", "{n}" }
            span { class: "pt-0.5", "{text}" }
        }
    }
}

/// Benachrichtigungen (früher die Glocke oben rechts).
#[component]
fn NotifySection() -> Element {
    use crate::notify::{MODE, Mode, set_mode};
    let mode = MODE();
    rsx! {
        Section { title: tr("Benachrichtigungen"), description: tr("Bei Rückfragen, wenn ein Agent fertig ist, bei Fehlern und Rate-Limits."),
            div { class: "grid max-w-md gap-2",
                for m in Mode::ALL {
                    button {
                        class: if m == mode { "flex items-start gap-2.5 rounded-lg border border-ring bg-accent px-4 py-2.5 text-left" } else { "flex items-start gap-2.5 rounded-lg border px-4 py-2.5 text-left hover:bg-accent/50" },
                        onclick: move |_| set_mode(m),
                        span { class: "min-w-0 flex-1",
                            span { class: "block text-sm font-medium", {m.label()} }
                            span { class: "block text-xs text-muted-foreground", {m.hint()} }
                        }
                        if m == mode {
                            icons::Check { class: "mt-0.5 size-4 shrink-0" }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Von nrzCode übernehmen

/// Konten (mit ihren Anmeldungen), die Verbindung zu nrzGit und die Projekte aus nrzCode auf
/// diesem Rechner übernehmen – dann muss man sich in nrzX nirgends neu anmelden.
#[component]
fn ImportSection() -> Element {
    let mut version = use_signal(|| 0u32);
    let preview = use_resource(move || async move {
        let _ = version();
        api::get::<NrzCodeImportPreview>("/import/nrzcode").await
    });
    match preview.read().clone() {
        None => rsx! { Loading {} },
        Some(Err(e)) => rsx! { Callout { text: e.message, error: true } },
        Some(Ok(p)) if !p.found => rsx! {
            Section { title: tr("Von nrzCode übernehmen"), description: tr("Agent-Konten mit ihren Anmeldungen, die Verbindung zu nrzGit und die Projekte."),
                p { class: "text-sm text-muted-foreground", {tr("Kein nrzCode auf diesem Rechner gefunden.")} }
            }
        },
        Some(Ok(p)) => rsx! { ImportForm { key: "{version}", preview: p, ondone: move |_| version += 1 } },
    }
}

#[component]
fn ImportForm(preview: NrzCodeImportPreview, ondone: EventHandler<()>) -> Element {
    let p = preview.clone();
    let new_accounts = p.accounts.iter().filter(|a| !a.exists).count();
    let new_projects = p.projects.iter().filter(|x| !x.exists).count();
    let new_nrzgit = p.nrzgit.is_some() && !p.nrzgit_exists;
    // Auch vorhandene Konten: ihre Anmeldung wird frisch übernommen und verknüpft
    let any_accounts = !p.accounts.is_empty();
    let _ = new_accounts;
    let mut accounts = use_signal(move || any_accounts);
    let mut nrzgit = use_signal(move || new_nrzgit);
    let mut projects = use_signal(move || new_projects > 0);
    let mut busy = use_signal(|| false);
    let nothing = !any_accounts && new_projects == 0 && !new_nrzgit;
    let run = move |_| {
        busy.set(true);
        spawn(async move {
            let body = NrzCodeImportRequest { accounts: accounts(), nrzgit: nrzgit(), projects: projects() };
            let res = api::post::<NrzCodeImportResult, _>("/import/nrzcode", &body).await;
            busy.set(false);
            match res {
                Ok(r) => {
                    for s in r.skipped.iter().take(3) {
                        toast_error(s.clone());
                    }
                    if let Some(m) = state::load_me().await {
                        *ME.write() = Some(m);
                    }
                    load_projects().await;
                    crate::pages::agents::data::reload_agents().await;
                    ondone.call(());
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let check = |on: bool| if on { "mt-0.5 size-4 shrink-0 accent-foreground" } else { "mt-0.5 size-4 shrink-0" };
    rsx! {
        Section {
            title: tr("Von nrzCode übernehmen"),
            description: tr("Agent-Konten mit ihren Anmeldungen, die Verbindung zu nrzGit und die Projekte – dann musst du dich in nrzX nirgends neu anmelden. Die Anmeldungen bleiben verknüpft: erneuert nrzCode oder nrzX ein Token, übernimmt die andere App es von selbst."),
            div { class: "grid max-w-xl gap-3 text-sm",
                label { class: "flex items-start gap-3 rounded-xl border bg-card/40 p-3",
                    input { r#type: "checkbox", class: check(accounts()), checked: accounts(), disabled: p.accounts.is_empty(), onchange: move |e| accounts.set(e.checked()) }
                    div { class: "min-w-0 flex-1",
                        p { class: "font-medium", {tr("Agent-Konten mit Anmeldung (verknüpft)")} }
                        p { class: "text-muted-foreground", {tr("Neue Konten kommen dazu, vorhandene holen sich ihre Anmeldung frisch aus nrzCode.")} }
                        if p.accounts.is_empty() {
                            p { class: "text-muted-foreground", {tr("Keine Konten in nrzCode.")} }
                        }
                        div { class: "mt-1.5 flex flex-wrap gap-1.5",
                            for a in p.accounts.iter() {
                                span {
                                    class: "inline-flex items-center gap-1.5 rounded-md border border-white/10 px-2 py-0.5 text-xs",
                                    title: if a.exists { tr("Gibt es in nrzX schon – die Anmeldung wird aufgefrischt") } else { tr("Neu") },
                                    crate::pages::agents::AgentIcon { kind: a.kind, size: 12 }
                                    {if a.label.is_empty() { a.kind.name().to_string() } else { format!("{} · {}", a.kind.name(), a.label) }}
                                    if a.exists {
                                        icons::RefreshCw { class: "size-3 text-muted-foreground" }
                                    }
                                }
                            }
                        }
                    }
                }
                label { class: "flex items-start gap-3 rounded-xl border bg-card/40 p-3",
                    input { r#type: "checkbox", class: check(nrzgit()), checked: nrzgit(), disabled: p.nrzgit.is_none(), onchange: move |e| nrzgit.set(e.checked()) }
                    div { class: "min-w-0 flex-1",
                        p { class: "font-medium", {tr("Verbindung zu nrzGit")} }
                        p { class: "truncate text-muted-foreground",
                            match (&p.nrzgit, p.nrzgit_exists) {
                                (None, _) => tr("nrzCode ist nicht mit nrzGit verbunden."),
                                (Some(u), true) => trf("{0} – nrzX ist schon verbunden (wird ersetzt).", &[u]),
                                (Some(u), false) => u.clone(),
                            }
                        }
                    }
                }
                label { class: "flex items-start gap-3 rounded-xl border bg-card/40 p-3",
                    input { r#type: "checkbox", class: check(projects()), checked: projects(), disabled: p.projects.is_empty(), onchange: move |e| projects.set(e.checked()) }
                    div { class: "min-w-0 flex-1",
                        p { class: "font-medium", {tr("Projekte")} }
                        if p.projects.is_empty() {
                            p { class: "text-muted-foreground", {tr("Keine Projekte in nrzCode.")} }
                        } else {
                            p { class: "text-muted-foreground", {tr("Aus nrzGit neu geklont, lokale Projekte kopiert.")} }
                        }
                        div { class: "mt-1.5 flex flex-wrap gap-1.5",
                            for x in p.projects.iter() {
                                span {
                                    class: if x.exists { "rounded-md border px-2 py-0.5 text-xs text-muted-foreground line-through" } else { "rounded-md border px-2 py-0.5 text-xs" },
                                    title: if x.repo.is_empty() { tr("Lokales Projekt") } else { x.repo.clone() },
                                    "{x.name}"
                                }
                            }
                        }
                    }
                }
                div { class: "flex items-center gap-3",
                    button {
                        class: btn(BTN_PRIMARY),
                        disabled: busy() || nothing || !(accounts() || nrzgit() || projects()),
                        onclick: run,
                        BusyIcon { busy: busy() }
                        {tr("Übernehmen")}
                    }
                    if nothing {
                        span { class: "text-xs text-muted-foreground", {tr("Alles aus nrzCode ist schon da.")} }
                    }
                }
                p { class: "truncate text-xs text-muted-foreground", title: "{p.path}", {trf("Gefunden in {0}", &[&p.path])} }
            }
        }
    }
}
