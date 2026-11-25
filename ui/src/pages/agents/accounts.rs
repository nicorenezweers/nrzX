//! Einstellungen → Agenten: je Agent-Art die Konten (mehrere Abos getrennt, jedes mit eigenem
//! Home), Konto hinzufügen (Name, Anmeldeart, Schlüssel), Anmeldung im CLI über ein Terminal,
//! Status prüfen, Standard, Löschen. Darunter die Skills (Prompt-Bausteine für `$`).

use super::data::{effort_label, reload_agents, reload_skills, use_agents, SKILLS};
use super::icons::AgentIcon;
use super::terminal::TerminalPane;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::{confirm, toast_error, Ask, Modal};
use crate::i18n::{tr, trf};
use crate::Route;
use dioxus::prelude::*;
use nrzx_shared::agents::{AccountUpdate, AgentAccount, AgentInfo, LoginSession, NewAccount, NewSkill, Skill, TerminalInput};

/// Einstellungen → Agenten: Konten je Anbieter (mehrere Abos), Anmeldung, Schlüssel.
#[component]
pub fn AccountsSettings() -> Element {
    let agents = use_agents();
    let mut add = use_signal(|| None::<AgentInfo>);
    let mut login = use_signal(|| None::<(AgentAccount, LoginSession)>);
    let mut edit = use_signal(|| None::<(AgentInfo, AgentAccount)>);
    let mut add_open = use_signal(|| false);
    let mut edit_open = use_signal(|| false);
    let mut login_open = use_signal(|| false);

    let start_login = move |acc: AgentAccount| {
        spawn(async move {
            match api::action::<LoginSession>(&format!("/accounts/{}/login", acc.id)).await {
                Ok(s) => {
                    login.set(Some((acc, s)));
                    login_open.set(true);
                }
                Err(e) => toast_error(e),
            }
        });
    };

    rsx! {
        div { class: "flex flex-col gap-6",
            div { class: "rounded-xl border bg-card px-5 py-4 text-sm",
                div { class: "flex items-start gap-3",
                    icons::Users { class: "mt-0.5 size-5 shrink-0 text-muted-foreground" }
                    div { class: "grid gap-1",
                        p { class: "font-medium", {tr("Mehrere Konten = mehrere Abos")} }
                        p { class: "text-muted-foreground",
                            {tr("Jedes Konto hat ein eigenes Home-Verzeichnis auf dem Server. So bleiben z. B. „Claude Max privat“ und „Claude Team“ vollständig getrennt – mit eigener Anmeldung, eigenen Einstellungen und eigenen Limits. Pro Thread wählst du, welches Konto arbeitet; Subagenten können andere Konten nutzen und so parallel laufen.")}
                        }
                    }
                }
            }
            match agents {
                None => rsx! { Loading {} },
                Some(list) => rsx! {
                    for a in list {
                        AgentSection {
                            key: "{a.kind.key()}",
                            info: a.clone(),
                            on_add: move |a: AgentInfo| {
                                add.set(Some(a));
                                add_open.set(true);
                            },
                            on_login: move |acc: AgentAccount| start_login(acc),
                            on_edit: move |(a, acc): (AgentInfo, AgentAccount)| {
                                edit.set(Some((a, acc)));
                                edit_open.set(true);
                            },
                        }
                    }
                },
            }
            SkillsSection {}
        }
        if let Some(a) = add() {
            AddAccount {
                key: "{a.kind.key()}",
                open: add_open,
                info: a,
                on_created: move |acc: AgentAccount| {
                    if acc.auth_kind == "login" {
                        start_login(acc);
                    }
                },
            }
        }
        if let Some((a, acc)) = edit() {
            EditAccount { key: "{acc.id}", open: edit_open, info: a, account: acc }
        }
        if let Some((acc, s)) = login() {
            LoginDialog {
                key: "{s.terminal.id}",
                open: login_open,
                account: acc,
                session: s,
                restart: move |acc| start_login(acc),
            }
        }
    }
}

fn auth_label(info: &AgentInfo, key: &str) -> String {
    info.auth_kinds.iter().find(|a| a.key == key).map(|a| tr(&a.label)).unwrap_or_else(|| key.to_string())
}

#[component]
fn StatusBadge(status: String, message: String) -> Element {
    let (dot, label) = match status.as_str() {
        "ok" => ("bg-success", tr("Bereit")),
        "login_required" => ("bg-warning", tr("Anmeldung nötig")),
        "error" => ("bg-destructive", tr("Fehler")),
        _ => ("bg-muted-foreground", tr("Unbekannt")),
    };
    rsx! {
        span { class: "inline-flex items-center gap-1.5 text-xs", title: tr(&message),
            span { class: "size-2 rounded-full {dot}" }
            "{label}"
        }
    }
}

#[component]
fn AgentSection(info: AgentInfo, on_add: EventHandler<AgentInfo>, on_login: EventHandler<AgentAccount>, on_edit: EventHandler<(AgentInfo, AgentAccount)>) -> Element {
    let a = info.clone();
    let description = if info.installed {
        trf("{0} · Version {1}", &[&info.program, &(if info.version.is_empty() { "?".to_string() } else { info.version.clone() })])
    } else {
        tr("Nicht installiert – unter Einstellungen → Werkzeuge installieren.")
    };
    rsx! {
        section { class: "rounded-xl border bg-card text-card-foreground shadow-sm",
            div { class: "flex flex-wrap items-center gap-3 border-b px-5 py-4",
                AgentIcon { kind: info.kind, size: 22 }
                div { class: "min-w-0 flex-1",
                    h2 { class: "font-semibold leading-none", "{info.name}" }
                    p { class: "mt-1.5 text-sm text-muted-foreground",
                        "{description}"
                        if !info.installed {
                            " "
                            Link { to: Route::Settings { section: "tools".into() }, class: "underline", {tr("Zu den Werkzeugen")} }
                        }
                    }
                }
                button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", onclick: move |_| on_add.call(a.clone()),
                    icons::Plus { class: "size-4" }
                    {tr("Konto hinzufügen")}
                }
            }
            if info.accounts.is_empty() {
                p { class: "px-5 py-4 text-sm text-muted-foreground", {tr("Noch kein Konto.")} }
            } else {
                div { class: "divide-y",
                    for acc in info.accounts.iter().cloned() {
                        AccountRow {
                            key: "{acc.id}",
                            info: info.clone(),
                            account: acc,
                            on_login: on_login,
                            on_edit: on_edit,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AccountRow(info: AgentInfo, account: AgentAccount, on_login: EventHandler<AgentAccount>, on_edit: EventHandler<(AgentInfo, AgentAccount)>) -> Element {
    let mut checking = use_signal(|| false);
    let acc = account.clone();
    let id = acc.id;
    let can_login = acc.auth_kind == "login" && !info.login_command.is_empty();
    let check = move |_| {
        checking.set(true);
        spawn(async move {
            match api::action::<AgentAccount>(&format!("/accounts/{id}/check")).await {
                Ok(a) => {
                    reload_agents().await;
                    if a.status != "ok" {
                        toast_error(tr(&a.status_message));
                    }
                }
                Err(e) => toast_error(e),
            }
            checking.set(false);
        });
    };
    let make_default = move |_| {
        spawn(async move {
            let body = AccountUpdate { is_default: Some(true), ..Default::default() };
            match api::patch::<AgentAccount, _>(&format!("/accounts/{id}"), &body).await {
                Ok(_) => reload_agents().await,
                Err(e) => toast_error(e),
            }
        });
    };
    let label = acc.label.clone();
    let remove = move |_| {
        let label = label.clone();
        spawn(async move {
            let ask = Ask::new(tr("Konto löschen?"), trf("„{0}“ und sein Home-Verzeichnis (Anmeldung, Einstellungen der CLIs) werden gelöscht.", &[&label]))
                .confirm(tr("Löschen"))
                .destructive();
            if !confirm(ask).await {
                return;
            }
            match api::delete(&format!("/accounts/{id}")).await {
                Ok(()) => reload_agents().await,
                Err(e) => toast_error(e),
            }
        });
    };
    let model = if acc.default_model.is_empty() { tr("Standardmodell") } else { acc.default_model.clone() };
    let effort = effort_label(&acc.default_effort);
    rsx! {
        div { class: "flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-3",
            div { class: "min-w-0 flex-1",
                div { class: "flex items-center gap-2",
                    span { class: "font-medium", "{acc.label}" }
                    if acc.is_default {
                        span { class: "rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground", {tr("Standard")} }
                    }
                    StatusBadge { status: acc.status.clone(), message: acc.status_message.clone() }
                }
                div { class: "mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground",
                    span { {auth_label(&info, &acc.auth_kind)} }
                    if !acc.secret_hint.is_empty() {
                        span { class: "font-mono", "{acc.secret_env}={acc.secret_hint}" }
                    }
                    span { "{model}" }
                    if !effort.is_empty() {
                        span { "{effort}" }
                    }
                    if acc.threads > 0 {
                        span { {trf("{0} Threads", &[&acc.threads])} }
                    }
                    if !acc.status_message.is_empty() {
                        span { {tr(&acc.status_message)} }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-1.5",
                if can_login {
                    button { class: "{btn(BTN_PRIMARY)} {BTN_SM}", onclick: move |_| on_login.call(acc.clone()),
                        icons::LogIn { class: "size-4" }
                        {tr("Anmelden")}
                    }
                }
                button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", disabled: checking(), onclick: check,
                    if checking() {
                        icons::LoaderCircle { class: "size-4 animate-spin" }
                    } else {
                        icons::RefreshCw { class: "size-4" }
                    }
                    {tr("Prüfen")}
                }
                if !account.is_default {
                    button { class: "{btn(BTN_GHOST)} {BTN_SM}", title: tr("Als Standard verwenden"), onclick: make_default,
                        icons::Star { class: "size-4" }
                    }
                }
                button { class: "{btn(BTN_GHOST)} {BTN_SM}", title: tr("Bearbeiten"),
                    onclick: {
                        let info = info.clone();
                        let account = account.clone();
                        move |_| on_edit.call((info.clone(), account.clone()))
                    },
                    icons::Pencil { class: "size-4" }
                }
                button { class: "{btn(BTN_GHOST)} {BTN_SM} text-destructive", title: tr("Löschen"), onclick: remove,
                    icons::Trash2 { class: "size-4" }
                }
            }
        }
    }
}

/// Agent anmelden, ohne in die Einstellungen zu gehen (aus dem Modell-Menü): Konto anlegen und –
/// bei Anmeldung im CLI – gleich das Anmelde-Terminal zeigen. Danach stehen die Modelle im Menü.
#[component]
pub(super) fn SignIn(open: Signal<bool>, info: AgentInfo) -> Element {
    let mut login = use_signal(|| None::<(AgentAccount, LoginSession)>);
    let mut login_open = use_signal(|| false);
    let start_login = move |acc: AgentAccount| {
        spawn(async move {
            match api::action::<LoginSession>(&format!("/accounts/{}/login", acc.id)).await {
                Ok(s) => {
                    login.set(Some((acc, s)));
                    login_open.set(true);
                }
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        AddAccount {
            key: "{info.kind.key()}",
            open,
            info: info.clone(),
            on_created: move |acc: AgentAccount| {
                if acc.auth_kind == "login" {
                    start_login(acc);
                }
            },
        }
        if let Some((acc, s)) = login() {
            LoginDialog {
                key: "{s.terminal.id}",
                open: login_open,
                account: acc,
                session: s,
                restart: move |acc| start_login(acc),
            }
        }
    }
}

#[component]
fn AddAccount(open: Signal<bool>, info: AgentInfo, on_created: EventHandler<AgentAccount>) -> Element {
    let first = info.auth_kinds.first().map(|a| a.key.clone()).unwrap_or_default();
    let mut label = use_signal(String::new);
    let mut auth = use_signal(move || first.clone());
    let mut env = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut model = use_signal(String::new);
    let mut effort = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let kind = info.kind;
    let current = info.auth_kinds.iter().find(|a| a.key == auth()).cloned();
    let needs_secret = auth() != "login";
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let body = NewAccount {
                kind,
                label: label().trim().to_string(),
                auth_kind: auth(),
                secret: secret(),
                secret_env: env(),
                default_model: model().trim().to_string(),
                default_effort: effort(),
                is_default: false,
            };
            match api::post::<AgentAccount, _>("/accounts", &body).await {
                Ok(acc) => {
                    reload_agents().await;
                    open.set(false);
                    on_created.call(acc);
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        Modal { open, title: trf("{0}: Konto hinzufügen", &[&info.name]), description: tr("Jedes Konto bekommt ein eigenes Home-Verzeichnis – mehrere Abos desselben Anbieters bleiben getrennt."),
            form { class: "grid gap-4", onsubmit: submit,
                Field { label: tr("Name"), hint: tr("z. B. „Claude Max privat“ oder „Claude Team“"),
                    input { class: INPUT, required: true, autofocus: true, value: "{label}", oninput: move |e| label.set(e.value()) }
                }
                Field { label: tr("Anmeldeart"),
                    select { class: SELECT, value: "{auth}", onchange: move |e| auth.set(e.value()),
                        for a in info.auth_kinds.iter() {
                            option { value: "{a.key}", selected: a.key == auth(), {tr(&a.label)} }
                        }
                    }
                }
                if let Some(a) = current.clone() {
                    p { class: "-mt-2 text-xs text-muted-foreground", {tr(&a.hint)} }
                    if needs_secret && a.env_vars.len() > 1 {
                        Field { label: tr("Umgebungsvariable"),
                            select { class: SELECT, value: "{env}", onchange: move |e| env.set(e.value()),
                                for v in a.env_vars.iter() {
                                    option { value: "{v}", "{v}" }
                                }
                            }
                        }
                    }
                }
                if needs_secret {
                    Field { label: if auth() == "oauth_token" { tr("OAuth-Token") } else { tr("API-Schlüssel") }, hint: tr("Wird verschlüsselt übertragen und nie wieder angezeigt."),
                        input { class: "{INPUT} font-mono", r#type: "password", autocomplete: "off", required: true, value: "{secret}", oninput: move |e| secret.set(e.value()) }
                    }
                }
                div { class: "grid gap-4 sm:grid-cols-2",
                    Field { label: tr("Modell-Vorgabe"),
                        input { class: "{INPUT} font-mono", list: "models-{kind.key()}", placeholder: tr("Standard"), value: "{model}", oninput: move |e| model.set(e.value()) }
                        datalist { id: "models-{kind.key()}",
                            for m in info.models.iter() {
                                option { value: "{m.id}", {tr(&m.label)} }
                            }
                        }
                    }
                    if !info.efforts.is_empty() {
                        Field { label: tr("Denk-Stufe"),
                            select { class: SELECT, value: "{effort}", onchange: move |e| effort.set(e.value()),
                                option { value: "", {tr("Standard")} }
                                for e in info.efforts.iter() {
                                    option { value: "{e}", {effort_label(e)} }
                                }
                            }
                        }
                    }
                }
                if auth() == "login" {
                    Callout { text: tr("Nach dem Anlegen öffnet sich ein Terminal im Home des Kontos mit dem Anmeldebefehl des CLIs.") }
                }
                div { class: "flex justify-end gap-2",
                    button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| open.set(false), {tr("Abbrechen")} }
                    button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy(),
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        }
                        {tr("Anlegen")}
                    }
                }
            }
        }
    }
}

#[component]
fn EditAccount(open: Signal<bool>, info: AgentInfo, account: AgentAccount) -> Element {
    let a = account.clone();
    let mut label = use_signal(|| a.label.clone());
    let mut model = use_signal(|| a.default_model.clone());
    let mut effort = use_signal(|| a.default_effort.clone());
    let mut env = use_signal(|| a.secret_env.clone());
    let mut secret = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let id = account.id;
    let auth = info.auth_kinds.iter().find(|x| x.key == account.auth_kind).cloned();
    let has_secret = account.auth_kind != "login";
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let s = secret();
            let body = AccountUpdate {
                label: Some(label().trim().to_string()),
                default_model: Some(model().trim().to_string()),
                default_effort: Some(effort()),
                secret_env: has_secret.then(|| env()),
                secret: if s.trim().is_empty() { None } else { Some(s) },
                ..Default::default()
            };
            match api::patch::<AgentAccount, _>(&format!("/accounts/{id}"), &body).await {
                Ok(_) => {
                    reload_agents().await;
                    open.set(false);
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        Modal { open, title: trf("{0} bearbeiten", &[&account.label]),
            form { class: "grid gap-4", onsubmit: submit,
                Field { label: tr("Name"),
                    input { class: INPUT, required: true, value: "{label}", oninput: move |e| label.set(e.value()) }
                }
                div { class: "grid gap-4 sm:grid-cols-2",
                    Field { label: tr("Modell-Vorgabe"),
                        input { class: "{INPUT} font-mono", list: "edit-models-{id}", placeholder: tr("Standard"), value: "{model}", oninput: move |e| model.set(e.value()) }
                        datalist { id: "edit-models-{id}",
                            for m in info.models.iter() {
                                option { value: "{m.id}", {tr(&m.label)} }
                            }
                        }
                    }
                    if !info.efforts.is_empty() {
                        Field { label: tr("Denk-Stufe"),
                            select { class: SELECT, value: "{effort}", onchange: move |e| effort.set(e.value()),
                                option { value: "", selected: effort().is_empty(), {tr("Standard")} }
                                for e in info.efforts.iter() {
                                    option { value: "{e}", selected: *e == effort(), {effort_label(e)} }
                                }
                            }
                        }
                    }
                }
                if let Some(a) = auth {
                    if has_secret {
                        if a.env_vars.len() > 1 {
                            Field { label: tr("Umgebungsvariable"),
                                select { class: SELECT, value: "{env}", onchange: move |e| env.set(e.value()),
                                    for v in a.env_vars.iter() {
                                        option { value: "{v}", selected: *v == env(), "{v}" }
                                    }
                                }
                            }
                        }
                        Field { label: tr("Neuer Schlüssel"), hint: trf("Leer lassen, um {0} zu behalten.", &[&account.secret_hint]),
                            input { class: "{INPUT} font-mono", r#type: "password", autocomplete: "off", value: "{secret}", oninput: move |e| secret.set(e.value()) }
                        }
                    }
                }
                p { class: "text-xs text-muted-foreground", {trf("Home-Verzeichnis: {0}", &[&account.home])} }
                div { class: "flex justify-end gap-2",
                    button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| open.set(false), {tr("Abbrechen")} }
                    button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy(), {tr("Speichern")} }
                }
            }
        }
    }
}

#[component]
fn LoginDialog(open: Signal<bool>, account: AgentAccount, session: LoginSession, restart: EventHandler<AgentAccount>) -> Element {
    let mut status = use_signal(|| None::<AgentAccount>);
    let mut checking = use_signal(|| false);
    let id = account.id;
    let term = session.terminal.id.clone();
    let mut code = use_signal(String::new);
    let mut sent = use_signal(|| false);
    // Grok: der Code steht im Terminal und wird im Browser bestätigt, nicht hier eingetippt.
    let device_code = session.command.contains("--device-auth") || session.command.contains("--device-code");
    // Programm beendet (z. B. falscher Code) – dann neu starten statt weiter einzugeben
    let mut ended = use_signal(|| false);
    let mut check = move || {
        checking.set(true);
        spawn(async move {
            match api::action::<AgentAccount>(&format!("/accounts/{id}/check")).await {
                Ok(a) => {
                    status.set(Some(a));
                    reload_agents().await;
                }
                Err(e) => toast_error(e),
            }
            checking.set(false);
        });
    };
    // Terminal schliessen, wenn der Dialog zugeht
    use_effect(move || {
        if !open() {
            let term = term.clone();
            spawn(async move {
                let _ = api::delete(&format!("/terminals/{term}")).await;
            });
        }
    });
    rsx! {
        Modal { open, title: trf("Anmelden: {0}", &[&account.label]), class: "max-w-4xl",
            // Spalte darf schmaler als die Terminalzeilen werden (das Terminal bricht neu um)
            div { class: "grid grid-cols-[minmax(0,1fr)] gap-3",
                p { class: "text-sm text-muted-foreground", {tr(&session.help)} }
                p { class: "text-xs text-muted-foreground",
                    {tr("Befehl:")}
                    " "
                    code { class: "rounded bg-muted px-1.5 py-0.5 font-mono", "{session.command}" }
                    " · "
                    {tr("läuft im eigenen Home dieses Kontos")}
                }
                div { class: "h-[22rem]",
                    TerminalPane {
                        id: session.terminal.id.clone(),
                        on_exit: move |_| {
                            ended.set(true);
                            check();
                        },
                    }
                }
                // Grok zeigt den Code im Terminal; bestätigt wird er im Browser.
                // Andere CLIs: Code aus dem Browser hier einfügen (manche zeigen ihn im Terminal nicht an).
                if device_code {
                    div { class: "grid gap-1.5",
                        p { class: "text-sm text-muted-foreground", {tr("Den Geräte-Code aus dem Terminal im Browser bestätigen. Hier nichts eintippen.")} }
                        if ended() {
                            div {
                                button {
                                    r#type: "button",
                                    class: btn(BTN_PRIMARY),
                                    onclick: {
                                        let account = account.clone();
                                        move |_| restart.call(account.clone())
                                    },
                                    icons::RotateCcw {}
                                    {tr("Neu starten")}
                                }
                            }
                        }
                    }
                } else {
                form {
                    class: "grid gap-1.5",
                    onsubmit: {
                        let term = session.terminal.id.clone();
                        move |e: FormEvent| {
                            e.prevent_default();
                            let c = code().trim().to_string();
                            if c.is_empty() || ended() {
                                return;
                            }
                            code.set(String::new());
                            sent.set(true);
                            let term = term.clone();
                            spawn(async move {
                                let res: Result<serde_json::Value, _> = api::post(&format!("/terminals/{term}/input"), &TerminalInput { data: format!("{c}\r") }).await;
                                if let Err(e) = res {
                                    toast_error(e);
                                    return;
                                }
                                gloo_timers::future::TimeoutFuture::new(4000).await;
                                check();
                            });
                        }
                    },
                    label { class: "text-sm font-medium", r#for: "login-code", {tr("Code aus dem Browser")} }
                    div { class: "flex gap-2",
                        input {
                            id: "login-code",
                            class: "{INPUT} font-mono",
                            value: "{code}",
                            placeholder: tr("Hier einfügen – Strg/Cmd+V oder Rechtsklick"),
                            autocomplete: "off",
                            autocapitalize: "off",
                            spellcheck: "false",
                            oninput: move |e| code.set(e.value()),
                        }
                        if ended() {
                            button {
                                r#type: "button",
                                class: btn(BTN_PRIMARY),
                                onclick: {
                                    let account = account.clone();
                                    move |_| restart.call(account.clone())
                                },
                                icons::RotateCcw {}
                                {tr("Neu starten")}
                            }
                        } else {
                            button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: code().trim().is_empty(),
                                icons::SendHorizontal {}
                                {tr("Senden")}
                            }
                        }
                    }
                    p { class: "text-xs text-muted-foreground",
                        if ended() {
                            {tr("Die Anmeldung ist beendet. Hat es nicht geklappt, mit „Neu starten“ einen neuen Link holen.")}
                        } else if sent() {
                            {tr("Code gesendet. Die Antwort erscheint im Terminal, der Status wird gleich geprüft.")}
                        } else {
                            {tr("Manche CLIs (z. B. Claude Code) zeigen den Code im Terminal nicht an. Hier siehst du ihn und schickst ihn mit Enter ab.")}
                        }
                    }
                }
                }
                div { class: "flex flex-wrap items-center justify-between gap-2",
                    if let Some(s) = status() {
                        StatusBadge { status: s.status.clone(), message: s.status_message.clone() }
                    } else {
                        span {}
                    }
                    div { class: "flex gap-2",
                        button { class: btn(BTN_OUTLINE), disabled: checking(), onclick: move |_| check(),
                            if checking() {
                                icons::LoaderCircle { class: "size-4 animate-spin" }
                            }
                            {tr("Status prüfen")}
                        }
                        button { class: btn(BTN_PRIMARY), onclick: move |_| open.set(false), {tr("Fertig")} }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Skills

#[component]
fn SkillsSection() -> Element {
    use_hook(|| {
        if SKILLS.peek().is_none() {
            spawn(reload_skills());
        }
    });
    let mut editing = use_signal(|| None::<Option<Skill>>);
    let mut open = use_signal(|| false);
    let skills = SKILLS.read().clone();
    rsx! {
        Section {
            title: tr("Skills"),
            description: tr("Gespeicherte Prompt-Bausteine. Im Composer mit $name einfügen."),
            actions: rsx! {
                button {
                    class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                    onclick: move |_| {
                        editing.set(Some(None));
                        open.set(true);
                    },
                    icons::Plus { class: "size-4" }
                    {tr("Skill")}
                }
            },
            match skills {
                None => rsx! { Loading {} },
                Some(list) if list.is_empty() => rsx! { p { class: "text-sm text-muted-foreground", {tr("Noch keine Skills.")} } },
                Some(list) => rsx! {
                    div { class: "divide-y",
                        for s in list {
                            div { key: "{s.id}", class: "flex items-center gap-3 py-2.5",
                                icons::Sparkles { class: "size-4 shrink-0 text-muted-foreground" }
                                div { class: "min-w-0 flex-1",
                                    div { class: "font-mono text-sm", "${s.name}" }
                                    p { class: "truncate text-xs text-muted-foreground",
                                        if s.description.is_empty() { "{s.prompt}" } else { "{s.description}" }
                                    }
                                }
                                button {
                                    class: "{btn(BTN_GHOST)} {BTN_SM}",
                                    title: tr("Bearbeiten"),
                                    onclick: {
                                        let s = s.clone();
                                        move |_| {
                                            editing.set(Some(Some(s.clone())));
                                            open.set(true);
                                        }
                                    },
                                    icons::Pencil { class: "size-4" }
                                }
                                button {
                                    class: "{btn(BTN_GHOST)} {BTN_SM} text-destructive",
                                    title: tr("Löschen"),
                                    onclick: {
                                        let id = s.id;
                                        move |_| {
                                            spawn(async move {
                                                match api::delete(&format!("/agents/skills/{id}")).await {
                                                    Ok(()) => reload_skills().await,
                                                    Err(e) => toast_error(e),
                                                }
                                            });
                                        }
                                    },
                                    icons::Trash2 { class: "size-4" }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some(s) = editing() {
            SkillDialog { key: "{s.as_ref().map(|s| s.id).unwrap_or(0)}", open, skill: s }
        }
    }
}

#[component]
fn SkillDialog(open: Signal<bool>, skill: Option<Skill>) -> Element {
    let s = skill.clone().unwrap_or_default();
    let mut name = use_signal(|| s.name.clone());
    let mut description = use_signal(|| s.description.clone());
    let mut prompt = use_signal(|| s.prompt.clone());
    let id = skill.as_ref().map(|s| s.id);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        spawn(async move {
            let body = NewSkill { name: name(), description: description(), prompt: prompt() };
            let r = match id {
                Some(id) => api::patch::<Skill, _>(&format!("/agents/skills/{id}"), &body).await,
                None => api::post::<Skill, _>("/agents/skills", &body).await,
            };
            match r {
                Ok(_) => {
                    reload_skills().await;
                    open.set(false);
                }
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        Modal { open, title: if id.is_some() { tr("Skill bearbeiten") } else { tr("Neuer Skill") }, class: "max-w-xl",
            form { class: "grid gap-4", onsubmit: submit,
                Field { label: tr("Name"), hint: tr("Buchstaben, Ziffern, - und _ (im Composer: $name)"),
                    input { class: "{INPUT} font-mono", required: true, value: "{name}", oninput: move |e| name.set(e.value()) }
                }
                Field { label: tr("Beschreibung"),
                    input { class: INPUT, value: "{description}", oninput: move |e| description.set(e.value()) }
                }
                Field { label: tr("Text"),
                    textarea { class: "{TEXTAREA} min-h-40 font-mono text-[13px]", required: true, value: "{prompt}", oninput: move |e| prompt.set(e.value()) }
                }
                div { class: "flex justify-end gap-2",
                    button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| open.set(false), {tr("Abbrechen")} }
                    button { r#type: "submit", class: btn(BTN_PRIMARY), {tr("Speichern")} }
                }
            }
        }
    }
}
