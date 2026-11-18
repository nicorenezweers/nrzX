//! Anmelden (mit Registrieren, wenn offen) und Ersteinrichtung – ohne Rahmen, als Karte in der
//! Mitte wie in nrzGit und nrzPaper.

use super::parts::BusyIcon;
use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::i18n::tr;
use crate::state::{self, STATUS};
use dioxus::prelude::*;
use nrzx_shared::Me;
use nrzx_shared::workspace::{LoginRequest, RegisterRequest, SetupRequest};

/// Gemeinsamer Rahmen der Anmeldeseiten: Logo, Titel, Karte, Sprachwahl.
#[component]
fn AuthCard(#[props(into)] title: String, #[props(into)] subtitle: String, children: Element) -> Element {
    rsx! {
        div { class: "flex min-h-screen items-center justify-center bg-background p-4",
            div { class: "w-full max-w-sm",
                div { class: "mb-6 flex flex-col items-center gap-3 text-center",
                    img { src: "/static/icon.png", class: "size-12 rounded-xl", alt: "" }
                    div {
                        h1 { class: "text-xl font-semibold tracking-tight", "{title}" }
                        p { class: "mt-1 text-sm text-muted-foreground", "{subtitle}" }
                    }
                }
                div { class: "rounded-xl border bg-card p-6 shadow-sm", {children} }
                p { class: "mt-6 text-center text-xs text-muted-foreground", {tr("nrzX – Coding-Agenten im Browser, verbunden mit nrzGit")} }
            }
        }
    }
}

/// Nur Ziele innerhalb der App (kein `//andere-seite`).
fn safe_next(next: &str) -> String {
    if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') && !next.starts_with("/user/login") {
        next.to_string()
    } else {
        "/".into()
    }
}

/// Nach erfolgreicher Anmeldung: die Seite neu laden.
async fn finish(_me: &Me, target: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().assign(target);
    }
}

#[component]
pub fn Login(next: String) -> Element {
    let nav = use_navigator();
    let mut login = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut register = use_signal(|| false);
    let mut email = use_signal(String::new);
    let mut full_name = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let status = use_resource(|| async { state::load_status().await });
    if let Some(Some(s)) = &*status.read() {
        if s.needs_setup {
            nav.replace(Route::Setup {});
        }
    }
    let can_register = STATUS.read().as_ref().is_some_and(|s| s.registration_open);
    let target = safe_next(&next);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        let target = target.clone();
        spawn(async move {
            busy.set(true);
            error.set(None);
            let res = if register() {
                let body = RegisterRequest { username: login(), email: email(), password: password(), full_name: full_name() };
                api::post::<Me, _>("/auth/register", &body).await
            } else {
                api::post::<Me, _>("/auth/login", &LoginRequest { login: login(), password: password() }).await
            };
            busy.set(false);
            match res {
                Ok(me) => finish(&me, &target).await,
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let (title, subtitle) = if register() {
        (tr("Konto erstellen"), tr("Ein Konto für nrzX auf diesem Server."))
    } else {
        (tr("Anmelden"), tr("Willkommen zurück bei nrzX."))
    };
    rsx! {
        AuthCard { title, subtitle,
            form { class: "grid gap-4", onsubmit: submit,
                if let Some(e) = error() {
                    Callout { text: e, error: true }
                }
                Field { label: if register() { tr("Benutzername") } else { tr("Benutzername oder E-Mail") },
                    input { class: INPUT, autofocus: true, autocomplete: "username", required: true, value: "{login}", oninput: move |e| login.set(e.value()) }
                }
                if register() {
                    Field { label: tr("E-Mail"),
                        input { class: INPUT, r#type: "email", autocomplete: "email", required: true, value: "{email}", oninput: move |e| email.set(e.value()) }
                    }
                    Field { label: tr("Voller Name"), hint: tr("Optional – erscheint als Autor von Commits."),
                        input { class: INPUT, autocomplete: "name", value: "{full_name}", oninput: move |e| full_name.set(e.value()) }
                    }
                }
                Field { label: tr("Passwort"),
                    input {
                        class: INPUT,
                        r#type: "password",
                        autocomplete: if register() { "new-password" } else { "current-password" },
                        required: true,
                        value: "{password}",
                        oninput: move |e| password.set(e.value()),
                    }
                }
                button { class: "{btn(BTN_PRIMARY)} w-full", r#type: "submit", disabled: busy(),
                    BusyIcon { busy: busy() }
                    if register() { {tr("Konto erstellen")} } else { {tr("Anmelden")} }
                }
                if can_register {
                    button {
                        r#type: "button",
                        class: "text-center text-sm text-muted-foreground hover:text-foreground",
                        onclick: move |_| {
                            register.toggle();
                            error.set(None);
                        },
                        if register() { {tr("Schon ein Konto? Anmelden")} } else { {tr("Noch kein Konto? Registrieren")} }
                    }
                }
            }
        }
    }
}

#[component]
pub fn Setup() -> Element {
    let nav = use_navigator();
    let mut username = use_signal(String::new);
    let mut email = use_signal(String::new);
    let mut full_name = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut open = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let status = use_resource(|| async { state::load_status().await });
    if let Some(Some(s)) = &*status.read() {
        if !s.needs_setup {
            nav.replace(Route::Login { next: String::new() });
        }
    }
    let submit = move |e: FormEvent| {
        e.prevent_default();
        spawn(async move {
            busy.set(true);
            error.set(None);
            let body = SetupRequest { username: username(), email: email(), password: password(), full_name: full_name(), registration_open: open() };
            let res = api::post::<Me, _>("/auth/setup", &body).await;
            busy.set(false);
            match res {
                Ok(me) => finish(&me, "/settings/nrzgit").await,
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    rsx! {
        AuthCard { title: tr("nrzX einrichten"), subtitle: tr("Das erste Konto wird Administrator."),
            form { class: "grid gap-4", onsubmit: submit,
                if let Some(e) = error() {
                    Callout { text: e, error: true }
                }
                Field { label: tr("Benutzername"),
                    input { class: INPUT, autofocus: true, autocomplete: "username", required: true, value: "{username}", oninput: move |e| username.set(e.value()) }
                }
                Field { label: tr("E-Mail"),
                    input { class: INPUT, r#type: "email", autocomplete: "email", required: true, value: "{email}", oninput: move |e| email.set(e.value()) }
                }
                Field { label: tr("Voller Name"), hint: tr("Optional – erscheint als Autor von Commits."),
                    input { class: INPUT, autocomplete: "name", value: "{full_name}", oninput: move |e| full_name.set(e.value()) }
                }
                Field { label: tr("Passwort"), hint: tr("Mindestens 8 Zeichen."),
                    input { class: INPUT, r#type: "password", autocomplete: "new-password", required: true, value: "{password}", oninput: move |e| password.set(e.value()) }
                }
                Checkbox { checked: open(), onchange: move |v| open.set(v), label: tr("Registrierung erlauben"), hint: tr("Weitere Personen dürfen sich selbst ein Konto anlegen.") }
                button { class: "{btn(BTN_PRIMARY)} w-full", r#type: "submit", disabled: busy(),
                    BusyIcon { busy: busy() }
                    {tr("Einrichten")}
                }
            }
        }
    }
}
