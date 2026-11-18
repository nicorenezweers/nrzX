//! „Open in editor“: Dateibaum des Arbeitsordners (Ordner werden beim Aufklappen geladen,
//! Schnellsuche über alle Dateien) und ein einfacher Editor für Textdateien.

use super::parts::BusyIcon;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::i18n::{tr, trf};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::workspace::{FileContent, FileEntry, FileWrite};

#[component]
pub fn EditorDialog(open: Signal<bool>, thread_id: i64, #[props(optional)] initial: Option<String>) -> Element {
    rsx! {
        Modal { open, title: tr("Dateien"), description: tr("Arbeitsordner dieses Threads – Änderungen sieht der Agent sofort."), class: "max-w-6xl",
            EditorBody { thread_id, initial }
        }
    }
}

#[component]
fn EditorBody(thread_id: i64, initial: Option<String>) -> Element {
    let mut current = use_signal(|| initial);
    let mut filter = use_signal(String::new);
    let mut tree_version = use_signal(|| 0u32);
    let mut new_open = use_signal(|| false);
    let mut new_name = use_signal(String::new);
    let all = use_resource(move || async move {
        let _ = tree_version();
        if filter().trim().is_empty() {
            return None;
        }
        api::get::<Vec<String>>(&format!("/workspace/{thread_id}/files/all")).await.ok()
    });
    let needle = filter().trim().to_lowercase();
    let matches: Vec<String> = match (&*all.read(), needle.is_empty()) {
        (Some(Some(list)), false) => list.iter().filter(|p| p.to_lowercase().contains(&needle)).take(200).cloned().collect(),
        _ => Vec::new(),
    };
    let onopen = move |p: String| current.set(Some(p));
    rsx! {
        div { class: "grid h-[70vh] min-h-0 gap-3 md:grid-cols-[16rem_minmax(0,1fr)]",
            div { class: "flex min-h-0 flex-col overflow-hidden rounded-lg border",
                div { class: "flex items-center gap-1 border-b p-1.5",
                    div { class: "relative min-w-0 flex-1",
                        icons::Search { class: "pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" }
                        input {
                            class: "h-7 w-full rounded-md bg-transparent pl-7 pr-2 text-[13px] outline-none placeholder:text-muted-foreground focus:bg-accent/40",
                            r#type: "search",
                            placeholder: tr("Datei suchen"),
                            value: "{filter}",
                            oninput: move |e| filter.set(e.value()),
                        }
                    }
                    button { class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground", title: tr("Neue Datei"), onclick: move |_| new_open.set(true),
                        icons::FilePlus { class: "size-4" }
                    }
                    button { class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground", title: tr("Neu laden"), onclick: move |_| tree_version += 1,
                        icons::RefreshCw { class: "size-3.5" }
                    }
                }
                div { class: "min-h-0 flex-1 overflow-y-auto p-1 text-[13px]",
                    if needle.is_empty() {
                        DirNode { key: "{tree_version}", thread_id, path: String::new(), depth: 0, current: current(), onopen }
                    } else if matches.is_empty() {
                        p { class: "px-2 py-3 text-xs text-muted-foreground", if all.read().is_none() { {tr("Lade …")} } else { {tr("Keine Treffer.")} } }
                    } else {
                        for p in matches {
                            button {
                                key: "{p}",
                                class: if current().as_deref() == Some(p.as_str()) { "flex w-full min-w-0 items-center gap-1.5 rounded bg-accent px-2 py-1 text-left" } else { "flex w-full min-w-0 items-center gap-1.5 rounded px-2 py-1 text-left hover:bg-accent/50" },
                                onclick: {
                                    let p = p.clone();
                                    move |_| current.set(Some(p.clone()))
                                },
                                icons::File { class: "size-3.5 shrink-0 text-muted-foreground" }
                                span { class: "truncate font-mono text-xs", title: "{p}", "{p}" }
                            }
                        }
                    }
                }
            }
            div { class: "flex min-h-0 flex-col overflow-hidden rounded-lg border",
                if let Some(path) = current() {
                    for p in std::iter::once(path) {
                        FileEditor { key: "{p}", thread_id, path: p.clone() }
                    }
                } else {
                    div { class: "grid flex-1 place-items-center p-6 text-center text-sm text-muted-foreground",
                        div {
                            icons::FileCode { class: "mx-auto mb-2 size-8" }
                            {tr("Links eine Datei wählen.")}
                        }
                    }
                }
            }
        }
        Modal { open: new_open, title: tr("Neue Datei"), description: tr("Pfad relativ zum Arbeitsordner, z. B. src/neu.rs."),
            form {
                class: "grid gap-4",
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    let path = new_name().trim().trim_start_matches('/').to_string();
                    spawn(async move {
                        match api::put::<serde_json::Value, _>(&format!("/workspace/{thread_id}/file"), &FileWrite { path: path.clone(), content: String::new() }).await {
                            Ok(_) => {
                                new_open.set(false);
                                new_name.set(String::new());
                                tree_version += 1;
                                current.set(Some(path));
                            }
                            Err(e) => toast_error(e),
                        }
                    });
                },
                Field { label: tr("Pfad"),
                    input { class: "{INPUT} font-mono", autofocus: true, required: true, value: "{new_name}", oninput: move |e| new_name.set(e.value()) }
                }
                div { class: "flex justify-end gap-2",
                    button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| new_open.set(false), {tr("Abbrechen")} }
                    button { r#type: "submit", class: btn(BTN_PRIMARY), {tr("Anlegen")} }
                }
            }
        }
    }
}

/// Ein Ordner im Baum: lädt seinen Inhalt, Unterordner klappen auf.
#[component]
fn DirNode(thread_id: i64, path: String, depth: u32, current: Option<String>, onopen: EventHandler<String>) -> Element {
    let p = path.clone();
    let entries = use_resource(move || {
        let p = p.clone();
        async move { api::get::<Vec<FileEntry>>(&format!("/workspace/{thread_id}/files?path={}", api::enc(&p))).await }
    });
    let pad = depth * 12 + 6;
    rsx! {
        match entries.read().clone() {
            None => rsx! { p { class: "py-1 text-xs text-muted-foreground", style: "padding-left: {pad}px", {tr("Lade …")} } },
            Some(Err(e)) => rsx! { p { class: "py-1 text-xs text-destructive", style: "padding-left: {pad}px", "{e.message}" } },
            Some(Ok(list)) => rsx! {
                if list.is_empty() && depth == 0 {
                    p { class: "px-2 py-3 text-xs text-muted-foreground", {tr("Der Ordner ist leer.")} }
                }
                for e in list {
                    if e.dir {
                        DirEntry { key: "{e.path}", thread_id, entry: e.clone(), depth, current: current.clone(), onopen }
                    } else {
                        button {
                            key: "{e.path}",
                            class: if current.as_deref() == Some(e.path.as_str()) { "flex w-full min-w-0 items-center gap-1.5 rounded bg-accent py-1 pr-2 text-left" } else { "flex w-full min-w-0 items-center gap-1.5 rounded py-1 pr-2 text-left hover:bg-accent/50" },
                            style: "padding-left: {pad + 14}px",
                            onclick: {
                                let p = e.path.clone();
                                move |_| onopen.call(p.clone())
                            },
                            icons::File { class: "size-3.5 shrink-0 text-muted-foreground" }
                            span { class: "truncate", "{e.name}" }
                        }
                    }
                }
            },
        }
    }
}

#[component]
fn DirEntry(thread_id: i64, entry: FileEntry, depth: u32, current: Option<String>, onopen: EventHandler<String>) -> Element {
    let mut open = use_signal(|| false);
    let pad = depth * 12 + 6;
    rsx! {
        button { class: "flex w-full min-w-0 items-center gap-1 rounded py-1 pr-2 text-left hover:bg-accent/50", style: "padding-left: {pad}px", onclick: move |_| open.toggle(),
            if open() {
                icons::ChevronDown { class: "size-3.5 shrink-0 text-muted-foreground" }
                icons::FolderOpen { class: "size-3.5 shrink-0 text-muted-foreground" }
            } else {
                icons::ChevronRight { class: "size-3.5 shrink-0 text-muted-foreground" }
                icons::Folder { class: "size-3.5 shrink-0 text-muted-foreground" }
            }
            span { class: "truncate", "{entry.name}" }
        }
        if open() {
            DirNode { thread_id, path: entry.path.clone(), depth: depth + 1, current, onopen }
        }
    }
}

/// Einfacher Editor: Text bearbeiten und speichern (Strg/⌘+S).
#[component]
fn FileEditor(thread_id: i64, path: String) -> Element {
    let p = path.clone();
    let file = use_resource(move || {
        let p = p.clone();
        async move { api::get::<FileContent>(&format!("/workspace/{thread_id}/file?path={}", api::enc(&p))).await }
    });
    let mut text = use_signal(|| None::<String>);
    let mut saved = use_signal(String::new);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        if let Some(Ok(f)) = &*file.read() {
            if text.peek().is_none() && !f.binary && !f.too_large {
                text.set(Some(f.content.clone()));
                saved.set(f.content.clone());
            }
        }
    });
    let save_path = path.clone();
    let save = use_callback(move |_: ()| {
        let Some(content) = text() else { return };
        let path = save_path.clone();
        busy.set(true);
        spawn(async move {
            match api::put::<serde_json::Value, _>(&format!("/workspace/{thread_id}/file"), &FileWrite { path, content: content.clone() }).await {
                Ok(_) => {
                    saved.set(content);
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    });
    let dirty = text().is_some_and(|t| t != saved());
    let info = match &*file.read() {
        Some(Ok(f)) if f.binary => Some(trf("Binärdatei ({0}) – hier nicht bearbeitbar.", &[&util::size(f.size)])),
        Some(Ok(f)) if f.too_large => Some(trf("Zu gross zum Bearbeiten ({0}).", &[&util::size(f.size)])),
        Some(Err(e)) => Some(e.message.clone()),
        _ => None,
    };
    rsx! {
        div { class: "flex h-10 shrink-0 items-center gap-2 border-b px-3",
            icons::FileCode { class: "size-4 shrink-0 text-muted-foreground" }
            span { class: "min-w-0 flex-1 truncate font-mono text-xs", title: "{path}", "{path}" }
            if dirty {
                span { class: "size-2 shrink-0 rounded-full bg-warning", title: tr("Nicht gespeichert") }
            }
            button { class: "{btn(BTN_PRIMARY)} {BTN_SM}", disabled: !dirty || busy(), onclick: move |_| save.call(()),
                BusyIcon { busy: busy() }
                {tr("Speichern")}
            }
        }
        if let Some(i) = info {
            div { class: "p-4", Callout { text: i } }
        } else if let Some(t) = text() {
            textarea {
                class: "min-h-0 w-full flex-1 resize-none bg-transparent p-3 font-mono text-[12.5px] leading-5 outline-none",
                spellcheck: false,
                value: "{t}",
                oninput: move |e| text.set(Some(e.value())),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Character("s".into()) && (e.modifiers().ctrl() || e.modifiers().meta()) {
                        e.prevent_default();
                        save.call(());
                    }
                },
            }
        } else {
            Loading {}
        }
    }
}
