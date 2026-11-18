//! Reiter „Team“ der rechten Spalte: mehrere Leute arbeiten – jeder mit seinem eigenen nrzX –
//! am selben Repository. Zeigt, wer dabei ist und woran die Agenten der anderen arbeiten, den
//! gemeinsamen Nachrichtenverlauf (Menschen und Agenten) und ein Feld, um selbst ins Team oder
//! an einen bestimmten Agenten zu schreiben. Der Stand kommt über `refs/nrzx/team` des
//! Repositorys; `HubEvent::TeamUpdated` lädt neu.

use crate::api;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::state::TEAM_REV;
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::team::{address, TeamMember, TeamMessage, TeamPost, TeamSettings, TeamView};

type Loaded = Signal<Option<Result<TeamView, String>>>;

const BTN: &str = "inline-flex h-8 items-center justify-center gap-1.5 rounded-md border px-3 text-[13px] font-medium hover:bg-accent disabled:opacity-50";
const BTN_PRIMARY: &str = "inline-flex h-8 items-center justify-center gap-1.5 rounded-md bg-primary px-3 text-[13px] font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50";
const ICON_BTN: &str = "grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-50";

#[component]
pub fn TeamPanel(project_id: Option<i64>) -> Element {
    match project_id {
        Some(pid) => rsx! { TeamBody { key: "{pid}", project_id: pid } },
        None => rsx! {
            p { class: "p-4 text-[13px] leading-relaxed text-muted-foreground", {tr("Ein Team gibt es in Projekten mit einem gemeinsamen Repository.")} }
        },
    }
}

async fn change(project_id: i64, body: TeamSettings, mut view: Loaded, mut busy: Signal<bool>) {
    busy.set(true);
    match api::put::<TeamView, _>(&format!("/projects/{project_id}/team"), &body).await {
        Ok(v) => view.set(Some(Ok(v))),
        Err(e) => toast_error(e),
    }
    busy.set(false);
}

async fn sync_now(project_id: i64, mut view: Loaded, mut busy: Signal<bool>) {
    busy.set(true);
    match api::action::<TeamView>(&format!("/projects/{project_id}/team/sync")).await {
        Ok(v) => view.set(Some(Ok(v))),
        Err(e) => toast_error(e),
    }
    busy.set(false);
}

async fn send(project_id: i64, to: String, mut text: Signal<String>, mut view: Loaded, mut busy: Signal<bool>) {
    let body = TeamPost { to, text: text.peek().trim().to_string(), reply_to: None };
    if body.text.is_empty() {
        return;
    }
    busy.set(true);
    match api::post::<TeamView, _>(&format!("/projects/{project_id}/team/messages"), &body).await {
        Ok(v) => {
            text.set(String::new());
            view.set(Some(Ok(v)));
        }
        Err(e) => toast_error(e),
    }
    busy.set(false);
}

#[component]
fn TeamBody(project_id: i64) -> Element {
    let mut view: Loaded = use_signal(|| None);
    let busy = use_signal(|| false);
    let _load = use_resource(move || {
        // Neu laden, sobald der Server eine Änderung meldet
        let rev = TEAM_REV.read().get(&project_id).copied();
        async move {
            let _ = rev;
            match api::get::<TeamView>(&format!("/projects/{project_id}/team")).await {
                Ok(v) => view.set(Some(Ok(v))),
                Err(e) => view.set(Some(Err(e.message))),
            }
        }
    });
    match view() {
        None => rsx! { p { class: "p-4 text-xs text-muted-foreground", {tr("Lade …")} } },
        Some(Err(e)) => rsx! { p { class: "p-4 text-xs text-destructive", "{e}" } },
        Some(Ok(v)) if !v.enabled => rsx! {
            div { class: "min-h-0 flex-1 overflow-y-auto p-4",
                div { class: "flex items-start gap-3",
                    icons::Users { class: "mt-0.5 size-5 shrink-0 text-muted-foreground" }
                    div { class: "min-w-0 space-y-2 text-[13px] leading-relaxed",
                        p { class: "text-[14px] font-semibold", {tr("Zusammen an diesem Projekt arbeiten")} }
                        p { class: "text-muted-foreground",
                            {tr("Jeder arbeitet mit seinem eigenen nrzX am selben Repository – egal ob Mac, Windows, Linux oder Server. Ihr seht, woran die Agenten der anderen gerade sitzen, und die Agenten können sich gegenseitig schreiben, fragen und abstimmen.")}
                        }
                        p { class: "text-muted-foreground",
                            {tr("Es braucht keinen weiteren Server: Der Austausch läuft über das Repository selbst (refs/nrzx/team). Wer es lesen und schreiben darf, ist dabei – und sieht deinen Namen, die Titel und Branches deiner Chats hier und die Team-Nachrichten.")}
                        }
                        if v.available {
                            button {
                                class: BTN_PRIMARY,
                                disabled: busy(),
                                onclick: move |_| {
                                    spawn(change(project_id, TeamSettings { enabled: Some(true), ..Default::default() }, view, busy));
                                },
                                if busy() {
                                    icons::LoaderCircle { class: "size-3.5 animate-spin" }
                                }
                                {tr("Team einschalten")}
                            }
                        } else {
                            p { class: "text-warning", {tr("Dieses Projekt hat kein gemeinsames Repository (kein Remote „origin“).")} }
                        }
                    }
                }
            }
        },
        Some(Ok(v)) => rsx! { TeamOpen { project_id, v, view, busy } },
    }
}

fn status_dot(status: &str) -> &'static str {
    match status {
        "working" => "size-2 shrink-0 rounded-full bg-success animate-pulse",
        "waiting" => "size-2 shrink-0 rounded-full bg-warning",
        "error" => "size-2 shrink-0 rounded-full bg-destructive",
        _ => "size-2 shrink-0 rounded-full bg-muted-foreground/40",
    }
}

fn status_word(status: &str) -> String {
    match status {
        "working" => tr("arbeitet"),
        "waiting" => tr("wartet"),
        "error" => tr("Fehler"),
        _ => tr("ruht"),
    }
}

#[component]
fn TeamOpen(project_id: i64, v: TeamView, view: Loaded, busy: Signal<bool>) -> Element {
    let text = use_signal(String::new);
    let mut to = use_signal(|| "all".to_string());
    let mut menu = use_signal(|| false);
    let me = v.me.clone();
    let others = v.members.iter().filter(|m| m.name.to_lowercase() != me).count();
    // Empfänger: alle, jede Person, jeder Agent der anderen
    let mut targets: Vec<(String, String)> = vec![("all".to_string(), tr("Alle"))];
    for m in v.members.iter().filter(|m| m.name.to_lowercase() != me) {
        let key = m.name.to_lowercase();
        targets.push((key.clone(), m.name.clone()));
        for a in &m.agents {
            targets.push((address(&key, Some(a.id)), format!("{} · {}", address(&m.name, Some(a.id)), a.title)));
        }
    }
    if !targets.iter().any(|(k, _)| *k == to()) {
        to.set("all".to_string());
    }
    let submit = move || {
        spawn(send(project_id, to.peek().clone(), text, view, busy));
    };
    let mut messages = v.messages.clone();
    messages.reverse();
    rsx! {
        div { class: "flex h-11 shrink-0 items-center gap-1 px-3",
            span { class: "min-w-0 flex-1 truncate text-[14px] font-semibold",
                {tr("Team")}
                span { class: "ml-2 text-[12px] font-normal text-muted-foreground",
                    if v.synced_at > 0 {
                        {trf("abgeglichen {0}", &[&util::ago(v.synced_at)])}
                    }
                }
            }
            button { class: ICON_BTN, title: tr("Jetzt abgleichen"), disabled: busy(), onclick: move |_| { spawn(sync_now(project_id, view, busy)); },
                icons::RefreshCw { class: if busy() { "size-3.5 animate-spin" } else { "size-3.5" } }
            }
            button { class: ICON_BTN, title: tr("Einstellungen des Teams"), onclick: move |_| menu.set(!menu()), icons::Ellipsis { class: "size-4" } }
        }
        if menu() {
            div { class: "mx-3 mb-2 space-y-2 rounded-lg border p-3 text-[13px]",
                label { class: "flex cursor-pointer items-start gap-2",
                    input {
                        r#type: "checkbox",
                        class: "mt-0.5 size-4 shrink-0 accent-primary",
                        checked: v.auto_reply,
                        disabled: busy(),
                        onchange: move |e| {
                            spawn(change(project_id, TeamSettings { auto_reply: Some(e.checked()), ..Default::default() }, view, busy));
                        },
                    }
                    span {
                        span { class: "font-medium", {tr("Agenten antworten selbstständig")} }
                        span { class: "block text-[12px] leading-snug text-muted-foreground",
                            {tr("Schreibt ein Agent aus dem Team einen deiner Agenten direkt an, startet der von sich aus und antwortet. Sonst liest er die Nachricht erst bei deinem nächsten Auftrag.")}
                        }
                    }
                }
                button {
                    class: BTN,
                    disabled: busy(),
                    onclick: move |_| {
                        menu.set(false);
                        spawn(change(project_id, TeamSettings { enabled: Some(false), ..Default::default() }, view, busy));
                    },
                    {tr("Team für mich ausschalten")}
                }
            }
        }
        if !v.error.is_empty() {
            p { class: "mx-3 mb-2 rounded-md bg-destructive/10 px-2.5 py-1.5 text-[12px] leading-snug text-destructive", "{v.error}" }
        }
        div { class: "min-h-0 flex-1 overflow-y-auto px-3 pb-3",
            div { class: "space-y-2",
                for m in v.members.iter() {
                    MemberCard { key: "{m.name}", m: m.clone(), own: m.name.to_lowercase() == me, onpick: move |a: String| to.set(a) }
                }
            }
            if others == 0 {
                p { class: "mt-2 text-[12px] leading-snug text-muted-foreground",
                    {tr("Noch ist niemand sonst da. Wer dasselbe Repository in seinem nrzX als Projekt hat und dort „Team einschalten“ wählt, erscheint hier.")}
                }
            }
            p { class: "mb-1.5 mt-4 text-[11px] font-medium uppercase tracking-wide text-muted-foreground", {tr("Nachrichten")} }
            if messages.is_empty() {
                p { class: "text-[12px] text-muted-foreground", {tr("Noch keine Nachrichten.")} }
            }
            div { class: "space-y-2",
                for m in messages {
                    MessageRow { key: "{m.id}", own: m.from == me, m }
                }
            }
        }
        div { class: "shrink-0 border-t p-2",
            div { class: "rounded-lg border focus-within:border-ring/60",
                textarea {
                    class: "block max-h-40 min-h-[3.25rem] w-full resize-y bg-transparent px-3 pt-2 pb-1 text-[13.5px] leading-relaxed outline-none placeholder:text-muted-foreground",
                    rows: "2",
                    placeholder: tr("Ans Team schreiben … (Strg+Enter sendet)"),
                    value: "{text}",
                    oninput: {
                        let mut text = text;
                        move |e: FormEvent| text.set(e.value())
                    },
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter && (e.modifiers().ctrl() || e.modifiers().meta()) {
                            e.prevent_default();
                            submit();
                        }
                    },
                }
                div { class: "flex items-center gap-2 px-2 pb-2",
                    span { class: "shrink-0 text-[12px] text-muted-foreground", {tr("An")} }
                    select {
                        class: "h-7 min-w-0 flex-1 rounded-md border bg-transparent px-1.5 text-[12.5px] outline-none",
                        value: "{to}",
                        onchange: move |e| to.set(e.value()),
                        for (key , label) in targets {
                            option { key: "{key}", value: "{key}", selected: key == to(), "{label}" }
                        }
                    }
                    button { class: BTN_PRIMARY, disabled: busy() || text().trim().is_empty(), title: tr("Senden"), onclick: move |_| submit(), icons::SendHorizontal { class: "size-3.5" } }
                }
            }
        }
    }
}

#[component]
fn MemberCard(m: TeamMember, own: bool, onpick: EventHandler<String>) -> Element {
    let key = m.name.to_lowercase();
    rsx! {
        div { class: "rounded-lg border p-2.5",
            div { class: "flex items-center gap-2",
                icons::User { class: "size-3.5 shrink-0 text-muted-foreground" }
                span { class: "min-w-0 truncate text-[13.5px] font-medium", "{m.name}" }
                if !m.full_name.is_empty() && m.full_name != m.name {
                    span { class: "min-w-0 truncate text-[12px] text-muted-foreground", "{m.full_name}" }
                }
                if own {
                    span { class: "shrink-0 rounded-full bg-accent px-1.5 text-[11px] text-muted-foreground", {tr("du")} }
                }
                span { class: "flex-1" }
                span { class: "shrink-0 text-[11px] text-muted-foreground", title: tr("Zuletzt gemeldet"), {util::ago(m.seen_at)} }
            }
            if m.agents.is_empty() {
                p { class: "mt-1 pl-5.5 text-[12px] text-muted-foreground", {tr("Gerade kein Agent aktiv.")} }
            }
            for a in m.agents.iter() {
                {
                    let addr = address(&key, Some(a.id));
                    let word = status_word(&a.status);
                    rsx! {
                        button {
                            key: "{a.id}",
                            class: "mt-1 flex w-full min-w-0 items-center gap-2 rounded-md px-1 py-1 text-left hover:bg-accent/50 disabled:hover:bg-transparent",
                            disabled: own,
                            title: if own { format!("{addr} – {word}") } else { trf("{0} – {1}. Klicken, um diesem Agenten zu schreiben.", &[&addr, &word]) },
                            onclick: {
                                let addr = addr.clone();
                                move |_| onpick.call(addr.clone())
                            },
                            span { class: status_dot(&a.status) }
                            span { class: "min-w-0 flex-1",
                                span { class: "block truncate text-[13px]", if a.title.is_empty() { {tr("Neuer Chat")} } else { "{a.title}" } }
                                span { class: "block truncate text-[11.5px] text-muted-foreground", "#{a.id} · {a.agent} · {a.branch}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MessageRow(m: TeamMessage, own: bool) -> Element {
    let from = if m.from_title.is_empty() { m.sender() } else { format!("{} · {}", m.sender(), m.from_title) };
    let to = if m.to.is_empty() { tr("alle") } else { m.recipient() };
    rsx! {
        div { class: if own { "rounded-lg border bg-accent/40 p-2.5" } else { "rounded-lg border p-2.5" },
            div { class: "flex items-center gap-1.5 text-[11.5px] text-muted-foreground",
                if m.from_agent.is_some() {
                    icons::Bot { class: "size-3.5 shrink-0" }
                } else {
                    icons::User { class: "size-3.5 shrink-0" }
                }
                span { class: "min-w-0 truncate font-medium text-foreground", "{from}" }
                span { class: "shrink-0", "→" }
                span { class: "min-w-0 shrink-0 truncate", "{to}" }
                span { class: "flex-1" }
                span { class: "shrink-0", title: "{util::datetime(m.at)}", {util::ago(m.at)} }
            }
            p { class: "mt-1 whitespace-pre-wrap break-words text-[13px] leading-relaxed", "{m.text}" }
        }
    }
}
