//! Diff-Ansicht für Commits, Vergleiche und Pull Requests (Bereich Code).

use crate::components::common::*;
use crate::components::overlay::{Dropdown, MenuItem};
use crate::components::widgets::plural;
use crate::i18n::{tr, trf};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::{DiffFile, DiffHunk, DiffLine, DiffLineKind};
use std::collections::HashSet;

/// Eine Zeile im Diff, auf die man sich beziehen kann (Code-Kommentare in Reviews).
#[derive(Debug, Clone, PartialEq)]
pub struct LineRef {
    pub path: String,
    /// "old" (entfernte Zeile / linke Seite) oder "new"
    pub side: String,
    pub line: i64,
    pub kind: DiffLineKind,
}

/// Bezug einer Zeile: entfernte Zeilen auf der alten Seite, sonst auf der neuen.
fn line_ref(path: &str, l: &DiffLine) -> Option<LineRef> {
    let (side, line) = match l.kind {
        DiffLineKind::Del => ("old", l.old_line?),
        DiffLineKind::Add | DiffLineKind::Context => ("new", l.new_line?),
        DiffLineKind::Meta => return None,
    };
    Some(LineRef {
        path: path.to_string(),
        side: side.into(),
        line,
        kind: l.kind,
    })
}

const SPLIT_KEY: &str = "nrz-diff-split";

/// Abschnitt eines Elements in den sichtbaren Bereich holen.
fn jump(id: &str) {
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    else {
        return;
    };
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_behavior(web_sys::ScrollBehavior::Smooth);
    opts.set_block(web_sys::ScrollLogicalPosition::Start);
    el.scroll_into_view_with_scroll_into_view_options(&opts);
}

/// Alle Dateien eines Diffs: Kopf je Datei (Pfad, +/−, einklappbar), Zeilen mit Nummern und
/// Syntaxfarben; Ansicht vereint oder nebeneinander.
///
/// * `on_comment` – wenn gesetzt, erscheint beim Überfahren einer Zeile ein "+"-Knopf
///   (Code-Kommentar in einem Review).
/// * `extra` – Inhalt, der unter einer Zeile eingeblendet wird (z. B. Kommentar-Threads);
///   wird für jede Zeile aufgerufen und gibt `None` zurück, wenn dort nichts steht.
/// * `file_actions` – zusätzliche Knöpfe im Kopf einer Datei (z. B. "Datei ansehen").
#[component]
pub fn DiffView(
    files: Vec<DiffFile>,
    #[props(optional)] on_comment: Option<EventHandler<LineRef>>,
    #[props(optional)] extra: Option<Callback<LineRef, Option<Element>>>,
    #[props(optional)] file_actions: Option<Callback<DiffFile, Element>>,
) -> Element {
    let mut split = use_signal(|| util::storage_get(SPLIT_KEY).as_deref() == Some("1"));
    let mut collapsed = use_signal(HashSet::<usize>::new);
    let adds: i64 = files.iter().map(|f| f.additions).sum();
    let dels: i64 = files.iter().map(|f| f.deletions).sum();
    if files.is_empty() {
        return rsx! {
            EmptyState { title: tr("Keine Änderungen"), description: tr("Hier unterscheiden sich keine Dateien."), icon: rsx! { icons::FileDiff {} } }
        };
    }
    let mut set_split = move |v: bool| {
        split.set(v);
        util::storage_set(SPLIT_KEY, if v { "1" } else { "0" });
    };
    let seg = |active: bool| {
        if active {
            "inline-flex items-center gap-1.5 rounded-md bg-background px-2.5 py-1 text-[13px] font-medium shadow-xs"
        } else {
            "inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-[13px] text-muted-foreground hover:text-foreground"
        }
    };
    let all_collapsed = collapsed.read().len() == files.len();
    let count = files.len();
    rsx! {
        div { class: "space-y-4",
            div { class: "flex flex-wrap items-center gap-3 text-sm",
                Dropdown {
                    trigger_class: format!("{} {BTN_SM}", btn(BTN_OUTLINE)),
                    class: "max-h-96 w-96 overflow-y-auto",
                    trigger: rsx! {
                        icons::Files { class: "size-4" }
                        {plural(count as i64, "1 Datei geändert", "{0} Dateien geändert")}
                        icons::ChevronDown { class: "size-3.5 text-muted-foreground" }
                    },
                    for (i, f) in files.iter().enumerate() {
                        MenuItem {
                            key: "{i}",
                            onclick: move |_| {
                                collapsed.write().remove(&i);
                                jump(&format!("diff-{i}"));
                            },
                            StatusIcon { status: f.status.clone() }
                            span { class: "min-w-0 flex-1 truncate font-mono text-xs", {f.path().to_string()} }
                            span { class: "shrink-0 font-mono text-[11px] text-success", "+{f.additions}" }
                            span { class: "shrink-0 font-mono text-[11px] text-destructive", "−{f.deletions}" }
                        }
                    }
                }
                span { class: "font-mono text-[13px]",
                    span { class: "text-success", "+{adds}" }
                    " "
                    span { class: "text-destructive", "−{dels}" }
                }
                div { class: "ml-auto flex items-center gap-2",
                    button {
                        class: format!("{} {BTN_SM}", btn(BTN_GHOST)),
                        onclick: move |_| {
                            if all_collapsed {
                                collapsed.write().clear();
                            } else {
                                *collapsed.write() = (0..count).collect();
                            }
                        },
                        if all_collapsed {
                            icons::ChevronsUpDown { class: "size-4" }
                            {tr("Alle aufklappen")}
                        } else {
                            icons::ChevronsDownUp { class: "size-4" }
                            {tr("Alle einklappen")}
                        }
                    }
                    div { class: "inline-flex rounded-md bg-muted p-0.5",
                        button { r#type: "button", class: seg(!split()), onclick: move |_| set_split(false),
                            icons::Rows2 { class: "size-3.5" }
                            {tr("Vereint")}
                        }
                        button { r#type: "button", class: seg(split()), onclick: move |_| set_split(true),
                            icons::Columns2 { class: "size-3.5" }
                            {tr("Nebeneinander")}
                        }
                    }
                }
            }
            for (i, f) in files.iter().enumerate() {
                FileBlock {
                    key: "{i}-{f.path()}",
                    index: i,
                    file: f.clone(),
                    split: split(),
                    collapsed: collapsed.read().contains(&i),
                    on_toggle: move |_| {
                        let mut c = collapsed.write();
                        if !c.remove(&i) {
                            c.insert(i);
                        }
                    },
                    on_comment,
                    extra,
                    file_actions,
                }
            }
        }
    }
}

/// Symbol für die Art der Änderung.
#[component]
fn StatusIcon(#[props(into)] status: String) -> Element {
    rsx! {
        match status.as_str() {
            "added" => rsx! { icons::FilePlus { class: "size-4 shrink-0 text-success" } },
            "deleted" => rsx! { icons::FileMinus { class: "size-4 shrink-0 text-destructive" } },
            "renamed" | "copied" => rsx! { icons::FileSymlink { class: "size-4 shrink-0 text-info" } },
            _ => rsx! { icons::FilePen { class: "size-4 shrink-0 text-warning" } },
        }
    }
}

/// Kleiner Balken aus fünf Kästchen (Anteil + und −).
#[component]
fn ChangeBar(adds: i64, dels: i64) -> Element {
    let total = (adds + dels).max(1);
    let green = if adds + dels == 0 {
        0
    } else {
        ((adds as f64 / total as f64) * 5.0).round() as usize
    };
    let red = if adds + dels == 0 {
        0
    } else {
        5 - green.min(5)
    };
    rsx! {
        span { class: "inline-flex gap-px",
            for i in 0..5usize {
                span {
                    key: "{i}",
                    class: if i < green { "size-2 rounded-[1px] bg-success" } else if i < green + red { "size-2 rounded-[1px] bg-destructive" } else { "size-2 rounded-[1px] bg-muted" },
                }
            }
        }
    }
}

#[component]
fn FileBlock(
    index: usize,
    file: DiffFile,
    split: bool,
    collapsed: bool,
    on_toggle: EventHandler<()>,
    on_comment: Option<EventHandler<LineRef>>,
    extra: Option<Callback<LineRef, Option<Element>>>,
    file_actions: Option<Callback<DiffFile, Element>>,
) -> Element {
    let f = file.clone();
    let path = f.path().to_string();
    let renamed = (f.status == "renamed" || f.status == "copied") && f.old_path != f.new_path;
    let mode_change = !f.old_mode.is_empty() && !f.new_mode.is_empty() && f.old_mode != f.new_mode;
    rsx! {
        div { id: "diff-{index}", class: "scroll-mt-20 overflow-hidden rounded-xl border bg-card",
            div { class: "flex flex-wrap items-center gap-2 bg-muted/30 px-3 py-2 text-sm",
                class: if !collapsed { "border-b" } else { "" },
                button { class: "grid size-6 place-items-center rounded text-muted-foreground hover:bg-accent hover:text-foreground", onclick: move |_| on_toggle.call(()),
                    title: if collapsed { tr("Aufklappen") } else { tr("Einklappen") },
                    if collapsed {
                        icons::ChevronRight { class: "size-4" }
                    } else {
                        icons::ChevronDown { class: "size-4" }
                    }
                }
                StatusIcon { status: f.status.clone() }
                span { class: "min-w-0 truncate font-mono text-[13px]",
                    if renamed {
                        span { class: "text-muted-foreground", "{f.old_path}" }
                        " → "
                    }
                    span { class: "font-medium", "{path}" }
                }
                CopyPath { path: path.clone() }
                if mode_change {
                    span { class: "rounded border px-1 font-mono text-[11px] text-muted-foreground", "{f.old_mode} → {f.new_mode}" }
                }
                span { class: "font-mono text-xs",
                    span { class: "text-success", "+{f.additions}" }
                    " "
                    span { class: "text-destructive", "−{f.deletions}" }
                }
                ChangeBar { adds: f.additions, dels: f.deletions }
                div { class: "ml-auto flex items-center gap-1",
                    if let Some(actions) = file_actions {
                        {actions.call(f.clone())}
                    }
                }
            }
            if !collapsed {
                if f.binary {
                    p { class: "px-4 py-6 text-center text-sm text-muted-foreground", {tr("Binärdatei – Änderungen werden nicht angezeigt.")} }
                } else if f.too_large {
                    p { class: "px-4 py-6 text-center text-sm text-muted-foreground",
                        {trf("Der Diff ist zu gross zum Anzeigen (+{0} −{1}).", &[&f.additions, &f.deletions])}
                    }
                } else if f.hunks.is_empty() {
                    p { class: "px-4 py-6 text-center text-sm text-muted-foreground",
                        if renamed {
                            {tr("Datei umbenannt, ohne Änderungen am Inhalt.")}
                        } else if mode_change {
                            {tr("Nur der Dateimodus hat sich geändert.")}
                        } else {
                            {tr("Leere Datei.")}
                        }
                    }
                } else {
                    div { class: "overflow-x-auto",
                        table { class: "code-table",
                            tbody {
                                for (h, hunk) in f.hunks.iter().enumerate() {
                                    if split {
                                        SplitHunk { key: "{h}", path: path.clone(), hunk: hunk.clone(), on_comment, extra }
                                    } else {
                                        UnifiedHunk { key: "{h}", path: path.clone(), hunk: hunk.clone(), on_comment, extra }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CopyPath(path: String) -> Element {
    rsx! {
        crate::components::widgets::CopyButton { text: path, class: "text-muted-foreground hover:text-foreground [&_svg]:size-3.5" }
    }
}

/// Zeilennummer mit „+“-Knopf beim Überfahren.
#[component]
fn LineNo(
    no: Option<i64>,
    target: Option<LineRef>,
    on_comment: Option<EventHandler<LineRef>>,
    #[props(into, default)] class: String,
) -> Element {
    rsx! {
        td { class: "ln relative {class}",
            if let (Some(handler), Some(t)) = (on_comment, target.clone()) {
                button {
                    class: "absolute left-1 top-0.5 hidden size-4 place-items-center rounded bg-info text-white shadow group-hover:grid",
                    title: tr("Kommentar schreiben"),
                    onclick: move |_| handler.call(t.clone()),
                    icons::Plus { class: "size-3" }
                }
            }
            if let Some(n) = no {
                "{n}"
            }
        }
    }
}

/// Code einer Zeile: hervorgehoben (HTML) oder als Text.
fn code_cell(l: &DiffLine) -> Element {
    match &l.html {
        Some(h) if !h.is_empty() => rsx! { span { dangerous_inner_html: "{h}" } },
        _ if l.content.is_empty() => rsx! { " " },
        _ => rsx! { "{l.content}" },
    }
}

#[component]
fn UnifiedHunk(
    path: String,
    hunk: DiffHunk,
    on_comment: Option<EventHandler<LineRef>>,
    extra: Option<Callback<LineRef, Option<Element>>>,
) -> Element {
    rsx! {
        tr { class: "diff-hunk",
            td { class: "ln", "…" }
            td { class: "ln", "…" }
            td { colspan: "2", class: "code py-1 text-xs", "{hunk.header}" }
        }
        for (i, l) in hunk.lines.iter().enumerate() {
            {
                let target = line_ref(&path, l);
                let below = match (&extra, &target) {
                    (Some(cb), Some(t)) => cb.call(t.clone()),
                    _ => None,
                };
                let (row, sign) = match l.kind {
                    DiffLineKind::Add => ("group diff-add", "+"),
                    DiffLineKind::Del => ("group diff-del", "−"),
                    DiffLineKind::Meta => ("group text-muted-foreground italic", ""),
                    DiffLineKind::Context => ("group", ""),
                };
                rsx! {
                    tr { key: "{i}", class: "{row}",
                        LineNo { no: l.old_line, target: target.clone(), on_comment }
                        LineNo { no: l.new_line, target: None, on_comment: None }
                        td { class: "w-4 select-none pl-2 text-muted-foreground", "{sign}" }
                        td { class: "code", {code_cell(l)} }
                    }
                    if let Some(el) = below {
                        tr { key: "{i}-extra",
                            td { colspan: "4", class: "border-y bg-background px-4 py-3 font-sans", {el} }
                        }
                    }
                }
            }
        }
    }
}

/// Zeilenpaare für die Ansicht nebeneinander: Kontext auf beiden Seiten, entfernte und
/// hinzugefügte Blöcke paarweise.
fn pairs(lines: &[DiffLine]) -> Vec<(Option<DiffLine>, Option<DiffLine>)> {
    let mut out = Vec::new();
    let mut dels: Vec<DiffLine> = Vec::new();
    let mut adds: Vec<DiffLine> = Vec::new();
    let flush = |dels: &mut Vec<DiffLine>,
                 adds: &mut Vec<DiffLine>,
                 out: &mut Vec<(Option<DiffLine>, Option<DiffLine>)>| {
        let n = dels.len().max(adds.len());
        for i in 0..n {
            out.push((dels.get(i).cloned(), adds.get(i).cloned()));
        }
        dels.clear();
        adds.clear();
    };
    for l in lines {
        match l.kind {
            DiffLineKind::Del => {
                if !adds.is_empty() {
                    flush(&mut dels, &mut adds, &mut out);
                }
                dels.push(l.clone());
            }
            DiffLineKind::Add => adds.push(l.clone()),
            DiffLineKind::Context => {
                flush(&mut dels, &mut adds, &mut out);
                out.push((Some(l.clone()), Some(l.clone())));
            }
            DiffLineKind::Meta => {
                flush(&mut dels, &mut adds, &mut out);
                out.push((Some(l.clone()), None));
            }
        }
    }
    flush(&mut dels, &mut adds, &mut out);
    out
}

#[component]
fn SplitHunk(
    path: String,
    hunk: DiffHunk,
    on_comment: Option<EventHandler<LineRef>>,
    extra: Option<Callback<LineRef, Option<Element>>>,
) -> Element {
    let rows = pairs(&hunk.lines);
    let cell = |l: &Option<DiffLine>| match l.as_ref().map(|l| l.kind) {
        Some(DiffLineKind::Add) => ("bg-diff-add-strong", "bg-diff-add"),
        Some(DiffLineKind::Del) => ("bg-diff-del-strong", "bg-diff-del"),
        None => ("bg-muted/30", "bg-muted/30"),
        _ => ("", ""),
    };
    rsx! {
        tr { class: "diff-hunk",
            td { class: "ln", "…" }
            td { colspan: "3", class: "code py-1 text-xs", "{hunk.header}" }
        }
        for (i, (left, right)) in rows.iter().enumerate() {
            {
                let context = left.as_ref().is_some_and(|l| l.kind == DiffLineKind::Context);
                let lt = left.as_ref().and_then(|l| line_ref(&path, l));
                let rt = if context { None } else { right.as_ref().and_then(|l| line_ref(&path, l)) };
                let mut below: Vec<Element> = Vec::new();
                if let Some(cb) = &extra {
                    for t in [lt.clone(), rt.clone()].into_iter().flatten() {
                        if let Some(el) = cb.call(t) {
                            below.push(el);
                        }
                    }
                }
                let (lln, lcode) = cell(left);
                let (rln, rcode) = cell(right);
                rsx! {
                    tr { key: "{i}", class: "group",
                        LineNo { no: left.as_ref().and_then(|l| l.old_line.or(if l.kind == DiffLineKind::Meta { None } else { l.new_line })), target: lt.clone(), on_comment, class: lln }
                        td { class: "code w-1/2 {lcode}",
                            if let Some(l) = left {
                                {code_cell(l)}
                            }
                        }
                        LineNo { no: right.as_ref().and_then(|l| l.new_line), target: rt.clone(), on_comment, class: "border-l {rln}" }
                        td { class: "code w-1/2 {rcode}",
                            if let Some(l) = right {
                                {code_cell(l)}
                            }
                        }
                    }
                    for (j, el) in below.into_iter().enumerate() {
                        tr { key: "{i}-extra-{j}",
                            td { colspan: "4", class: "border-y bg-background px-4 py-3 font-sans", {el} }
                        }
                    }
                }
            }
        }
    }
}
