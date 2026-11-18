//! Rechte Spalte der Thread-Seite mit Reitern wie in einem Editor: Dateien (Ordnerstruktur des
//! Arbeitsordners, geänderte Dateien farbig), Suche (Dateinamen), Übersicht (Umgebung, Git,
//! Vorschau, Subagenten) und Notizblock. Ein Klick auf eine Datei öffnet sie im Editor.

use super::editor::EditorDialog;
use super::parts::*;
use crate::api;
use crate::i18n::tr;
use crate::state::{PROJECTS, THREADS};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::workspace::{FileEntry, WorkspaceStatus};
use std::collections::HashMap;

/// Gewählter Reiter: "files" | "search" | "overview" | "notes" | "btw" | "team" (im Browser gemerkt).
pub static SIDE_TAB: GlobalSignal<String> = Signal::global(|| util::storage_get("nrz-side-tab").unwrap_or_else(|| "overview".into()));

pub fn set_side_tab(tab: &str) {
    *SIDE_TAB.write() = tab.to_string();
    util::storage_set("nrz-side-tab", tab);
}

const TAB: &str = "relative grid size-8 place-items-center rounded-md text-muted-foreground hover:text-foreground [&_svg]:size-[17px]";
const TAB_ON: &str = "relative grid size-8 place-items-center rounded-md text-foreground [&_svg]:size-[17px] after:absolute after:-bottom-[7px] after:left-1.5 after:right-1.5 after:h-0.5 after:rounded-full after:bg-foreground";

/// Kopf mit den Reitern und darunter der Inhalt. `overview` ist die bisherige Kartenansicht.
#[component]
pub fn SidePanel(thread_id: i64, status: Option<Result<WorkspaceStatus, api::ApiError>>, overview: Element, onclose: EventHandler<()>) -> Element {
    let tab = SIDE_TAB();
    let project_id = THREADS.read().get(&thread_id).and_then(|t| t.project_id);
    let tabs = [
        ("files", tr("Dateien"), rsx! { icons::Files {} }),
        ("search", tr("Dateien suchen"), rsx! { icons::Search {} }),
        ("overview", tr("Übersicht: Git, Vorschau, Subagenten"), rsx! { icons::GitBranch {} }),
        ("notes", tr("Notizblock"), rsx! { icons::NotebookPen {} }),
        ("btw", tr("Zwischenstand (/btw)"), rsx! { icons::MessagesSquare {} }),
        ("team", tr("Team: zusammen am Projekt, Agenten sprechen miteinander"), rsx! { icons::Users {} }),
    ];
    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            div { class: "flex h-11 shrink-0 items-center gap-1 border-b px-2",
                for (key , label , icon) in tabs {
                    button { key: "{key}", class: if tab == key { TAB_ON } else { TAB }, title: "{label}", onclick: move |_| set_side_tab(key), {icon} }
                }
                div { class: "flex-1" }
                button { class: TAB, title: tr("Rechte Spalte schliessen"), onclick: move |_| onclose.call(()), icons::PanelRight {} }
            }
            match tab.as_str() {
                "files" => rsx! { FilesTab { thread_id, status: status.clone() } },
                "search" => rsx! { SearchTab { thread_id } },
                "notes" => rsx! { crate::pages::agents::NotesPanel { thread_id: Some(thread_id), project_id } },
                "btw" => rsx! { crate::pages::agents::BtwPanel { thread_id } },
                "team" => rsx! { super::team::TeamPanel { project_id } },
                _ => rsx! {
                    div { class: "min-h-0 flex-1 overflow-y-auto p-3", {overview} }
                },
            }
        }
    }
}

/// Änderungsstatus je Pfad (aus dem Status des Arbeitsordners): "M", "A", "D", "R".
fn change_map(status: &Option<Result<WorkspaceStatus, api::ApiError>>) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Some(Ok(s)) = status {
        for f in s.files.iter().chain(s.uncommitted.iter()) {
            let k = match f.status.as_str() {
                "added" | "untracked" => "A",
                "deleted" => "D",
                "renamed" => "R",
                _ => "M",
            };
            m.insert(f.path.clone(), k.to_string());
        }
    }
    m
}

fn change_class(k: &str) -> &'static str {
    match k {
        "A" => "text-success",
        "D" => "text-destructive line-through",
        _ => "text-warning",
    }
}

/// Icon je Dateiart.
fn file_icon(name: &str) -> Element {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "kt" | "c" | "h" | "cpp" | "cs" | "rb" | "php" | "swift" | "sh" | "sql" | "css" | "scss" | "html" | "vue" | "svelte" => {
            rsx! { icons::FileCode { class: "size-4 shrink-0 text-muted-foreground" } }
        }
        "json" | "toml" | "yaml" | "yml" | "lock" | "xml" => rsx! { icons::FileJson { class: "size-4 shrink-0 text-muted-foreground" } },
        "md" | "txt" | "rst" => rsx! { icons::FileText { class: "size-4 shrink-0 text-muted-foreground" } },
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => rsx! { icons::Image { class: "size-4 shrink-0 text-muted-foreground" } },
        _ => rsx! { icons::File { class: "size-4 shrink-0 text-muted-foreground" } },
    }
}

#[component]
fn FilesTab(thread_id: i64, status: Option<Result<WorkspaceStatus, api::ApiError>>) -> Element {
    let mut version = use_signal(|| 0u32);
    let mut editor_open = use_signal(|| false);
    let mut editor_path = use_signal(|| None::<String>);
    let mut current = use_signal(|| None::<String>);
    let changes = change_map(&status);
    let t = THREADS.read().get(&thread_id).cloned();
    let title = t
        .as_ref()
        .and_then(|t| t.project_id.and_then(|p| PROJECTS.read().get(&p).map(|p| p.name.clone())))
        .unwrap_or_else(|| tr("Arbeitsordner"));
    let onopen = move |p: String| {
        current.set(Some(p.clone()));
        editor_path.set(Some(p));
        editor_open.set(true);
    };
    rsx! {
        div { class: "flex h-11 shrink-0 items-center gap-1 px-3",
            span { class: "min-w-0 flex-1 truncate text-[14px] font-semibold", "{title}" }
            if !changes.is_empty() {
                span { class: "shrink-0 rounded-full bg-warning/15 px-1.5 text-[11px] tabular-nums text-warning", title: tr("Geänderte Dateien"), "{changes.len()}" }
            }
            button {
                class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                title: tr("Neu laden"),
                onclick: move |_| {
                    version += 1;
                    git_refresh();
                },
                icons::RefreshCw { class: "size-3.5" }
            }
            button {
                class: "grid size-7 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                title: tr("Im Editor öffnen"),
                onclick: move |_| {
                    editor_path.set(None);
                    editor_open.set(true);
                },
                icons::Ellipsis { class: "size-4" }
            }
        }
        div { class: "min-h-0 flex-1 overflow-y-auto px-1.5 pb-3 text-[13.5px]",
            for v in std::iter::once(version()) {
                TreeDir { key: "{v}", thread_id, path: String::new(), depth: 0, current: current(), changes: changes.clone(), onopen }
            }
        }
        if editor_open() {
            EditorDialog { open: editor_open, thread_id, initial: editor_path() }
        }
    }
}

/// Ordnerinhalt (beim Aufklappen geladen).
#[component]
fn TreeDir(thread_id: i64, path: String, depth: u32, current: Option<String>, changes: HashMap<String, String>, onopen: EventHandler<String>) -> Element {
    let p = path.clone();
    let entries = use_resource(move || {
        let p = p.clone();
        async move { api::get::<Vec<FileEntry>>(&format!("/workspace/{thread_id}/files?path={}", api::enc(&p))).await }
    });
    let pad = depth * 14 + 6;
    rsx! {
        match entries.read().clone() {
            None => rsx! { p { class: "py-1 text-xs text-muted-foreground", style: "padding-left: {pad + 20}px", {tr("Lade …")} } },
            Some(Err(e)) => rsx! { p { class: "py-1 pr-2 text-xs text-destructive", style: "padding-left: {pad}px", "{e.message}" } },
            Some(Ok(list)) => rsx! {
                if list.is_empty() && depth == 0 {
                    p { class: "px-3 py-3 text-xs text-muted-foreground", {tr("Der Ordner ist leer.")} }
                }
                for e in list {
                    if e.dir {
                        TreeFolder { key: "{e.path}", thread_id, entry: e.clone(), depth, current: current.clone(), changes: changes.clone(), onopen }
                    } else {
                        {
                            let change = changes.get(&e.path).cloned();
                            let dim = e.name.starts_with('.');
                            let active = current.as_deref() == Some(e.path.as_str());
                            rsx! {
                                button {
                                    key: "{e.path}",
                                    class: if active { "flex h-7 w-full min-w-0 items-center gap-1.5 rounded-md bg-accent pr-2 text-left" } else { "flex h-7 w-full min-w-0 items-center gap-1.5 rounded-md pr-2 text-left hover:bg-accent/50" },
                                    style: "padding-left: {pad + 20}px",
                                    title: "{e.path}",
                                    onclick: {
                                        let p = e.path.clone();
                                        move |_| onopen.call(p.clone())
                                    },
                                    {file_icon(&e.name)}
                                    span {
                                        class: match (&change, dim) {
                                            (Some(k), _) => format!("min-w-0 flex-1 truncate {}", change_class(k)),
                                            (None, true) => "min-w-0 flex-1 truncate text-muted-foreground".to_string(),
                                            _ => "min-w-0 flex-1 truncate".to_string(),
                                        },
                                        "{e.name}"
                                    }
                                    if let Some(k) = change.clone() {
                                        span { class: "shrink-0 font-mono text-[11px] {change_class(&k)}", "{k}" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}

#[component]
fn TreeFolder(thread_id: i64, entry: FileEntry, depth: u32, current: Option<String>, changes: HashMap<String, String>, onopen: EventHandler<String>) -> Element {
    let key = format!("nrz-tree-{thread_id}");
    let mut open = use_signal({
        let key = key.clone();
        let path = entry.path.clone();
        move || util::storage_get(&key).is_some_and(|s| s.split('\n').any(|p| p == path))
    });
    let pad = depth * 14 + 6;
    let prefix = format!("{}/", entry.path);
    let changed_inside = changes.keys().any(|k| k.starts_with(&prefix));
    let dim = entry.name.starts_with('.') || matches!(entry.name.as_str(), "node_modules" | "target" | "dist" | "build");
    let toggle = {
        let path = entry.path.clone();
        move |_| {
            let v = !open();
            open.set(v);
            // Offene Ordner je Thread merken
            let mut list: Vec<String> = util::storage_get(&key).map(|s| s.split('\n').filter(|x| !x.is_empty()).map(String::from).collect()).unwrap_or_default();
            list.retain(|p| p != &path);
            if v {
                list.push(path.clone());
            }
            list.truncate(300);
            util::storage_set(&key, &list.join("\n"));
        }
    };
    rsx! {
        button {
            class: "flex h-7 w-full min-w-0 items-center gap-1.5 rounded-md pr-2 text-left hover:bg-accent/50",
            style: "padding-left: {pad}px",
            title: "{entry.path}",
            onclick: toggle,
            if open() {
                icons::ChevronDown { class: "size-3.5 shrink-0 text-muted-foreground" }
                icons::FolderOpen { class: "size-4 shrink-0 text-muted-foreground" }
            } else {
                icons::ChevronRight { class: "size-3.5 shrink-0 text-muted-foreground" }
                icons::Folder { class: "size-4 shrink-0 text-muted-foreground" }
            }
            span { class: if dim { "min-w-0 flex-1 truncate text-muted-foreground" } else if changed_inside { "min-w-0 flex-1 truncate text-warning" } else { "min-w-0 flex-1 truncate" }, "{entry.name}" }
            if changed_inside {
                span { class: "size-1.5 shrink-0 rounded-full bg-warning" }
            }
        }
        if open() {
            TreeDir { thread_id, path: entry.path.clone(), depth: depth + 1, current, changes, onopen }
        }
    }
}

/// Dateien nach Namen suchen.
#[component]
fn SearchTab(thread_id: i64) -> Element {
    let mut q = use_signal(String::new);
    let mut editor_open = use_signal(|| false);
    let mut editor_path = use_signal(|| None::<String>);
    let all = use_resource(move || async move { api::get::<Vec<String>>(&format!("/workspace/{thread_id}/files/all")).await });
    let needle = q().trim().to_lowercase();
    let (matches, total): (Vec<String>, usize) = match &*all.read() {
        Some(Ok(list)) if !needle.is_empty() => {
            let m: Vec<String> = list.iter().filter(|p| p.to_lowercase().contains(&needle)).cloned().collect();
            let n = m.len();
            (m.into_iter().take(300).collect(), n)
        }
        _ => (Vec::new(), 0),
    };
    rsx! {
        div { class: "shrink-0 p-3 pb-2",
            div { class: "relative",
                icons::Search { class: "pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" }
                input {
                    class: "h-8 w-full rounded-md border bg-transparent pl-8 pr-2 text-sm outline-none placeholder:text-muted-foreground focus:border-ring/60",
                    r#type: "search",
                    autofocus: true,
                    placeholder: tr("Dateiname oder Pfad"),
                    value: "{q}",
                    oninput: move |e| q.set(e.value()),
                }
            }
            if !needle.is_empty() {
                p { class: "mt-1.5 px-0.5 text-[11px] text-muted-foreground", {crate::i18n::trf("{0} Treffer", &[&total])} }
            }
        }
        div { class: "min-h-0 flex-1 overflow-y-auto px-1.5 pb-3",
            match &*all.read() {
                None => rsx! { p { class: "px-3 text-xs text-muted-foreground", {tr("Lade …")} } },
                Some(Err(e)) => rsx! { p { class: "px-3 text-xs text-destructive", "{e.message}" } },
                Some(Ok(_)) if needle.is_empty() => rsx! { p { class: "px-3 text-xs text-muted-foreground", {tr("Tippe, um Dateien im Arbeitsordner zu finden.")} } },
                _ => rsx! {
                    for p in matches {
                        {
                            let (dir, name) = p.rsplit_once('/').map(|(d, n)| (d.to_string(), n.to_string())).unwrap_or((String::new(), p.clone()));
                            rsx! {
                                button {
                                    key: "{p}",
                                    class: "flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left hover:bg-accent/50",
                                    onclick: {
                                        let p = p.clone();
                                        move |_| {
                                            editor_path.set(Some(p.clone()));
                                            editor_open.set(true);
                                        }
                                    },
                                    {file_icon(&name)}
                                    span { class: "shrink-0 text-[13px]", "{name}" }
                                    span { class: "min-w-0 truncate text-[11.5px] text-muted-foreground", "{dir}" }
                                }
                            }
                        }
                    }
                },
            }
        }
        if editor_open() {
            EditorDialog { open: editor_open, thread_id, initial: editor_path() }
        }
    }
}
