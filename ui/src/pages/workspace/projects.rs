//! Projekte: Liste mit Status, Projektseite (Repository in nrzGit, Ordner, letzte Commits,
//! Threads, neuer Thread im Projekt) und der Dialog „Projekt hinzufügen“ mit drei Reitern: aus
//! nrzGit übernehmen (mit Suche), neues Repository in nrzGit anlegen, lokales Projekt.

use super::parts::*;
use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::components::widgets::*;
use crate::i18n::{tr, trf};
use crate::state::{ME, PROJECTS, THREADS};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::workspace::{
    CreateLocalProject, CreateRepoProject, EditProject, GitResult, ImportProject, NrzGitOwner, NrzGitRepo, ProjectDetail,
};
use nrzx_shared::{ProjectSummary, ThreadSummary};

/// Projekt in den globalen Zustand übernehmen.
fn upsert(p: ProjectSummary) {
    PROJECTS.write().insert(p.id, p);
}

// ------------------------------------------------------------------ Liste

#[component]
pub fn Projects() -> Element {
    let mut list: Vec<ProjectSummary> = PROJECTS.read().values().cloned().collect();
    list.sort_by_key(|x| std::cmp::Reverse(x.last_activity_at));
    rsx! {
        div { class: "mx-auto w-full max-w-5xl p-4 md:p-6",
            PageHeader {
                title: tr("Projekte"),
                description: tr("Repositories aus nrzGit oder lokale Ordner, in denen die Agenten arbeiten."),
                actions: rsx! {
                    button { class: btn(BTN_PRIMARY), onclick: move |_| *ADD_PROJECT.write() = true,
                        icons::FolderPlus {}
                        {tr("Projekt hinzufügen")}
                    }
                },
            }
            if list.is_empty() {
                EmptyState {
                    title: tr("Noch keine Projekte"),
                    description: tr("Übernimm ein Repository aus nrzGit, lege ein neues an oder beginne lokal."),
                    icon: rsx! { icons::FolderGit2 {} },
                    button { class: btn(BTN_OUTLINE), onclick: move |_| *ADD_PROJECT.write() = true,
                        icons::FolderPlus {}
                        {tr("Projekt hinzufügen")}
                    }
                }
            } else {
                div { class: "grid gap-3 sm:grid-cols-2 lg:grid-cols-3",
                    for p in list {
                        ProjectCard { key: "{p.id}", project: p.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn ProjectCard(project: ProjectSummary) -> Element {
    let p = project;
    rsx! {
        Link { to: Route::ProjectPage { id: p.id }, class: "flex flex-col gap-3 rounded-xl border bg-card p-4 shadow-sm transition-colors hover:bg-accent/30",
            div { class: "flex items-center gap-3",
                ProjectBadge { name: Some(p.name.clone()), size: 32 }
                div { class: "min-w-0 flex-1",
                    p { class: "truncate font-medium", "{p.name}" }
                    p { class: "truncate text-xs text-muted-foreground",
                        if p.repo.is_empty() { {tr("Lokal")} } else { "{p.repo}" }
                    }
                }
                ProjectStatus { status: p.status.clone() }
            }
            if p.status == "error" && !p.status_message.is_empty() {
                p { class: "line-clamp-2 text-xs text-destructive", "{p.status_message}" }
            }
            div { class: "flex items-center gap-4 text-xs text-muted-foreground [&_svg]:size-3.5",
                span { class: "inline-flex items-center gap-1", icons::GitBranch {} "{p.default_branch}" }
                span { class: "inline-flex items-center gap-1", icons::MessagesSquare {}
                    if p.threads == 1 { {tr("1 Thread")} } else { {trf("{0} Threads", &[&p.threads])} }
                }
                span { class: "ml-auto", {short_ago(p.last_activity_at)} }
            }
        }
    }
}

// ------------------------------------------------------------------ Projektseite

#[component]
pub fn ProjectPage(id: i64) -> Element {
    rsx! { ProjectView { key: "{id}", id } }
}

#[component]
fn ProjectView(id: i64) -> Element {
    let nav = use_navigator();
    let mut version = use_signal(|| 0u64);
    // Neu laden, wenn sich der Zustand des Projekts ändert (z. B. fertig geklont)
    let live = PROJECTS.read().get(&id).cloned();
    let live_status = use_memo(move || PROJECTS.read().get(&id).map(|p| p.status.clone()));
    let detail = use_resource(move || async move {
        let _ = (live_status(), version());
        api::get::<ProjectDetail>(&format!("/projects/{id}")).await
    });
    let mut busy = use_signal(|| false);
    let mut rename_open = use_signal(|| false);
    let mut new_name = use_signal(String::new);

    let d = match detail.read().clone() {
        None => return rsx! { Loading {} },
        Some(Err(e)) => {
            return rsx! {
                div { class: "mx-auto max-w-xl p-6", Callout { text: e.message, error: true } }
            };
        }
        Some(Ok(d)) => d,
    };
    let p = live.unwrap_or_else(|| d.summary.clone());
    let mut threads: Vec<ThreadSummary> = THREADS.read().values().filter(|t| t.project_id == Some(id) && t.parent_id.is_none()).cloned().collect();
    threads.sort_by_key(|x| std::cmp::Reverse(x.last_activity_at));
    let name = p.name.clone();

    let refresh = move |_| {
        busy.set(true);
        spawn(async move {
            if let Err(e) = api::action::<GitResult>(&format!("/projects/{id}/refresh")).await {
                toast_error(e);
            }
            busy.set(false);
            version += 1;
        });
    };
    let remove = move |_| {
        let name = name.clone();
        spawn(async move {
            let ask = Ask::new(
                trf("Projekt „{0}“ löschen?", &[&name]),
                tr("Alle Threads des Projekts und die Arbeitsordner auf dem Server werden gelöscht, laufende Agenten angehalten. Das Repository in nrzGit bleibt erhalten."),
            )
            .confirm(tr("Löschen"))
            .destructive();
            if !confirm(ask).await {
                return;
            }
            match api::delete(&format!("/projects/{id}")).await {
                Ok(()) => {
                    PROJECTS.write().remove(&id);
                    THREADS.write().retain(|_, t| t.project_id != Some(id));
                    nav.replace(Route::Projects {});
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let save_name = move |e: FormEvent| {
        e.prevent_default();
        spawn(async move {
            match api::patch::<ProjectSummary, _>(&format!("/projects/{id}"), &EditProject { name: new_name() }).await {
                Ok(p) => {
                    upsert(p);
                    rename_open.set(false);
                }
                Err(e) => toast_error(e),
            }
        });
    };
    let ready = p.status == "ready";
    let current_name = p.name.clone();

    rsx! {
        div { class: "mx-auto w-full max-w-5xl p-4 md:p-6",
            div { class: "mb-6 flex flex-wrap items-center gap-4",
                ProjectBadge { name: Some(p.name.clone()), size: 44 }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-2",
                        h1 { class: "truncate text-2xl font-semibold tracking-tight", "{p.name}" }
                        ProjectStatus { status: p.status.clone() }
                    }
                    p { class: "mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground",
                        if d.web_url.is_empty() {
                            span { {tr("Lokales Projekt")} }
                        } else {
                            ExternalLink { href: d.web_url.clone(), class: "inline-flex items-center gap-1 hover:text-foreground",
                                icons::GitPullRequest { class: "size-3.5" }
                                "{p.repo}"
                                icons::ExternalLink { class: "size-3" }
                            }
                        }
                        span { class: "inline-flex items-center gap-1", icons::GitBranch { class: "size-3.5" } "{p.default_branch}" }
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    if ready {
                        button {
                            class: "{btn(BTN_PRIMARY)} {BTN_SM}",
                            onclick: move |_| {
                                *NEW_THREAD_PROJECT.write() = Some(id);
                                nav.push(Route::Home {});
                            },
                            icons::SquarePen {}
                            {tr("Neuer Thread")}
                        }
                    }
                    button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", disabled: busy() || p.status == "cloning", onclick: refresh,
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else {
                            icons::RefreshCw {}
                        }
                        if p.status == "error" { {tr("Erneut klonen")} } else { {tr("Aktualisieren")} }
                    }
                    Dropdown {
                        align: Align::End,
                        trigger_class: "{btn(BTN_OUTLINE)} {BTN_SM} px-2",
                        trigger: rsx! { icons::Ellipsis {} },
                        MenuItem {
                            onclick: move |_| {
                                new_name.set(current_name.clone());
                                rename_open.set(true);
                            },
                            icons::Pencil {}
                            {tr("Umbenennen")}
                        }
                        MenuItem { onclick: move |_| *NEW_THREAD_PROJECT.write() = Some(id),
                            icons::SquarePen {}
                            {tr("Als Projekt für neue Threads wählen")}
                        }
                        MenuSeparator {}
                        MenuItem { destructive: true, onclick: remove,
                            icons::Trash2 {}
                            {tr("Projekt löschen")}
                        }
                    }
                }
            }
            if p.status == "error" {
                div { class: "mb-6",
                    Callout { text: trf("Klonen fehlgeschlagen: {0}", &[&p.status_message]), error: true }
                }
            }
            div { class: "grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]",
                div { class: "min-w-0 space-y-6",
                    section {
                        h2 { class: "mb-2 text-sm font-medium text-muted-foreground", {tr("Threads")} }
                        if threads.is_empty() {
                            p { class: "rounded-xl border border-dashed px-4 py-6 text-center text-sm text-muted-foreground", {tr("Noch keine Threads in diesem Projekt.")} }
                        } else {
                            div { class: "divide-y overflow-hidden rounded-xl border bg-card",
                                for t in threads {
                                    Link { key: "{t.id}", to: Route::ThreadPage { id: t.id }, class: "flex items-center gap-3 px-4 py-3 hover:bg-accent/30",
                                        crate::pages::agents::AgentIcon { kind: t.agent, size: 18 }
                                        div { class: "min-w-0 flex-1",
                                            p { class: "truncate text-sm font-medium", {thread_title(&t)} }
                                            p { class: "truncate text-xs text-muted-foreground", "{t.branch}" }
                                        }
                                        ThreadState { thread: t.clone() }
                                    }
                                }
                            }
                        }
                    }
                }
                aside { class: "space-y-6",
                    section { class: "rounded-xl border bg-card p-4 text-sm shadow-sm",
                        p { class: "mb-3 font-medium", {tr("Arbeitsordner")} }
                        dl { class: "grid gap-2 text-xs",
                            div {
                                dt { class: "text-muted-foreground", {tr("Ordner auf dem Server")} }
                                dd { class: "mt-0.5 flex items-center gap-1 font-mono break-all",
                                    "{d.path}"
                                    CopyButton { text: d.path.clone(), class: "shrink-0 text-muted-foreground hover:text-foreground [&_svg]:size-3.5" }
                                }
                            }
                            if !d.branch.is_empty() {
                                div {
                                    dt { class: "text-muted-foreground", {tr("Branch des Hauptcheckouts")} }
                                    dd { class: "mt-0.5 font-mono", "{d.branch}" }
                                }
                            }
                            if !d.clone_url.is_empty() {
                                div {
                                    dt { class: "text-muted-foreground", {tr("Klonen von")} }
                                    dd { class: "mt-0.5 font-mono break-all", "{d.clone_url}" }
                                }
                            }
                            div {
                                dt { class: "text-muted-foreground", {tr("Angelegt")} }
                                dd { class: "mt-0.5", {util::datetime(d.created_at)} }
                            }
                        }
                    }
                    if !d.commits.is_empty() {
                        section { class: "rounded-xl border bg-card p-4 text-sm shadow-sm",
                            p { class: "mb-3 font-medium", {tr("Letzte Commits")} }
                            div { class: "grid gap-2.5",
                                for c in d.commits.iter().cloned() {
                                    div { key: "{c.sha}", class: "min-w-0",
                                        p { class: "truncate", "{c.summary}" }
                                        p { class: "truncate text-xs text-muted-foreground",
                                            span { class: "font-mono", "{c.short_sha}" }
                                            " · {c.author.name} · "
                                            RelTime { ts: c.author.when }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Modal { open: rename_open, title: tr("Projekt umbenennen"), description: tr("Nur der Name in nrzX – das Repository in nrzGit behält seinen Namen."),
            form { class: "grid gap-4", onsubmit: save_name,
                Field { label: tr("Name"),
                    input { class: INPUT, autofocus: true, value: "{new_name}", oninput: move |e| new_name.set(e.value()) }
                }
                div { class: "flex justify-end gap-2",
                    button { r#type: "button", class: btn(BTN_OUTLINE), onclick: move |_| rename_open.set(false), {tr("Abbrechen")} }
                    button { r#type: "submit", class: btn(BTN_PRIMARY), {tr("Speichern")} }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Projekt hinzufügen

#[derive(Clone, Copy, PartialEq)]
enum AddTab {
    Import,
    Create,
    Local,
}

/// Dialog „Projekt hinzufügen“ (einmal im Rahmen eingebunden, geöffnet über `ADD_PROJECT`).
#[component]
pub fn AddProjectDialog() -> Element {
    let mut open = use_signal(|| false);
    // Globales Signal → lokales Signal des Dialogs (und zurück beim Schliessen)
    use_effect(move || {
        if ADD_PROJECT() && !open() {
            open.set(true);
        }
    });
    use_effect(move || {
        if !open() && *ADD_PROJECT.peek() {
            *ADD_PROJECT.write() = false;
        }
    });
    rsx! {
        Modal { open, title: tr("Projekt hinzufügen"), description: tr("Die Agenten arbeiten in einer Kopie auf diesem Server – jeder Thread auf Wunsch in einem eigenen Worktree."), class: "max-w-2xl",
            AddProjectBody { open }
        }
    }
}

#[component]
fn AddProjectBody(open: Signal<bool>) -> Element {
    let connected = ME.read().as_ref().is_some_and(|m| m.nrzgit_connected);
    let mut tab = use_signal(|| if connected { AddTab::Import } else { AddTab::Local });
    let seg = |active: bool| {
        if active {
            "inline-flex flex-1 items-center justify-center gap-1.5 rounded-md bg-background px-3 py-1.5 text-sm font-medium shadow-xs [&_svg]:size-4"
        } else {
            "inline-flex flex-1 items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground [&_svg]:size-4"
        }
    };
    rsx! {
        div { class: "mb-5 flex rounded-lg bg-muted p-1",
            button { class: seg(tab() == AddTab::Import), onclick: move |_| tab.set(AddTab::Import), icons::Download {} {tr("Aus nrzGit")} }
            button { class: seg(tab() == AddTab::Create), onclick: move |_| tab.set(AddTab::Create), icons::Plus {} {tr("Neues Repository")} }
            button { class: seg(tab() == AddTab::Local), onclick: move |_| tab.set(AddTab::Local), icons::HardDrive {} {tr("Lokal")} }
        }
        match tab() {
            AddTab::Import if connected => rsx! { ImportTab { open, forge: "nrzgit".to_string() } },
            AddTab::Create if connected => rsx! { CreateTab { open, forge: "nrzgit".to_string() } },
            AddTab::Local => rsx! { LocalTab { open } },
            _ => rsx! { NeedsNrzGit { open } },
        }
    }
}

#[component]
fn NeedsNrzGit(open: Signal<bool>) -> Element {
    let nav = use_navigator();
    rsx! {
        div { class: "grid gap-3 rounded-lg border border-dashed p-5 text-center text-sm",
            p { class: "font-medium", {tr("Noch nicht mit nrzGit verbunden")} }
            p { class: "text-muted-foreground", {tr("Lege in nrzGit ein Token an und füge es in den Einstellungen ein – dann erscheinen hier deine Repositories.")} }
            div {
                button {
                    class: btn(BTN_PRIMARY),
                    onclick: move |_| {
                        open.set(false);
                        nav.push(Route::Settings { section: "nrzgit".into() });
                    },
                    icons::Link {}
                    {tr("Mit nrzGit verbinden")}
                }
            }
        }
    }
}

/// Nach dem Anlegen bzw. Öffnen: Dialog schliessen und gleich wie bei „Neuer Thread“ weiter –
/// die Startseite mit dem Projekt gewählt.
fn created(p: ProjectSummary, open: Signal<bool>, nav: dioxus::router::Navigator) {
    let id = p.id;
    upsert(p);
    start_in(id, open, nav);
}

fn start_in(id: i64, mut open: Signal<bool>, nav: dioxus::router::Navigator) {
    *NEW_THREAD_PROJECT.write() = Some(id);
    open.set(false);
    nav.push(Route::Home {});
}

#[component]
fn ImportTab(open: Signal<bool>, forge: String) -> Element {
    let nav = use_navigator();
    let mut q = use_signal(String::new);
    let mut busy = use_signal(|| None::<String>);
    let path = format!("/nrzgit/repos?kind={forge}");
    let repos = use_resource(move || {
        let path = path.clone();
        async move { api::get::<Vec<NrzGitRepo>>(&path).await }
    });
    let list = match repos.read().clone() {
        None => return rsx! { Loading {} },
        Some(Err(e)) => return rsx! { Callout { text: e.message, error: true } },
        Some(Ok(l)) => l,
    };
    let needle = q().trim().to_lowercase();
    let shown: Vec<NrzGitRepo> = list
        .into_iter()
        .filter(|r| needle.is_empty() || r.full_name.to_lowercase().contains(&needle) || r.description.to_lowercase().contains(&needle))
        .take(100)
        .collect();
    rsx! {
        div { class: "relative mb-3",
            icons::Search { class: "pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" }
            input { class: "{INPUT} pl-9", r#type: "search", autofocus: true, placeholder: tr("Repositories durchsuchen"), value: "{q}", oninput: move |e| q.set(e.value()) }
        }
        div { class: "max-h-[50vh] divide-y overflow-y-auto rounded-lg border",
            if shown.is_empty() {
                p { class: "px-4 py-8 text-center text-sm text-muted-foreground", {tr("Keine Repositories gefunden.")} }
            }
            for r in shown {
                div {
                    key: "{r.id}-{r.full_name}",
                    // Schon ein Projekt: die ganze Zeile wählt es für den neuen Thread
                    class: if r.project_id.is_some() { "flex cursor-pointer items-center gap-3 px-3 py-2.5 hover:bg-accent/50" } else { "flex items-center gap-3 px-3 py-2.5" },
                    onclick: move |_| {
                        if let Some(pid) = r.project_id {
                            start_in(pid, open, nav);
                        }
                    },
                    if r.owner_is_org {
                        icons::Building2 { class: "size-4 shrink-0 text-muted-foreground" }
                    } else {
                        icons::BookMarked { class: "size-4 shrink-0 text-muted-foreground" }
                    }
                    div { class: "min-w-0 flex-1",
                        p { class: "truncate text-sm",
                            span { class: "text-muted-foreground", "{r.owner}/" }
                            span { class: "font-medium", "{r.name}" }
                            if r.private {
                                span { class: "ml-2 rounded-full border px-1.5 text-[10px] text-muted-foreground", {tr("Privat")} }
                            }
                        }
                        if !r.description.is_empty() {
                            p { class: "truncate text-xs text-muted-foreground", "{r.description}" }
                        }
                    }
                    if let Some(pid) = r.project_id {
                        span { class: "text-xs text-muted-foreground", {tr("Schon ein Projekt")} }
                        button {
                            class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                            onclick: move |e| {
                                e.stop_propagation();
                                start_in(pid, open, nav);
                            },
                            icons::Check {}
                            {tr("Auswählen")}
                        }
                    } else {
                        button {
                            class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                            disabled: busy().is_some(),
                            onclick: {
                                let full = r.full_name.clone();
                                let forge = forge.clone();
                                move |_| {
                                    let full = full.clone();
                                    let forge = forge.clone();
                                    busy.set(Some(full.clone()));
                                    spawn(async move {
                                        let res = api::post::<ProjectSummary, _>("/projects/import", &ImportProject { repo: full, forge }).await;
                                        busy.set(None);
                                        match res {
                                            Ok(p) => created(p, open, nav),
                                            Err(e) => toast_error(e),
                                        }
                                    });
                                }
                            },
                            BusyIcon { busy: busy().as_deref() == Some(r.full_name.as_str()) }
                            {tr("Übernehmen")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CreateTab(open: Signal<bool>, forge: String) -> Element {
    let nav = use_navigator();
    let path = format!("/nrzgit/owners?kind={forge}");
    let owners = use_resource(move || {
        let path = path.clone();
        async move { api::get::<Vec<NrzGitOwner>>(&path).await }
    });
    let mut owner = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut private = use_signal(|| true);
    let mut readme = use_signal(|| true);
    let mut busy = use_signal(|| false);
    let list: Vec<NrzGitOwner> = match owners.read().clone() {
        Some(Ok(l)) => l,
        Some(Err(e)) => return rsx! { Callout { text: e.message, error: true } },
        None => return rsx! { Loading {} },
    };
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        let forge = forge.clone();
        spawn(async move {
            let body = CreateRepoProject { owner: owner(), name: name().trim().to_string(), description: description(), private: private(), readme: readme(), forge };
            let res = api::post::<ProjectSummary, _>("/projects/create", &body).await;
            busy.set(false);
            match res {
                Ok(p) => created(p, open, nav),
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        form { class: "grid gap-4", onsubmit: submit,
            div { class: "grid gap-4 sm:grid-cols-[12rem_minmax(0,1fr)]",
                Field { label: tr("Besitzer"),
                    select { class: SELECT, onchange: move |e| owner.set(e.value()),
                        for o in list.iter().cloned() {
                            option { key: "{o.username}", value: if o.is_org { o.username.clone() } else { String::new() }, selected: owner() == o.username || (owner().is_empty() && !o.is_org),
                                if o.is_org { "{o.username}" } else { {trf("{0} (ich)", &[&o.username])} }
                            }
                        }
                    }
                }
                Field { label: tr("Name des Repositorys"),
                    input { class: INPUT, autofocus: true, required: true, placeholder: "mein-projekt", value: "{name}", oninput: move |e| name.set(e.value()) }
                }
            }
            Field { label: tr("Beschreibung"),
                input { class: INPUT, value: "{description}", oninput: move |e| description.set(e.value()) }
            }
            Checkbox { checked: private(), onchange: move |v| private.set(v), label: tr("Privat"), hint: tr("Nur du (und deine Organisation) sehen das Repository.") }
            Checkbox { checked: readme(), onchange: move |v| readme.set(v), label: tr("Mit README anlegen"), hint: tr("Ein erster Commit – sonst ist das Repository leer.") }
            div { class: "flex justify-end",
                button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy() || name().trim().is_empty(),
                    BusyIcon { busy: busy() }
                    {tr("Anlegen und übernehmen")}
                }
            }
        }
    }
}

#[component]
fn LocalTab(open: Signal<bool>) -> Element {
    let nav = use_navigator();
    let mut name = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let submit = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let res = api::post::<ProjectSummary, _>("/projects/local", &CreateLocalProject { name: name().trim().to_string() }).await;
            busy.set(false);
            match res {
                Ok(p) => created(p, open, nav),
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        form { class: "grid gap-4", onsubmit: submit,
            p { class: "text-sm text-muted-foreground", {tr("Ein Git-Repository nur auf diesem Server, ohne nrzGit – zum Ausprobieren. Pushen und Pull Requests gehen erst mit nrzGit.")} }
            Field { label: tr("Name"),
                input { class: INPUT, autofocus: true, required: true, value: "{name}", oninput: move |e| name.set(e.value()) }
            }
            div { class: "flex justify-end",
                button { r#type: "submit", class: btn(BTN_PRIMARY), disabled: busy() || name().trim().is_empty(),
                    BusyIcon { busy: busy() }
                    {tr("Lokales Projekt anlegen")}
                }
            }
        }
    }
}
