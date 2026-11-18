//! Einstellungen → Werkzeuge: Agent-CLIs (npm-Pakete) und die Docker CLI mit Version und
//! Herkunft, Installieren/Aktualisieren mit Live-Ausgabe.

use super::data::reload_agents;
use super::icons::AgentIcon;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::toast_error;
use crate::i18n::{tr, trf};
use crate::state::ME;
use dioxus::prelude::*;
use nrzx_shared::agents::{ToolInfo, ToolLatest};
use nrzx_shared::AgentKind;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Einstellungen → Werkzeuge: Agent-CLIs installieren und aktualisieren.
#[component]
pub fn ToolsSettings() -> Element {
    let mut tools = use_signal(|| None::<Vec<ToolInfo>>);
    let mut checking = use_signal(|| false);
    let reload = move || {
        spawn(async move {
            match api::get::<Vec<ToolInfo>>("/tools").await {
                Ok(l) => tools.set(Some(l)),
                Err(e) => toast_error(e),
            }
        });
    };
    use_hook(move || reload());
    let check_updates = move |_| {
        checking.set(true);
        spawn(async move {
            match api::get::<Vec<ToolLatest>>("/tools/latest").await {
                Ok(list) => {
                    if let Some(t) = tools.write().as_mut() {
                        for l in list {
                            if let Some(x) = t.iter_mut().find(|x| x.key == l.key) {
                                x.latest = l.latest;
                            }
                        }
                    }
                }
                Err(e) => toast_error(e),
            }
            checking.set(false);
        });
    };
    let admin = ME.read().as_ref().is_some_and(|m| m.is_admin);
    rsx! {
        div { class: "flex flex-col gap-6",
            Section {
                title: tr("Werkzeuge"),
                description: tr("nrzX installiert die Agent-CLIs in seinen Datenordner (tools/npm) und sucht Programme zuerst dort, dann im PATH. Fehlt Node.js, lädt nrzX es vorher nach tools/node. Die Docker CLI landet in tools/bin – damit können Agenten Images bauen und in die Registry von nrzGit schieben."),
                actions: rsx! {
                    button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", disabled: checking(), onclick: check_updates,
                        if checking() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else {
                            icons::RefreshCw { class: "size-4" }
                        }
                        {tr("Nach Updates suchen")}
                    }
                },
                if !admin {
                    div { class: "mb-4",
                        Callout { text: tr("Installieren und Aktualisieren dürfen nur Administratoren.") }
                    }
                }
                match tools() {
                    None => rsx! { Loading {} },
                    Some(list) => rsx! {
                        div { class: "divide-y",
                            for t in list {
                                ToolRow { key: "{t.key}", tool: t, admin, on_done: move |_| reload() }
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Protokoll-Strom einer Installation.
struct LogStream {
    source: web_sys::EventSource,
    _msg: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _done: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl Drop for LogStream {
    fn drop(&mut self) {
        self.source.close();
    }
}

#[component]
fn ToolRow(tool: ToolInfo, admin: bool, on_done: EventHandler<()>) -> Element {
    let mut log = use_signal(Vec::<String>::new);
    let mut running = use_signal(|| tool.installing);
    let mut show = use_signal(|| tool.installing);
    let mut finished = use_signal(|| None::<bool>);
    let stream: Rc<RefCell<Option<LogStream>>> = use_hook(|| Rc::new(RefCell::new(None)));
    let key = tool.key.clone();

    let follow = {
        let stream = stream.clone();
        let key = key.clone();
        move || {
            log.set(Vec::new());
            let Ok(source) = web_sys::EventSource::new(&format!("/api/v1/tools/{key}/stream")) else { return };
            let msg = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                if let Some(line) = e.data().as_string() {
                    let mut l = log.write();
                    l.push(line);
                    if l.len() > 400 {
                        l.remove(0);
                    }
                }
            });
            let st = stream.clone();
            let done = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                let ok = e.data().as_string().as_deref() == Some("ok");
                running.set(false);
                finished.set(Some(ok));
                // Strom schliessen (sonst verbindet der Browser neu)
                if let Some(s) = st.borrow().as_ref() {
                    s.source.close();
                }
            });
            source.set_onmessage(Some(msg.as_ref().unchecked_ref()));
            let _ = source.add_event_listener_with_callback("done", done.as_ref().unchecked_ref());
            *stream.borrow_mut() = Some(LogStream { source, _msg: msg, _done: done });
        }
    };
    // Bei laufender Installation gleich mitlesen
    {
        let mut follow = follow.clone();
        use_hook(move || {
            if tool.installing {
                follow();
            }
        });
    }
    {
        let stream = stream.clone();
        use_drop(move || {
            stream.borrow_mut().take();
        });
    }
    // Fertig: Liste und Agenten neu laden
    use_effect(move || {
        if finished().is_some() {
            on_done.call(());
            spawn(reload_agents());
        }
    });

    let install = {
        let key = key.clone();
        let follow = follow.clone();
        move |_| {
            let key = key.clone();
            running.set(true);
            show.set(true);
            finished.set(None);
            let mut follow = follow.clone();
            spawn(async move {
                match api::action::<ToolInfo>(&format!("/tools/{key}/install")).await {
                    Ok(_) => follow(),
                    Err(e) => {
                        running.set(false);
                        toast_error(e);
                    }
                }
            });
        }
    };

    let update = tool.installed && !tool.latest.is_empty() && !tool.version.is_empty() && tool.latest != tool.version;
    let source = match tool.source.as_str() {
        "nrzx" => tr("in nrzX installiert"),
        "path" => tr("aus dem PATH des Servers"),
        "env" => trf("über NRZX_BIN_{0}", &[&tool.key.to_uppercase()]),
        _ => String::new(),
    };
    rsx! {
        div { class: "py-3.5",
            div { class: "flex flex-wrap items-center gap-3",
                div { class: "grid size-9 shrink-0 place-items-center rounded-lg border bg-background",
                    if let Some(k) = AgentKind::parse(&tool.key) {
                        AgentIcon { kind: k, size: 20 }
                    } else {
                        icons::Container { class: "size-5 text-sky-400" }
                    }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-2",
                        span { class: "font-medium", "{tool.name}" }
                        if tool.installed {
                            span { class: "rounded-full bg-muted px-2 py-0.5 font-mono text-[11px]", if tool.version.is_empty() { "?" } else { "{tool.version}" } }
                        } else {
                            span { class: "rounded-full border px-2 py-0.5 text-[11px] text-muted-foreground", {tr("nicht installiert")} }
                        }
                        if update {
                            span { class: "rounded-full bg-info/15 px-2 py-0.5 text-[11px] text-info", {trf("Update: {0}", &[&tool.latest])} }
                        } else if tool.installed && !tool.latest.is_empty() {
                            span { class: "text-[11px] text-muted-foreground", {tr("aktuell")} }
                        }
                    }
                    div { class: "mt-0.5 flex flex-wrap gap-x-3 text-xs text-muted-foreground",
                        span { class: "font-mono", "{tool.package}" }
                        if !source.is_empty() {
                            span { "{source}" }
                        }
                        if !tool.path.is_empty() {
                            span { class: "truncate font-mono", "{tool.path}" }
                        }
                    }
                    if !tool.error.is_empty() && finished().is_none() {
                        p { class: "mt-1 text-xs text-destructive", {tr(&tool.error)} }
                    }
                }
                div { class: "flex items-center gap-1.5",
                    if !log().is_empty() {
                        button { class: "{btn(BTN_GHOST)} {BTN_SM}", onclick: move |_| show.toggle(),
                            icons::ScrollText { class: "size-4" }
                            {tr("Ausgabe")}
                        }
                    }
                    button {
                        class: if tool.installed { format!("{} {}", btn(BTN_OUTLINE), BTN_SM) } else { format!("{} {}", btn(BTN_PRIMARY), BTN_SM) },
                        disabled: !admin || running(),
                        onclick: install,
                        if running() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else if tool.installed {
                            icons::RefreshCw { class: "size-4" }
                        } else {
                            icons::Download { class: "size-4" }
                        }
                        if tool.installed { {tr("Aktualisieren")} } else { {tr("Installieren")} }
                    }
                }
            }
            if show() && (!log().is_empty() || running()) {
                pre { class: "mt-3 max-h-64 overflow-auto rounded-lg border bg-[#141414] px-3 py-2 font-mono text-[12px] leading-5 text-[#e5e5e5]",
                    for (i , l) in log().iter().enumerate() {
                        div { key: "{i}", class: if l.starts_with('✗') { "text-red-300" } else if l.starts_with('✓') { "text-green-300" } else { "" }, "{l}" }
                    }
                    if running() {
                        div { class: "text-white/40", "…" }
                    }
                }
            }
        }
    }
}
