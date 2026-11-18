//! Git-Dialoge der rechten Spalte: Branch wechseln/anlegen, Änderungen ansehen (Diff mit
//! Syntaxfarben), Commit (alle oder ausgewählte Dateien, optional mit Push) und Pull Request in
//! nrzGit.

use super::parts::*;
use crate::api;
use crate::components::common::*;
use crate::components::diff::DiffView;
use crate::components::overlay::*;
use crate::i18n::{tr, trf};
use crate::state::THREADS;
use dioxus::prelude::*;
use nrzx_shared::Diff;
use nrzx_shared::workspace::{BranchList, BranchRequest, CommitRequest, CommitResult, PullRequestRef, PullRequestRequest, WorkspaceStatus};
use std::collections::BTreeSet;

// ------------------------------------------------------------------ Branches

/// Inhalt des Branch-Menüs (lädt beim Öffnen).
#[component]
pub fn BranchMenu(thread_id: i64, onnew: EventHandler<()>) -> Element {
    let list = use_resource(move || async move { api::get::<BranchList>(&format!("/workspace/{thread_id}/branches")).await });
    let switch = move |name: String| {
        spawn(async move {
            if let Err(e) = api::post::<WorkspaceStatus, _>(&format!("/workspace/{thread_id}/branch"), &BranchRequest { name, create: false }).await {
                toast_error(e);
            }
            git_refresh();
        });
    };
    rsx! {
        match list.read().clone() {
            None => rsx! { p { class: "px-2 py-2 text-xs text-muted-foreground", {tr("Lade …")} } },
            Some(Err(e)) => rsx! { p { class: "px-2 py-2 text-xs text-destructive", "{e.message}" } },
            Some(Ok(l)) => rsx! {
                MenuLabel { {tr("Branch wechseln")} }
                for b in l.branches.iter().cloned() {
                    if b.checked_out_elsewhere {
                        div { key: "{b.name}", class: "flex items-center gap-2 px-2 py-1.5 text-sm text-muted-foreground/70", title: tr("In einem anderen Worktree ausgecheckt"),
                            icons::GitBranch { class: "size-4 shrink-0" }
                            span { class: "min-w-0 flex-1 truncate", "{b.name}" }
                            span { class: "text-[10px]", {tr("Worktree")} }
                        }
                    } else {
                        MenuItem {
                            key: "{b.name}",
                            active: b.current,
                            onclick: {
                                let name = b.name.clone();
                                let current = b.current;
                                move |_| {
                                    if !current {
                                        switch(name.clone());
                                    }
                                }
                            },
                            if b.current {
                                icons::Check {}
                            } else {
                                icons::GitBranch {}
                            }
                            span { class: "min-w-0 flex-1 truncate", "{b.name}" }
                            if b.remote_only {
                                span { class: "text-[10px] text-muted-foreground", "origin" }
                            }
                            if b.name == l.default_branch {
                                span { class: "text-[10px] text-muted-foreground", {tr("Standard")} }
                            }
                        }
                    }
                }
                MenuSeparator {}
                MenuItem { onclick: move |_| onnew.call(()),
                    icons::Plus {}
                    {tr("Neuer Branch …")}
                }
            },
        }
    }
}

#[component]
pub fn NewBranchDialog(open: Signal<bool>, thread_id: i64) -> Element {
    rsx! {
        Modal { open, title: tr("Neuer Branch"), description: tr("Vom aktuellen Stand aus; ungespeicherte Änderungen wandern mit."),
            NewBranchForm { open, thread_id }
        }
    }
}

#[component]
fn NewBranchForm(open: Signal<bool>, thread_id: i64) -> Element {
    let mut name = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let res = api::post::<WorkspaceStatus, _>(&format!("/workspace/{thread_id}/branch"), &BranchRequest { name: name().trim().to_string(), create: true }).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    open.set(false);
                    git_refresh();
                }
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        form { class: "grid gap-4", onsubmit: submit,
            Field { label: tr("Name"),
                input { class: "{INPUT} font-mono", autofocus: true, required: true, placeholder: "feature/neu", value: "{name}", oninput: move |e| name.set(e.value()) }
            }
            div { class: "flex justify-end gap-2",
                button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| open.set(false), {tr("Abbrechen")} }
                button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy() || name().trim().is_empty(),
                    BusyIcon { busy: busy() }
                    {tr("Anlegen und wechseln")}
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Änderungen

#[component]
pub fn ChangesDialog(open: Signal<bool>, thread_id: i64, oncommit: EventHandler<()>) -> Element {
    rsx! {
        Modal { open, title: tr("Änderungen"), description: tr("Alles, was dieser Thread gegenüber dem Standard-Branch ändert – auch neue Dateien."), class: "max-w-6xl",
            ChangesBody { thread_id, oncommit }
        }
    }
}

#[component]
fn ChangesBody(thread_id: i64, oncommit: EventHandler<()>) -> Element {
    // Zahl „Changes“ in der rechten Spalte auffrischen
    use_effect(git_refresh);
    let diff = use_resource(move || async move { api::get::<Diff>(&format!("/workspace/{thread_id}/diff")).await });
    rsx! {
        match diff.read().clone() {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { Callout { text: e.message, error: true } },
            Some(Ok(d)) => rsx! {
                if !d.files.is_empty() {
                    div { class: "mb-4 flex justify-end",
                        button { class: "{btn(BTN_PRIMARY)} {BTN_SM}", onclick: move |_| oncommit.call(()),
                            icons::GitCommitHorizontal {}
                            {tr("Committen …")}
                        }
                    }
                }
                if d.truncated {
                    div { class: "mb-4", Callout { text: tr("Es gibt mehr geänderte Dateien, als hier angezeigt werden.") } }
                }
                DiffView { files: d.files }
            },
        }
    }
}

// ------------------------------------------------------------------ Commit

#[component]
pub fn CommitDialog(open: Signal<bool>, status: WorkspaceStatus, push: bool) -> Element {
    let title = if push { tr("Commit & Push") } else { tr("Commit") };
    rsx! {
        Modal { open, title, description: trf("Auf Branch „{0}“.", &[&status.branch]), class: "max-w-xl",
            FreshStatus { thread_id: status.thread_id, render: move |s: WorkspaceStatus| rsx! { CommitForm { open, status: s, push } } }
        }
    }
}

#[component]
fn CommitForm(open: Signal<bool>, status: WorkspaceStatus, push: bool) -> Element {
    let files = status.uncommitted.clone();
    let id = status.thread_id;
    let mut message = use_signal(String::new);
    let mut selected = use_signal(|| files.iter().map(|f| f.path.clone()).collect::<BTreeSet<String>>());
    let mut do_push = use_signal(|| push);
    let mut busy = use_signal(|| false);
    let total = files.len();
    let all = selected.read().len() == total;
    let has_remote = status.has_remote;
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let chosen: Vec<String> = if selected.read().len() == total { Vec::new() } else { selected.read().iter().cloned().collect() };
            let body = CommitRequest { message: message(), files: chosen, push: do_push() && has_remote };
            let res = api::post::<CommitResult, _>(&format!("/workspace/{id}/commit"), &body).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    open.set(false);
                    git_refresh();
                }
                Err(e) => {
                    toast_error(e);
                    git_refresh();
                }
            }
        });
    };
    rsx! {
        form { class: "grid gap-4", onsubmit: submit,
            Field { label: tr("Nachricht"),
                textarea { class: "{TEXTAREA} min-h-24", autofocus: true, required: true, placeholder: tr("Was ändert sich?"), value: "{message}", oninput: move |e| message.set(e.value()) }
            }
            if total == 0 {
                Callout { text: tr("Keine Änderungen zum Committen.") }
            } else {
                div {
                    div { class: "mb-2 flex items-center justify-between text-sm",
                        span { class: "font-medium", {trf("Dateien ({0}/{1})", &[&selected.read().len(), &total])} }
                        button {
                            r#type: "button",
                            class: "text-xs text-muted-foreground hover:text-foreground",
                            onclick: {
                                let files = files.clone();
                                move |_| {
                                    if all {
                                        selected.write().clear();
                                    } else {
                                        *selected.write() = files.iter().map(|f| f.path.clone()).collect();
                                    }
                                }
                            },
                            if all { {tr("Keine")} } else { {tr("Alle")} }
                        }
                    }
                    div { class: "max-h-56 divide-y overflow-y-auto rounded-lg border",
                        for f in files.iter().cloned() {
                            label { key: "{f.path}", class: "flex cursor-pointer items-center gap-2.5 px-3 py-1.5 text-sm hover:bg-accent/30",
                                input {
                                    r#type: "checkbox",
                                    class: "size-4 accent-[var(--primary)]",
                                    checked: selected.read().contains(&f.path),
                                    onchange: {
                                        let p = f.path.clone();
                                        move |_| {
                                            let mut s = selected.write();
                                            if !s.remove(&p) {
                                                s.insert(p.clone());
                                            }
                                        }
                                    },
                                }
                                FileStatusDot { status: f.status.clone() }
                                span { class: "min-w-0 flex-1 truncate font-mono text-xs", title: "{f.path}", "{f.path}" }
                                span { class: "shrink-0 font-mono text-[11px] text-success", "+{f.additions}" }
                                span { class: "shrink-0 font-mono text-[11px] text-destructive", "−{f.deletions}" }
                            }
                        }
                    }
                }
            }
            if has_remote {
                Toggle { checked: do_push(), onchange: move |v| do_push.set(v), label: tr("Danach pushen") }
            }
            div { class: "flex justify-end gap-2",
                button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| open.set(false), {tr("Abbrechen")} }
                button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy() || message().trim().is_empty() || selected.read().is_empty(),
                    BusyIcon { busy: busy() }
                    if do_push() && has_remote { {tr("Committen und pushen")} } else { {tr("Committen")} }
                }
            }
        }
    }
}

/// Farbpunkt für die Art der Änderung.
#[component]
fn FileStatusDot(#[props(into)] status: String) -> Element {
    let (c, label) = match status.as_str() {
        "added" => ("bg-success", tr("neu")),
        "deleted" => ("bg-destructive", tr("gelöscht")),
        "renamed" | "copied" => ("bg-info", tr("umbenannt")),
        _ => ("bg-warning", tr("geändert")),
    };
    rsx! { span { class: "size-2 shrink-0 rounded-full {c}", title: "{label}" } }
}

// ------------------------------------------------------------------ Pull Request

#[component]
pub fn PullRequestDialog(open: Signal<bool>, status: WorkspaceStatus) -> Element {
    rsx! {
        Modal { open, title: tr("Pull Request in nrzGit"), description: trf("„{0}“ wird gepusht und als Pull Request in {1} angelegt.", &[&status.branch, &status.repo]), class: "max-w-xl",
            FreshStatus { thread_id: status.thread_id, render: move |s: WorkspaceStatus| rsx! { PullRequestForm { open, status: s } } }
        }
    }
}

#[component]
fn PullRequestForm(open: Signal<bool>, status: WorkspaceStatus) -> Element {
    let id = status.thread_id;
    let initial_title = THREADS.read().get(&id).map(thread_title).unwrap_or_else(|| status.title.clone());
    let mut title = use_signal(|| initial_title);
    let mut body = use_signal(String::new);
    let mut base = use_signal(|| status.default_branch.clone());
    let mut draft = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut done = use_signal(|| None::<PullRequestRef>);
    if let Some(pr) = done() {
        return rsx! {
            div { class: "grid gap-4",
                div { class: "flex items-start gap-3 rounded-lg border border-success/40 bg-success/10 p-4",
                    icons::GitPullRequest { class: "mt-0.5 size-5 shrink-0 text-success" }
                    div { class: "min-w-0",
                        p { class: "font-medium", {trf("Pull Request #{0} angelegt", &[&pr.number])} }
                        p { class: "mt-1 truncate text-sm text-muted-foreground", "{pr.title}" }
                    }
                }
                div { class: "flex justify-end gap-2",
                    button { class: btn(BTN_OUTLINE), onclick: move |_| open.set(false), {tr("Schliessen")} }
                    ExternalLink { href: pr.url.clone(), class: btn(BTN_PRIMARY),
                        icons::ExternalLink {}
                        {tr("In nrzGit öffnen")}
                    }
                }
            }
        };
    }
    let nothing = status.files.is_empty();
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let req = PullRequestRequest { title: title(), body: body(), base: base(), draft: draft() };
            let res = api::post::<PullRequestRef, _>(&format!("/workspace/{id}/pull-request"), &req).await;
            busy.set(false);
            match res {
                Ok(pr) => {
                    done.set(Some(pr));
                    git_refresh();
                }
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        form { class: "grid gap-4", onsubmit: submit,
            if nothing {
                Callout { text: tr("Auf diesem Branch gibt es noch keine Änderungen gegenüber dem Standard-Branch.") }
            } else if !status.uncommitted.is_empty() {
                Callout { text: tr("Es gibt nicht committete Änderungen – nur Commits landen im Pull Request.") }
            }
            Field { label: tr("Titel"),
                input { class: INPUT, autofocus: true, required: true, value: "{title}", oninput: move |e| title.set(e.value()) }
            }
            Field { label: tr("Beschreibung"),
                textarea { class: "{TEXTAREA} min-h-28", placeholder: tr("Was und warum (Markdown)"), value: "{body}", oninput: move |e| body.set(e.value()) }
            }
            div { class: "grid gap-4 sm:grid-cols-2",
                Field { label: tr("Ziel-Branch"),
                    input { class: "{INPUT} font-mono", value: "{base}", oninput: move |e| base.set(e.value()) }
                }
                Field { label: tr("Quell-Branch"),
                    input { class: "{INPUT} font-mono", value: "{status.branch}", disabled: true }
                }
            }
            Checkbox { checked: draft(), onchange: move |v| draft.set(v), label: tr("Als Entwurf"), hint: tr("Noch nicht bereit zum Prüfen.") }
            div { class: "flex justify-end gap-2",
                button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| open.set(false), {tr("Abbrechen")} }
                button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy() || title().trim().is_empty(),
                    BusyIcon { busy: busy() }
                    {tr("Pull Request anlegen")}
                }
            }
        }
    }
}

/// Frischen Zustand des Arbeitsordners laden (Dialoge sollen nicht mit einem alten Stand arbeiten).
#[component]
fn FreshStatus(thread_id: i64, render: Callback<WorkspaceStatus, Element>) -> Element {
    let status = use_resource(move || async move { api::get::<WorkspaceStatus>(&format!("/workspace/{thread_id}/status")).await });
    match status.read().clone() {
        None => rsx! { Loading {} },
        Some(Err(e)) => rsx! { Callout { text: e.message, error: true } },
        Some(Ok(s)) => render.call(s),
    }
}
