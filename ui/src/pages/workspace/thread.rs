//! Thread-Seite: in der Mitte die Unterhaltung (`agents::ChatView`), rechts die Spalte mit
//! Karten wie im Bild – Umgebung (Host, „Open in editor“, „Setup Worktree“), Git (Branch,
//! Commit, Änderungen), Vorschau (lauschende Ports, Seite im Paneel) und die Subagenten
//! (`agents::Lineage`) –, unten auf Wunsch das Terminal. Auf dem Handy kommt die rechte Spalte
//! als Sheet. Statt des Chats auf Wunsch das CLI des Agenten selbst (`cli::NativeCli`).

use super::editor::EditorDialog;
use super::git::{BranchMenu, ChangesDialog, CommitDialog, NewBranchDialog, PullRequestDialog};
use super::parts::*;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::i18n::{tr, trf};
use crate::state::{PREVIEW, THREADS};
use crate::util;
use dioxus::prelude::*;
use nrzx_shared::workspace::{GitResult, PreviewPort, PreviewToken, PreviewTokenRequest, WorkspaceStatus};

/// Seite eines Threads: zeichnet nichts selbst – `AppLayout` hält die zuletzt geöffneten Threads
/// eingehängt (`ThreadView`) und zeigt den gewählten.
#[component]
pub fn ThreadPage(id: i64) -> Element {
    let _ = id;
    rsx! {}
}

/// Ein Thread mit seinen Ansichten; bleibt eingehängt, auch wenn ein anderer gezeigt wird.
#[component]
pub fn ThreadView(id: i64) -> Element {
    // Status des Arbeitsordners: neu laden, wenn der Thread fertig wird oder Git sich ändert
    let marker = use_memo(move || THREADS.read().get(&id).map(|t| (t.status, t.branch.clone())));
    let status = use_resource(move || async move {
        let _ = (marker(), GIT_REFRESH());
        api::get::<WorkspaceStatus>(&format!("/workspace/{id}/status")).await
    });
    // Ein Agent öffnet eine Vorschau (`HubEvent::PreviewOpened`): im Reiter „Browser“ zeigen
    use_effect(move || {
        if let Some((tid, port, path)) = PREVIEW() {
            if tid == id {
                super::browser::open(id, super::browser::Target::Port(port, path));
                super::views::activate(id, super::views::View::Browser);
                *PREVIEW.write() = None;
            }
        }
    });
    // /btw im Composer: Seiten-Chat in der rechten Spalte öffnen
    let mut btw_seen = use_signal(|| *crate::pages::agents::BTW_OPEN.peek());
    use_effect(move || {
        let n = crate::pages::agents::BTW_OPEN();
        // Nur der gezeigte Thread öffnet die Spalte (die übrigen sind nur eingehängt)
        let here = *crate::state::SHOWN_THREAD.peek() == Some(id);
        if n != *btw_seen.peek() {
            btw_seen.set(n);
            if !here {
                return;
            }
            super::sidepanel::set_side_tab("btw");
            if narrow() {
                *MOBILE_RIGHT.write() = true;
            } else if !*RIGHT_FEATURE.peek() {
                *RIGHT_TEMP.write() = true;
            } else if !*RIGHT.peek() {
                toggle(&RIGHT, "nrz-right");
            }
        }
    });
    let s = status.read().clone();
    let cli_wanted = super::cli::wanted(id);
    let view = super::views::active(id);
    let browser = super::browser::open_for(id);
    let new_chat = super::views::list(id).contains(&super::views::View::NewChat);
    let terminals = super::views::TERMINALS.read().get(&id).cloned().unwrap_or_default();
    let side = right_open();
    let workdir = s.as_ref().and_then(|r| r.as_ref().ok()).map(|w| w.path.clone()).unwrap_or_default();
    // Offene Ansichten liegen übereinander und bleiben geladen; sichtbar ist die gewählte
    let layer = |on: bool| if on { "absolute inset-0 flex flex-col" } else { "invisible absolute inset-0 flex flex-col" };
    rsx! {
        div { class: "flex h-full min-h-0 w-full gap-2",
            // Paneel des Threads: oben die Reiter (Chats, neuer Chat, Browser, Terminals) – der
            // gewählte hängt an der Oberkante –, darunter die Kopfleiste und die gewählte Ansicht
            section { class: format!("nrx-panel flex min-w-0 flex-1 flex-col overflow-hidden {}", super::tabs::panel_attach(id)),
                // Eingeklappt ohne Reiter steht die Kopfleiste oben im Reiter neben den Knöpfen
                if LEFT() || super::tabs::has_tabs(id) {
                    super::layout::CenterBar { thread: Some(id), edge: !side, top: LEFT() && !super::tabs::has_tabs(id) }
                }
                div { class: "flex min-h-0 flex-1 flex-col",
                    div { class: "relative min-h-0 min-w-0 flex-1",
                        div { id: "chat-layer-{id}", class: layer(view == super::views::View::Chat),
                            crate::pages::agents::ChatView { thread_id: id }
                        }
                        if new_chat {
                            div { class: layer(view == super::views::View::NewChat),
                                super::views::NewChatPane { thread_id: id }
                            }
                        }
                        if cli_wanted {
                            div { class: layer(view == super::views::View::Cli),
                                super::cli::NativeCli { thread_id: id, visible: view == super::views::View::Cli }
                            }
                        }
                        if browser {
                            div { class: layer(view == super::views::View::Browser),
                                super::browser::BrowserPanel { thread_id: id, workdir }
                            }
                        }
                        for t in terminals {
                            div { key: "{t}", class: "{layer(view == super::views::View::Term(t.clone()))} px-3 pb-3",
                                crate::pages::agents::TerminalView { id: t.clone() }
                            }
                        }
                    }
                }
            }
            if side {
                aside { class: "nrx-panel hidden w-[21rem] shrink-0 flex-col overflow-hidden lg:flex",
                    super::sidepanel::SidePanel {
                        thread_id: id,
                        status: s.clone(),
                        overview: rsx! { RightColumn { thread_id: id, status: s.clone() } },
                        onclose: move |_| close_right(),
                    }
                }
            }
            if MOBILE_RIGHT() {
                div { class: "fixed inset-0 z-50 bg-black/60 lg:hidden", onclick: move |_| *MOBILE_RIGHT.write() = false }
                aside { class: "nrz-pane fixed inset-y-0 right-0 z-50 flex w-[90vw] max-w-sm flex-col border-l bg-background shadow-xl animate-in lg:hidden",
                    super::sidepanel::SidePanel {
                        thread_id: id,
                        status: s.clone(),
                        overview: rsx! { RightColumn { thread_id: id, status: s } },
                        onclose: move |_| *MOBILE_RIGHT.write() = false,
                    }
                }
            }
        }
    }
}

/// Token für eine Vorschau holen und die Adresse zurückgeben.
async fn preview_url(thread_id: i64, port: u16, path: &str) -> Option<String> {
    let body = PreviewTokenRequest { thread_id, port, path: path.to_string() };
    match api::post::<PreviewToken, _>("/previews/token", &body).await {
        Ok(t) => Some(t.url),
        Err(e) => {
            toast_error(e);
            None
        }
    }
}

// ------------------------------------------------------------------ Rechte Spalte

const ROW: &str = "flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent/60 [&_svg]:size-4 [&_svg]:shrink-0";
const ROW_STATIC: &str = "flex w-full min-w-0 items-center gap-2.5 px-2 py-1.5 text-sm [&_svg]:size-4 [&_svg]:shrink-0";

/// Überschrift eines Abschnitts im Panel der rechten Spalte (leise, wie „Environment“).
#[component]
pub fn SectionLabel(#[props(into)] text: String, children: Element) -> Element {
    rsx! {
        div { class: "flex items-center gap-2 px-2 pt-2.5 pb-1 text-xs text-muted-foreground/80",
            span { class: "flex-1", "{text}" }
            {children}
        }
    }
}

/// Trennlinie zwischen den Abschnitten des Panels.
#[component]
pub fn SectionDivider() -> Element {
    rsx! { div { class: "mx-2 my-1.5 border-t" } }
}

#[component]
fn RightColumn(thread_id: i64, status: Option<Result<WorkspaceStatus, api::ApiError>>) -> Element {
    rsx! {
        div { class: "grid grid-cols-[minmax(0,1fr)] gap-3",
            match status {
                None => rsx! {
                    div { class: "rounded-xl border bg-card p-4 shadow-sm", Loading {} }
                },
                Some(Err(e)) => rsx! {
                    div { class: "rounded-xl border bg-card p-3 shadow-sm", Callout { text: e.message, error: true } }
                },
                Some(Ok(s)) => rsx! {
                    // Ein Panel mit Abschnitten (Umgebung, Lokale Server, Nutzung, Repository, …)
                    div { class: "rounded-2xl border bg-card/70 p-1.5 shadow-sm [.nrz-glass_&]:bg-card/50",
                        WorkspaceCard { status: s.clone() }
                        SectionDivider {}
                        PreviewCard { thread_id }
                        SectionDivider {}
                        crate::pages::agents::UsageSection {}
                    }
                },
            }
            crate::pages::agents::Lineage { thread_id }
        }
    }
}

/// Karte „Umgebung“ und „Git“ (wie im Bild in einer Karte, durch Linien getrennt).
#[component]
fn WorkspaceCard(status: WorkspaceStatus) -> Element {
    let s = status.clone();
    let id = s.thread_id;
    let mut editor_open = use_signal(|| false);
    let mut changes_open = use_signal(|| false);
    let mut commit_open = use_signal(|| false);
    let mut commit_push = use_signal(|| false);
    let mut pr_open = use_signal(|| false);
    let mut branch_open = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let live = crate::state::CHANGES.read().get(&id).copied();
    let (adds, dels) = live.map(|(a, d, _)| (a, d)).unwrap_or((s.additions, s.deletions));
    let has_project = s.project_id.is_some();
    let ready = s.project_status == "ready";
    let path = s.path.clone();
    let path_copy = s.path.clone();
    let repo_url = s.repo_url.clone();
    let can_pr = has_project && !s.repo.is_empty() && s.has_remote;

    let mut git_action = move |what: &'static str| {
        busy.set(true);
        spawn(async move {
            if let Err(e) = api::action::<GitResult>(&format!("/workspace/{id}/{what}")).await {
                toast_error(e);
            }
            busy.set(false);
            git_refresh();
        });
    };
    let worktree = move |create: bool| {
        spawn(async move {
            if !create {
                let ask = Ask::new(
                    tr("Worktree entfernen?"),
                    tr("Der Ordner des Worktrees wird gelöscht – nicht committete Änderungen darin gehen verloren. Der Branch bleibt erhalten; der Thread arbeitet danach im Hauptcheckout."),
                )
                .confirm(tr("Entfernen"))
                .destructive();
                if !confirm(ask).await {
                    return;
                }
            }
            busy.set(true);
            let res = if create {
                api::action::<WorkspaceStatus>(&format!("/workspace/{id}/worktree")).await
            } else {
                api::send::<WorkspaceStatus, _>("DELETE", &format!("/workspace/{id}/worktree"), &serde_json::json!({})).await
            };
            busy.set(false);
            if let Err(e) = res {
                toast_error(e);
            }
            git_refresh();
        });
    };

    let pr = s.pull_request.clone();
    rsx! {
        // Umgebung: Änderungen, Worktree, Branch, Commit & Push
        SectionLabel { text: tr("Umgebung"),
            if s.busy {
                icons::LoaderCircle { class: "size-3.5 animate-spin text-info" }
            }
        }
        if !ready {
            div { class: "{ROW_STATIC} text-muted-foreground",
                icons::LoaderCircle { class: "animate-spin" }
                {tr("Projekt wird geklont …")}
            }
        } else if s.is_git {
            button { class: ROW, onclick: move |_| changes_open.set(true),
                icons::FileDiff { class: "text-muted-foreground" }
                span { class: "flex-1", {tr("Changes")} }
                span { class: "font-mono text-xs tabular-nums",
                    span { class: if adds > 0 { "text-success" } else { "text-muted-foreground" }, "+{adds}" }
                    " "
                    span { class: if dels > 0 { "text-destructive" } else { "text-muted-foreground" }, "−{dels}" }
                }
            }
        }
        Dropdown {
            class: "w-72",
            trigger_class: ROW,
            trigger: rsx! {
                icons::GitFork { class: "text-muted-foreground" }
                span { class: "flex-1",
                    if s.worktree { {tr("Worktree")} } else { {tr("Setup Worktree")} }
                }
                if busy() {
                    icons::LoaderCircle { class: "animate-spin text-muted-foreground" }
                }
                icons::ChevronDown { class: "text-muted-foreground" }
            },
            if !has_project {
                p { class: "px-2 py-2 text-xs text-muted-foreground", {tr("Ohne Projekt arbeitet der Agent in einem eigenen Scratch-Ordner.")} }
            } else if s.worktree {
                p { class: "px-2 py-2 text-xs text-muted-foreground", {trf("Eigener Worktree auf Branch „{0}“ – andere Threads stören nicht.", &[&s.branch])} }
                MenuSeparator {}
                MenuItem { destructive: true, onclick: move |_| worktree(false),
                    icons::Trash2 {}
                    {tr("Worktree entfernen")}
                }
            } else {
                p { class: "px-2 py-2 text-xs text-muted-foreground", {tr("Der Thread arbeitet im Hauptcheckout des Projekts. Ein eigener Worktree bekommt einen eigenen Branch vom Standard-Branch.")} }
                MenuSeparator {}
                MenuItem { onclick: move |_| worktree(true),
                    icons::GitFork {}
                    {tr("Worktree einrichten")}
                }
            }
        }
        if ready && !s.is_git {
            div { class: "{ROW_STATIC} text-muted-foreground",
                icons::GitBranch {}
                {tr("Kein Git-Repository")}
            }
        } else if ready {
            Dropdown {
                class: "max-h-80 w-72 overflow-y-auto",
                trigger_class: ROW,
                trigger: rsx! {
                    icons::GitBranch { class: "text-muted-foreground" }
                    span { class: "min-w-0 flex-1 truncate", title: "{s.branch}",
                        if s.branch.is_empty() { {tr("(kein Branch)")} } else { "{s.branch}" }
                    }
                    if s.ahead > 0 {
                        span { class: "shrink-0 text-xs tabular-nums text-muted-foreground", title: tr("Noch nicht gepusht"), "↑{s.ahead}" }
                    }
                    if s.behind > 0 {
                        span { class: "shrink-0 text-xs tabular-nums text-muted-foreground", title: tr("Neu auf dem Server"), "↓{s.behind}" }
                    }
                    icons::ChevronDown { class: "text-muted-foreground" }
                },
                BranchMenu { thread_id: id, onnew: move |_| branch_open.set(true) }
            }
            Dropdown {
                class: "w-64",
                trigger_class: ROW,
                trigger: rsx! {
                    icons::CloudUpload { class: "text-muted-foreground" }
                    span { class: "flex-1", if s.has_remote { {tr("Commit & Push")} } else { {tr("Commit")} } }
                    icons::ChevronDown { class: "text-muted-foreground" }
                },
                MenuItem {
                    onclick: move |_| {
                        commit_push.set(false);
                        commit_open.set(true);
                    },
                    icons::GitCommitHorizontal {}
                    {tr("Commit …")}
                }
                if s.has_remote {
                    MenuItem {
                        onclick: move |_| {
                            commit_push.set(true);
                            commit_open.set(true);
                        },
                        icons::ArrowUpFromLine {}
                        {tr("Commit & Push …")}
                    }
                    MenuSeparator {}
                    MenuItem { onclick: move |_| git_action("push"),
                        icons::Upload {}
                        span { class: "flex-1", {tr("Push")} }
                        if s.ahead > 0 {
                            span { class: "text-xs text-muted-foreground", "↑{s.ahead}" }
                        }
                    }
                    MenuItem { onclick: move |_| git_action("pull"),
                        icons::Download {}
                        span { class: "flex-1", {tr("Pull")} }
                        if s.behind > 0 {
                            span { class: "text-xs text-muted-foreground", "↓{s.behind}" }
                        }
                    }
                }
                if can_pr {
                    MenuSeparator {}
                    MenuItem { onclick: move |_| pr_open.set(true),
                        icons::GitPullRequest {}
                        {tr("Pull Request in nrzGit …")}
                    }
                }
            }
        }
        // Repository und Pull Request
        if !s.repo.is_empty() {
            SectionDivider {}
            SectionLabel { text: tr("Repository") }
            button {
                class: ROW,
                disabled: repo_url.is_empty(),
                onclick: {
                    let u = repo_url.clone();
                    move |_| open_tab(&u)
                },
                icons::BookMarked { class: "text-muted-foreground" }
                span { class: "min-w-0 flex-1 truncate", "{s.repo}" }
                if !repo_url.is_empty() {
                    icons::SquareArrowOutUpRight { class: "text-muted-foreground" }
                }
            }
        }
        if let Some(pr) = pr {
            SectionDivider {}
            SectionLabel { text: tr("Pull Request") }
            button {
                class: ROW,
                title: "{pr.title}",
                onclick: {
                    let u = pr.url.clone();
                    move |_| open_tab(&u)
                },
                icons::GitPullRequest { class: "text-success" }
                span { class: "min-w-0 flex-1 truncate", "#{pr.number} {pr.title}" }
                icons::SquareArrowOutUpRight { class: "text-muted-foreground" }
            }
        } else if can_pr {
            SectionDivider {}
            SectionLabel { text: tr("Pull Request") }
            button { class: ROW, onclick: move |_| pr_open.set(true),
                icons::GitPullRequestCreate { class: "text-muted-foreground" }
                span { class: "flex-1 text-muted-foreground", {tr("Pull Request erstellen …")} }
            }
        }
        // Editor: Dateien, Pfad, Rechner
        SectionDivider {}
        SectionLabel { text: tr("Editor") }
        button { class: ROW, disabled: !ready, onclick: move |_| editor_open.set(true),
            icons::SquareCode { class: "text-muted-foreground" }
            span { class: "flex-1", {tr("Dateien ansehen und bearbeiten")} }
        }
        Dropdown {
            class: "w-72",
            trigger_class: ROW,
            trigger: rsx! {
                icons::Monitor { class: "text-muted-foreground" }
                span { class: "min-w-0 flex-1 truncate", title: if path.is_empty() { s.host.clone() } else { path.clone() }, "{s.host}" }
                icons::ChevronDown { class: "text-muted-foreground" }
            },
            MenuItem {
                onclick: move |_| {
                    util::copy(&path_copy);
                },
                icons::Copy {}
                {tr("Pfad kopieren")}
            }
            if !s.path.is_empty() {
                MenuSeparator {}
                p { class: "break-all px-2 py-1.5 font-mono text-[11px] text-muted-foreground", "{s.path}" }
            }
        }
        EditorDialog { open: editor_open, thread_id: id }
        ChangesDialog { open: changes_open, thread_id: id, oncommit: move |_| { changes_open.set(false); commit_push.set(false); commit_open.set(true); } }
        CommitDialog { open: commit_open, status: s.clone(), push: commit_push() }
        PullRequestDialog { open: pr_open, status: s.clone() }
        NewBranchDialog { open: branch_open, thread_id: id }
    }
}

// ------------------------------------------------------------------ Vorschau

#[component]
fn PreviewCard(thread_id: i64) -> Element {
    let mut version = use_signal(|| 0u64);
    let ports = use_resource(move || async move {
        let _ = version();
        api::get::<Vec<PreviewPort>>(&format!("/previews?threadId={thread_id}")).await
    });
    // Alle 15 Sekunden nachsehen (Agenten starten Server oft nebenbei)
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(15_000).await;
            version += 1;
        }
    });
    let mut path = use_signal(|| "/".to_string());
    // Im Reiter „Browser“ des Threads öffnen und zeigen
    let open_here = move |port: u16| {
        super::browser::open(thread_id, super::browser::Target::Port(port, path()));
        super::views::activate(thread_id, super::views::View::Browser);
    };
    let open_tab_for = move |port: u16| {
        spawn(async move {
            if let Some(url) = preview_url(thread_id, port, &path()).await {
                super::browser::open_external(&url);
            }
        });
    };
    let list = ports.read().clone();
    let current = super::browser::BROWSER.read().get(&thread_id).and_then(|b| match b.target {
        super::browser::Target::Port(p, _) => Some(p),
        _ => None,
    });
    let count = list.as_ref().and_then(|l| l.as_ref().ok()).map(|l| l.len()).unwrap_or(0);
    let mut open = use_signal(|| util::storage_get("nrz-servers-open").as_deref() == Some("1"));
    rsx! {
        div {
            SectionLabel { text: tr("Aktionen"),
                button { class: "grid size-5 place-items-center rounded text-muted-foreground hover:bg-accent hover:text-foreground", title: tr("Neu suchen"), onclick: move |_| version += 1,
                    icons::RefreshCw { class: "size-3" }
                }
            }
            button {
                class: ROW,
                onclick: move |_| {
                    let v = !open();
                    open.set(v);
                    util::storage_set("nrz-servers-open", if v { "1" } else { "0" });
                },
                icons::Globe { class: "text-muted-foreground" }
                span { class: "flex-1", {tr("Lokale Server")} }
                span { class: if count > 0 { "text-xs tabular-nums text-success" } else { "text-xs tabular-nums text-muted-foreground" }, "{count}" }
                if open() {
                    icons::ChevronUp { class: "text-muted-foreground" }
                } else {
                    icons::ChevronDown { class: "text-muted-foreground" }
                }
            }
            if open() {
            div { class: "px-2 pb-1.5",
            match list {
                None => rsx! { p { class: "text-xs text-muted-foreground", {tr("Suche Server …")} } },
                Some(Err(e)) => rsx! { p { class: "text-xs text-destructive", "{e.message}" } },
                Some(Ok(l)) if l.is_empty() => rsx! {
                    p { class: "text-xs text-muted-foreground", {tr("Kein Server läuft. Startet ein Agent z. B. „npm run dev“, erscheint der Port hier.")} }
                },
                Some(Ok(l)) => rsx! {
                    div { class: "grid gap-1",
                        for p in l {
                            div { key: "{p.port}", class: if current == Some(p.port) { "flex items-center gap-2 rounded-md bg-accent/60 px-2 py-1.5" } else { "flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-accent/40" },
                                span { class: if p.this_thread { "size-2 shrink-0 rounded-full bg-success" } else { "size-2 shrink-0 rounded-full bg-muted-foreground/50" } }
                                span { class: "font-mono text-sm tabular-nums", ":{p.port}" }
                                span { class: "min-w-0 flex-1 truncate text-xs text-muted-foreground", "{p.process}" }
                                button { class: "rounded px-1.5 py-0.5 text-xs hover:bg-accent", onclick: move |_| open_here(p.port), {tr("Öffnen")} }
                                button { class: "grid size-6 place-items-center rounded text-muted-foreground hover:bg-accent hover:text-foreground", title: tr("In neuem Tab öffnen"), onclick: move |_| open_tab_for(p.port),
                                    icons::ExternalLink { class: "size-3.5" }
                                }
                            }
                        }
                    }
                    div { class: "mt-2 flex items-center gap-2",
                        span { class: "text-xs text-muted-foreground", {tr("Pfad")} }
                        input { class: "h-7 min-w-0 flex-1 rounded-md border bg-transparent px-2 font-mono text-xs outline-none focus:border-ring", value: "{path}", oninput: move |e| path.set(e.value()) }
                    }
                },
            }
            }
            }
        }
    }
}

