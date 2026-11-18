//! Composer: grosses Eingabefeld mit `@` (Dateien), `@@` (andere Repositories als Referenz),
//! `$` (Skills) und `/` (Befehle), Anhängen
//! (Büroklammer, Einfügen, Ziehen), der Auswahl Agent · Konto · Modell · Stufe und dem runden
//! Senden-Knopf. Ohne Thread beginnt er einen neuen; FreeStyle und eigener Worktree stehen dann in
//! der Auswahl (Chat-Einstellungen), damit unten nur Anhängen, Auswahl und Senden bleiben.

use super::data::{self, effort_label, use_agents, Selection, AGENTS, COMPOSER_DRAFT, SKILLS};
use super::icons::AgentIcon;
use crate::api;
use crate::mic;
use crate::components::common::*;
use crate::components::overlay::{toast_error, Modal};
use crate::i18n::{tr, trf};
use crate::state::{self, MESSAGES, THREADS};
use crate::util;
use crate::Route;
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use nrzx_shared::agents::{AnswerQuestion, QuestionData, FileEntry, ModelList, ModelOption, NewSubagents, NewThread, PostMessage, RetractedMessage, SubagentTask, ThreadPatch};
use nrzx_shared::{AgentKind, Attachment, ThreadSummary};
use std::collections::HashMap;
use wasm_bindgen::JsCast;


#[derive(Clone, PartialEq)]
struct Item {
    label: String,
    detail: String,
    insert: String,
    dir: bool,
    /// `@@`: Repository der Plattform, das erst als Projekt übernommen wird (Plattform, `owner/name`)
    import: Option<(String, String)>,
}

/// Auslöser für `@@` (andere Repositories) – intern, steht nie im Text.
const REPO: char = 'R';

/// Repositories der verbundenen Plattformen für `@@` (einmal je Sitzung geladen).
static REMOTE_REPOS: GlobalSignal<Option<Vec<nrzx_shared::workspace::NrzGitRepo>>> = Signal::global(|| None);

async fn load_remote_repos() {
    let mut all = Vec::new();
    if let Ok(list) = api::get::<Vec<nrzx_shared::workspace::NrzGitStatus>>("/nrzgit/connections").await {
        for c in list.into_iter().filter(|c| c.connected) {
            if let Ok(repos) = api::get::<Vec<nrzx_shared::workspace::NrzGitRepo>>(&format!("/nrzgit/repos?kind={}", c.kind)).await {
                all.extend(repos);
            }
        }
    }
    *REMOTE_REPOS.write() = Some(all);
}

/// Name für `@@` wie auf dem Server (`workspace::repo_token_key`): klein, Leerzeichen als `-`.
fn repo_key(name: &str) -> String {
    name.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
}

/// Vorschläge für `@@`: Projekte (ausser dem eigenen), dann noch nicht übernommene Repositories.
fn repo_items(query: &str, own: Option<i64>) -> Vec<Item> {
    let q = query.to_lowercase();
    let projects: Vec<nrzx_shared::ProjectSummary> = state::PROJECTS.read().values().cloned().collect();
    let mut items: Vec<Item> = Vec::new();
    let mut sorted = projects.clone();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.last_activity_at));
    for p in sorted.iter().filter(|p| Some(p.id) != own) {
        if !q.is_empty() && !p.name.to_lowercase().contains(&q) && !p.repo.to_lowercase().contains(&q) {
            continue;
        }
        // Kurzer Name, ausser er ist nicht eindeutig
        let key = repo_key(&p.name);
        let unique = projects.iter().filter(|o| repo_key(&o.name) == key).count() == 1;
        let token = if unique || p.repo.is_empty() { key } else { p.repo.clone() };
        let detail = if p.repo.is_empty() { tr("Lokales Projekt") } else { p.repo.clone() };
        items.push(Item { label: format!("@@{token}"), detail, insert: format!("@@{token}"), dir: false, import: None });
    }
    for r in REMOTE_REPOS.read().clone().unwrap_or_default() {
        if items.len() >= 40 {
            break;
        }
        if r.project_id.is_some() || projects.iter().any(|p| p.repo.eq_ignore_ascii_case(&r.full_name)) {
            continue;
        }
        if !q.is_empty() && !r.full_name.to_lowercase().contains(&q) {
            continue;
        }
        items.push(Item {
            label: format!("@@{}", r.full_name),
            detail: tr("wird geklont"),
            insert: format!("@@{}", r.full_name),
            dir: false,
            import: Some((r.forge.clone(), r.full_name.clone())),
        });
    }
    items
}

#[derive(Clone, PartialEq)]
struct Popup {
    trigger: char,
    /// Byte-Bereich des Wortes im Text (mit Auslöser)
    start: usize,
    end: usize,
    query: String,
    items: Vec<Item>,
    active: usize,
}

/// Befehle im Composer (`/…`).
fn commands(thread: bool) -> Vec<Item> {
    let mut v = vec![
        ("/new", tr("Neuer Thread")),
        ("/agent", tr("Agent wählen")),
        ("/model", tr("Modell und Stufe wählen")),
        ("/subagents", tr("Subagenten starten (mehrere Aufgaben)")),
    ];
    if thread {
        v.push(("/status", tr("Stand mit geschätzter Restdauer (Popup, unterbricht nicht)")));
        v.push(("/btw", tr("Zwischenstand erfragen (Seiten-Chat, unterbricht nicht)")));
        v.push(("/stop", tr("Agent stoppen")));
        v.push(("/clear", tr("Kontext zurücksetzen (neue Sitzung beim CLI)")));
    }
    v.into_iter().map(|(c, d)| Item { label: c.into(), detail: d, insert: c.into(), dir: false, import: None }).collect()
}

/// Cursor-Position im Textfeld als Byte-Index (das Feld zählt UTF-16-Einheiten).
fn cursor_of(id: &str, text: &str) -> usize {
    let pos = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        .and_then(|t| t.selection_start().ok().flatten())
        .map(|p| p as usize)
        .unwrap_or(usize::MAX);
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= pos {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// Cursor setzen (Byte-Index) und Feld fokussieren.
fn set_cursor(id: String, text: String, byte: usize) {
    spawn(async move {
        gloo_timers::future::TimeoutFuture::new(0).await;
        let Some(t) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(&id))
            .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        else {
            return;
        };
        let units = text[..byte.min(text.len())].encode_utf16().count() as u32;
        let _ = t.focus();
        let _ = t.set_selection_range(units, units);
    });
}

/// Wort vor dem Cursor, falls es mit `@@` (Auslöser [`REPO`]), `@`, `$` oder (am Anfang) `/` beginnt.
fn token_at(text: &str, cursor: usize) -> Option<(char, usize, String)> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let start = before.rfind(char::is_whitespace).map(|i| i + before[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)).unwrap_or(0);
    let word = &before[start..];
    let trigger = word.chars().next()?;
    if let Some(rest) = word.strip_prefix("@@") {
        return Some((REPO, start, rest.to_string()));
    }
    match trigger {
        '@' | '$' => Some((trigger, start, word[1..].to_string())),
        '/' if start == 0 => Some(('/', 0, word[1..].to_string())),
        _ => None,
    }
}

/// Dateien hochladen (Feld `file`), auch für Bilder im Notizblock.
pub(super) async fn upload_all(list: Vec<web_sys::File>, mut files: Signal<Vec<Attachment>>, mut uploading: Signal<bool>) {
    if list.is_empty() {
        return;
    }
    uploading.set(true);
    for f in list {
        match api::upload::<Attachment>("/attachments", &[f]).await {
            Ok(a) => files.write().push(a),
            Err(e) => toast_error(e),
        }
    }
    uploading.set(false);
}

/// Anhänge im Eingabefeld je Composer (Schlüssel wie beim Text-Entwurf). Sie liegen global und im
/// Browser-Speicher: Beim Wechsel zwischen Chat und Raster (anderes Fenster, gleicher Chat) und nach
/// dem Neuladen sind sie noch da, und ein Upload läuft zu Ende, auch wenn das Eingabefeld dabei
/// verschwindet.
static DRAFT_FILES: GlobalSignal<HashMap<String, Vec<Attachment>>> = Signal::global(HashMap::new);
static DRAFT_UPLOADS: GlobalSignal<HashMap<String, usize>> = Signal::global(HashMap::new);

#[derive(Clone, Copy, PartialEq)]
struct DraftFiles(Memo<String>);

impl DraftFiles {
    fn use_for(field_id: &str) -> Self {
        let id = field_id.to_string();
        DraftFiles(use_memo(use_reactive!(|id| format!("nrz-draft-files-{id}"))))
    }

    fn load(key: &str) -> Vec<Attachment> {
        util::storage_get(key).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Anhänge (beobachtet – für die Darstellung).
    fn get(&self) -> Vec<Attachment> {
        let key = (self.0)();
        DRAFT_FILES.read().get(&key).cloned().unwrap_or_else(|| Self::load(&key))
    }

    fn peek(&self) -> Vec<Attachment> {
        let key = self.0.peek().clone();
        DRAFT_FILES.peek().get(&key).cloned().unwrap_or_else(|| Self::load(&key))
    }

    fn set(&self, list: Vec<Attachment>) {
        Self::store(self.0.peek().clone(), list);
    }

    fn store(key: String, list: Vec<Attachment>) {
        if list.is_empty() {
            util::storage_remove(&key);
        } else if let Ok(s) = serde_json::to_string(&list) {
            util::storage_set(&key, &s);
        }
        DRAFT_FILES.write().insert(key, list);
    }

    /// Anhänge dazunehmen (ohne doppelte).
    fn add(&self, more: Vec<Attachment>) {
        Self::add_to(self.0.peek().clone(), more);
    }

    fn add_to(key: String, more: Vec<Attachment>) {
        let mut list = DRAFT_FILES.peek().get(&key).cloned().unwrap_or_else(|| Self::load(&key));
        for a in more {
            if !list.iter().any(|x| x.uuid == a.uuid) {
                list.push(a);
            }
        }
        Self::store(key, list);
    }

    fn remove(&self, uuid: &str) {
        let mut list = self.peek();
        list.retain(|x| x.uuid != uuid);
        self.set(list);
    }

    fn uploading(&self) -> bool {
        let key = (self.0)();
        DRAFT_UPLOADS.read().get(&key).is_some_and(|n| *n > 0)
    }

    /// Hochladen – unabhängig vom Eingabefeld (läuft beim Wechsel der Ansicht weiter).
    fn upload(&self, list: Vec<web_sys::File>) {
        if list.is_empty() {
            return;
        }
        let key = self.0.peek().clone();
        *DRAFT_UPLOADS.write().entry(key.clone()).or_default() += 1;
        spawn_forever(async move {
            for f in list {
                match api::upload::<Attachment>("/attachments", &[f]).await {
                    Ok(a) => Self::add_to(key.clone(), vec![a]),
                    Err(e) => toast_error(e),
                }
            }
            if let Some(n) = DRAFT_UPLOADS.write().get_mut(&key) {
                *n = n.saturating_sub(1);
            }
        });
    }
}

/// Eingabefeld „Ask anything. @ files, $ skills, / commands“ mit Agent-/Konto-/Modell-Auswahl.
/// Ohne `thread_id` beginnt es einen neuen Thread (im Projekt `project_id`, sonst ohne Projekt)
/// und navigiert dorthin.
#[component]
pub fn Composer(
    project_id: Option<i64>,
    thread_id: Option<i64>,
    /// Neuer Thread in einem leeren Fenster des Rasters: statt zur Thread-Seite zu wechseln, wird
    /// die ID gemeldet (das Fenster zeigt dann den Chat).
    on_created: Option<EventHandler<i64>>,
    /// Eindeutiger Zusatz für die Element-ID (mehrere leere Composer auf einer Seite).
    slot: Option<usize>,
    /// Links über dem Eingabefeld (im Chat: was der Agent gerade tut); rechts steht das Modell.
    status: Option<Element>,
    /// Neuer Thread als Deep Research (recherchiert und schreibt ein Research Paper)
    #[props(default)]
    research: bool,
) -> Element {
    let agents = use_agents();
    let nav = use_navigator();
    let field_id = match (thread_id, slot) {
        (Some(i), _) => format!("composer-{i}"),
        (None, Some(n)) => format!("composer-new-{n}"),
        (None, None) => "composer-new".into(),
    };
    // Entwurf: Ungesendeter Text bleibt je Chat im Eingabefeld (auch nach Wechseln und Neuladen)
    let draft_key = format!("nrz-draft-{field_id}");
    let mut text = use_signal({
        let k = draft_key.clone();
        move || util::storage_get(&k).unwrap_or_default()
    });
    let draft_store = use_signal({
        let k = draft_key.clone();
        move || k
    });
    use_effect(use_reactive!(|draft_key| {
        let t = text();
        if t.is_empty() {
            util::storage_remove(&draft_key);
        } else {
            util::storage_set(&draft_key, &t);
        }
    }));
    let files = DraftFiles::use_for(&field_id);
    let mut sending = use_signal(|| false);
    // Standard: direkt auf dem Standard-Branch (main) arbeiten; eigener Worktree nur auf Wunsch
    let worktree = use_signal(|| false);
    // „Nur lesen“ gibt es beim Start nicht mehr (im Thread weiter über das Kontextmenü)
    let read_only = use_signal(|| false);
    let mut freestyle = use_signal(|| util::storage_get(FREESTYLE_KEY).as_deref() == Some("1"));
    let mut popup = use_signal(|| None::<Popup>);
    let mut picker = use_signal(|| false);
    let mut sub_open = use_signal(|| false);
    let mut sel = use_signal(move || match thread_id.and_then(|id| THREADS.peek().get(&id).cloned()) {
        Some(t) => Selection::of_thread(&t),
        None => Selection::remembered(),
    });
    let fid = use_signal({
        let f = field_id.clone();
        move || f
    });
    // Daneben abgelegte Dateien (im Chat, auf der Startseite …) landen im nächsten Eingabefeld
    use_effect(move || {
        let hit = util::FILE_DROP.read().as_ref().is_some_and(|(id, _)| *id == *fid.peek());
        if hit {
            if let Some((_, list)) = util::FILE_DROP.write().take() {
                files.upload(list);
            }
        }
    });

    // Auswahl am Thread ausrichten (Änderungen kommen live)
    use_effect(move || {
        if let Some(id) = thread_id {
            if let Some(t) = THREADS.read().get(&id) {
                let s = Selection::of_thread(t);
                if *sel.peek() != s {
                    sel.set(s);
                }
            }
        }
    });
    // Neuer Thread: gültiges Konto wählen, sobald die Agenten geladen sind
    use_effect(move || {
        if thread_id.is_some() {
            return;
        }
        let list = AGENTS.read().clone();
        if let Some(list) = list {
            let mut s = sel.peek().clone();
            let has = |k: AgentKind| list.iter().any(|a| a.kind == k && !a.accounts.is_empty());
            if !has(s.agent) {
                if let Some(first) = list.iter().find(|a| !a.accounts.is_empty()) {
                    s = s.with_agent(first.kind, &list);
                }
            }
            let s = s.normalized(&list);
            if s != *sel.peek() {
                sel.set(s);
            }
        }
    });

    // Text und Anhänge übernehmen (zurückgezogene Nachricht, Notiz aus dem Notizblock)
    use_effect(move || {
        let d = COMPOSER_DRAFT.read().clone();
        if let Some(d) = d {
            if d.thread_id == thread_id {
                *COMPOSER_DRAFT.write() = None;
                text.set(d.text.clone());
                files.add(d.attachments);
                set_cursor(fid.peek().clone(), d.text.clone(), d.text.len());
            }
        }
    });

    let thread = thread_id.and_then(|id| THREADS.read().get(&id).cloned());
    let active = thread.as_ref().is_some_and(|t| t.status.is_active());
    let thread_read_only = thread.as_ref().is_some_and(|t| t.read_only);
    // Offene Rückfrage: Senden beantwortet sie
    let question: Option<i64> = thread_id.and_then(|id| {
        MESSAGES.read().get(&id).and_then(|l| {
            l.iter()
                .rev()
                .take(30)
                .find(|m| m.role == "question" && serde_json::from_value::<QuestionData>(m.data.clone()).is_ok_and(|d| d.answer.is_none() && !d.expired))
                .map(|m| m.id)
        })
    });

    // Auswahl übernehmen (bei bestehendem Thread auf dem Server wechseln)
    let mut apply = move |s: Selection| {
        sel.set(s.clone());
        match thread_id {
            Some(id) => {
                spawn(async move {
                    let patch = ThreadPatch {
                        agent: Some(s.agent),
                        account_id: s.account_id,
                        model: Some(s.model.clone()),
                        effort: Some(s.effort.clone()),
                        ..Default::default()
                    };
                    match api::patch::<ThreadSummary, _>(&format!("/threads/{id}"), &patch).await {
                        Ok(t) => state::upsert_thread(t),
                        Err(e) => {
                            toast_error(e);
                            if let Some(t) = THREADS.peek().get(&id) {
                                sel.set(Selection::of_thread(t));
                            }
                        }
                    }
                });
            }
            None => s.remember(),
        }
    };

    // Befehl ausführen; true = erledigt
    let mut run_command = move |cmd: &str| -> bool {
        match cmd {
            "/new" => {
                nav.push(Route::Home {});
            }
            "/agent" | "/model" => picker.set(true),
            "/status" => {
                if let Some(id) = thread_id {
                    super::status::open(id);
                }
            }
            "/subagents" => sub_open.set(true),
            "/stop" => {
                if let Some(id) = thread_id {
                    spawn(async move {
                        match api::action::<ThreadSummary>(&format!("/threads/{id}/stop")).await {
                            Ok(s) => state::upsert_thread(s),
                            Err(e) => toast_error(e),
                        }
                    });
                }
            }
            "/clear" => {
                if let Some(id) = thread_id {
                    spawn(async move {
                        match api::action::<ThreadSummary>(&format!("/threads/{id}/clear")).await {
                            Ok(s) => state::upsert_thread(s),
                            Err(e) => toast_error(e),
                        }
                    });
                }
            }
            _ => return false,
        }
        true
    };

    // interrupt: arbeitet der Agent, ihn unterbrechen statt in die Warteschlange zu stellen
    let mut send_as = move |interrupt: bool| {
        let body = text().trim().to_string();
        if (body.is_empty() && files.peek().is_empty()) || sending() {
            return;
        }
        // /btw <Frage>: Zwischenstand im Seiten-Chat, der Agent läuft weiter
        if let (Some(id), Some(rest)) = (thread_id, body.strip_prefix("/btw")) {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                let q = rest.trim();
                super::btw::ask(id, if q.is_empty() { tr("Wie ist der aktuelle Stand?") } else { q.to_string() });
                text.set(String::new());
                return;
            }
        }
        if body.starts_with('/') && !body.contains(char::is_whitespace) && files.peek().is_empty() && run_command(&body) {
            text.set(String::new());
            return;
        }
        sending.set(true);
        // Neuer Thread: Eingabe sofort leeren – auch den gespeicherten Entwurf, sonst steht der
        // Prompt nach dem Wechsel in den Chat beim nächsten Besuch wieder da. Bei einem Fehler
        // kommt der Text zurück.
        let typed = text();
        if thread_id.is_none() {
            text.set(String::new());
            util::storage_remove(&draft_store.peek());
        }
        // Neuer Thread von der Startseite: sofort als Chat zeigen (Seitenleiste links und die
        // gesendete Nachricht), statt bis zur Antwort des Servers nichts zu tun
        let placeholder = thread_id.is_none() && on_created.is_none();
        if placeholder {
            *state::PENDING_THREAD.write() = Some(state::PendingThread {
                project_id,
                title: nrzx_shared::agents::title_from(&body),
                text: body.clone(),
                attachments: files.peek(),
                agent: sel.peek().agent,
                created_at: util::now(),
                known: THREADS.peek().keys().copied().collect(),
            });
        }
        spawn(async move {
            let attachments: Vec<String> = files.peek().iter().map(|a| a.uuid.clone()).collect();
            let result = match thread_id {
                Some(id) if question.is_some() && attachments.is_empty() => {
                    let mid = question.unwrap_or_default();
                    api::post::<ThreadSummary, _>(&format!("/threads/{id}/questions/{mid}"), &AnswerQuestion { answer: body, delegate: false }).await
                }
                Some(id) => api::post::<ThreadSummary, _>(&format!("/threads/{id}/messages"), &PostMessage { text: body, attachments, interrupt }).await,
                None => {
                    let s = sel();
                    let req = NewThread {
                        project_id,
                        prompt: body,
                        agent: s.agent,
                        account_id: s.account_id,
                        model: s.model.clone(),
                        effort: s.effort.clone(),
                        use_worktree: worktree(),
                        read_only: read_only(),
                        freestyle: freestyle() && !read_only(),
                        title: None,
                        attachments,
                        tasks: Vec::new(),
                        research,
                    };
                    api::post::<ThreadSummary, _>("/threads", &req).await
                }
            };
            match result {
                Ok(t) => {
                    files.set(Vec::new());
                    sending.set(false);
                    if thread_id.is_some() {
                        text.set(String::new());
                        state::upsert_thread(t);
                    } else {
                        // Direkt in den Chat: Thread zuerst in den Zustand, dann wechseln
                        util::storage_remove(&draft_store.peek());
                        sel().remember();
                        let id = t.id;
                        state::upsert_thread(t);
                        match on_created {
                            Some(h) => h.call(id),
                            None => {
                                // Verlauf vorher laden: der Chat steht beim Wechsel sofort da
                                // (kein Ladekreis, kein Springen)
                                let _ = super::chat::load_messages(id).await;
                                nav.push(Route::ThreadPage { id });
                                *state::PENDING_THREAD.write() = None;
                            }
                        }
                    }
                    return;
                }
                Err(e) => {
                    if placeholder {
                        *state::PENDING_THREAD.write() = None;
                    }
                    if thread_id.is_none() && text.peek().is_empty() {
                        text.set(typed);
                    }
                    toast_error(e);
                }
            }
            sending.set(false);
        });
    };

    // Vorschläge für `@` / `$` / `/` aktualisieren
    let mut refresh_popup = move |value: &str| {
        let cursor = cursor_of(&fid.peek(), value);
        let Some((trigger, start, query)) = token_at(value, cursor) else {
            popup.set(None);
            return;
        };
        let q = query.to_lowercase();
        let old = popup.peek().clone();
        let mut p = Popup { trigger, start, end: cursor, query: query.clone(), items: Vec::new(), active: 0 };
        match trigger {
            '/' => {
                p.items = commands(thread_id.is_some()).into_iter().filter(|c| c.label[1..].starts_with(&q)).collect();
            }
            REPO => {
                if REMOTE_REPOS.peek().is_none() {
                    // Erst Projekte zeigen, die Repositories der Plattformen kommen dazu
                    *REMOTE_REPOS.write() = Some(Vec::new());
                    spawn(async move {
                        load_remote_repos().await;
                        let mut cur = popup.write();
                        if let Some(p) = cur.as_mut().filter(|p| p.trigger == REPO) {
                            p.items = repo_items(&p.query, project_id);
                            p.active = p.active.min(p.items.len().saturating_sub(1));
                        }
                    });
                }
                p.items = repo_items(&query, project_id);
            }
            '$' => {
                if SKILLS.peek().is_none() {
                    spawn(data::reload_skills());
                }
                p.items = SKILLS
                    .read()
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| s.name.to_lowercase().contains(&q))
                    .map(|s| Item { label: format!("${}", s.name), detail: s.description.clone(), insert: s.prompt.clone(), dir: false, import: None })
                    .collect();
            }
            _ => {
                // Dateien: alte Liste behalten, bis die neue da ist
                p.items = old.filter(|o| o.trigger == '@').map(|o| o.items).unwrap_or_default();
                let url = match (thread_id, project_id) {
                    (Some(id), _) => Some(format!("/threads/{id}/files?q={}", api::enc(&query))),
                    (None, Some(pid)) => Some(format!("/agents/files?project={pid}&q={}", api::enc(&query))),
                    _ => None,
                };
                if let Some(url) = url {
                    let query = query.clone();
                    spawn(async move {
                        gloo_timers::future::TimeoutFuture::new(120).await;
                        if popup.peek().as_ref().map(|p| &p.query) != Some(&query) {
                            return;
                        }
                        if let Ok(list) = api::get::<Vec<FileEntry>>(&url).await {
                            let mut cur = popup.write();
                            if let Some(p) = cur.as_mut().filter(|p| p.query == query && p.trigger == '@') {
                                p.items = list
                                    .into_iter()
                                    .map(|f| Item { label: f.path.clone(), detail: String::new(), insert: format!("@{}", f.path), dir: f.dir, import: None })
                                    .collect();
                                p.active = p.active.min(p.items.len().saturating_sub(1));
                            }
                        }
                    });
                }
            }
        }
        popup.set(Some(p));
    };

    // Vorschlag übernehmen
    let mut choose = move |item: Item| {
        let Some(p) = popup.peek().clone() else { return };
        let mut value = text.peek().clone();
        let end = p.end.min(value.len());
        if p.trigger == '/' {
            popup.set(None);
            if run_command(&item.insert) {
                text.set(String::new());
            }
            return;
        }
        if let Some((forge, repo)) = item.import.clone() {
            // Repository erst als Projekt übernehmen (klont im Hintergrund; der Lauf wartet darauf)
            spawn(async move {
                let body = nrzx_shared::workspace::ImportProject { repo, forge };
                match api::post::<nrzx_shared::ProjectSummary, _>("/projects/import", &body).await {
                    Ok(p) => {
                        state::PROJECTS.write().insert(p.id, p);
                    }
                    Err(e) => toast_error(e),
                }
            });
        }
        let insert = if p.trigger == '@' && item.dir { item.insert.clone() } else { format!("{} ", item.insert) };
        value.replace_range(p.start..end, &insert);
        let cursor = p.start + insert.len();
        text.set(value.clone());
        popup.set(None);
        set_cursor(fid.peek().clone(), value, cursor);
    };

    let s = sel();
    // „Schreib an Claude …“: kurzer Name des gewählten Agenten
    // Im Gespräch mit einem Bot: sein Name
    let who = thread
        .as_ref()
        .filter(|t| t.bot_id.is_some())
        .map(|t| t.title.clone())
        .unwrap_or_else(|| s.agent.name().split_whitespace().next().unwrap_or("").to_string());
    // Feld wächst mit dem Text (auch beim Diktieren und Einfügen, nicht nur bei Zeilenumbrüchen)
    use_effect(move || {
        let _ = text();
        let id = fid.peek().clone();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            grow(&id);
        });
    });

    let queued = thread.as_ref().map(|t| t.queued.clone()).unwrap_or_default();
    rsx! {
        div { class: "relative",
            if let (Some(tid), false) = (thread_id, queued.is_empty()) {
                QueueList {
                    thread_id: tid,
                    items: queued,
                    on_withdraw: move |r: RetractedMessage| {
                        // Zurückgezogene Nachricht unten anhängen (Leerzeile dazwischen)
                        files.add(r.attachments);
                        let mut v = text.peek().trim_end().to_string();
                        if !v.is_empty() && !r.text.trim().is_empty() {
                            v.push_str("\n\n");
                        }
                        v.push_str(&r.text);
                        text.set(v.clone());
                        let end = v.len();
                        set_cursor(fid.peek().clone(), v, end);
                    },
                }
            }
            // /status: Stand mit Schätzung als Popup über der Eingabe
            if let Some(id) = thread_id.filter(|id| super::status::STATUS_OPEN() == Some(*id)) {
                super::status::StatusPopup { thread_id: id }
            }
            // Vorschläge
            if let Some(p) = popup() {
                if !p.items.is_empty() {
                    div { class: "nrx-pop absolute bottom-full left-0 z-40 mb-2 max-h-72 w-full max-w-lg overflow-y-auto p-1.5", "data-popup": "1",
                        for (i , item) in p.items.iter().cloned().enumerate() {
                            button {
                                key: "{i}",
                                class: if i == p.active { "flex w-full items-center gap-2 rounded-md bg-accent px-2 py-1.5 text-left text-sm" } else { "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent/60" },
                                onmousedown: move |e| e.prevent_default(),
                                onclick: {
                                    let item = item.clone();
                                    move |_| choose(item.clone())
                                },
                                span { class: "shrink-0 text-muted-foreground",
                                    match p.trigger {
                                        '@' if item.dir => rsx! { icons::Folder { class: "size-3.5" } },
                                        '@' => rsx! { icons::File { class: "size-3.5" } },
                                        '$' => rsx! { icons::Sparkles { class: "size-3.5" } },
                                        REPO if item.import.is_some() => rsx! { icons::CloudDownload { class: "size-3.5" } },
                                        REPO => rsx! { icons::FolderGit2 { class: "size-3.5" } },
                                        _ => rsx! { icons::SquareSlash { class: "size-3.5" } },
                                    }
                                }
                                span { class: "truncate font-mono text-[13px]", "{item.label}" }
                                if !item.detail.is_empty() {
                                    span { class: "ml-auto truncate pl-3 text-xs text-muted-foreground", "{item.detail}" }
                                }
                            }
                        }
                    }
                }
            }
            // Über dem Feld: links der Zustand (im Chat), rechts das Modell (Auswahl als Popup)
            div { class: "flex min-h-7 items-center gap-2 px-1 pb-1.5 text-[13px] text-muted-foreground",
                div { class: "min-w-0 flex-1", {status} }
                div { class: "shrink-0",
                        Picker {
                        open: picker,
                        sel: s.clone(),
                        agents: agents.clone().unwrap_or_default(),
                        onchange: move |n: Selection| apply(n),
                        access: if read_only() || research { None } else { match &thread { Some(t) => Some(t.freestyle), None if thread_id.is_none() => Some(freestyle()), None => None } },
                        onaccess: move |v: bool| {
                            match thread_id {
                                // Im Thread: gleich am Thread speichern (wie im Kontextmenü)
                                Some(id) => {
                                    spawn(async move {
                                        match api::patch::<ThreadSummary, _>(&format!("/threads/{id}"), &ThreadPatch { freestyle: Some(v), ..Default::default() }).await {
                                            Ok(s) => state::upsert_thread(s),
                                            Err(e) => toast_error(e),
                                        }
                                    });
                                }
                                None => {
                                    freestyle.set(v);
                                    util::storage_set(FREESTYLE_KEY, if v { "1" } else { "0" });
                                }
                            }
                        },
                        worktree: (thread_id.is_none() && project_id.is_some() && !read_only()).then_some(worktree),
                    }
                }
            }
            div {
                "data-composer": "{field_id}",
                class: if util::DRAGGING_FILES() { "relative rounded-xl border border-primary/70 bg-black/30 ring-2 ring-primary/30 transition-colors" } else { "relative rounded-xl border border-white/[0.13] bg-black/25 shadow-[inset_0_1px_0_rgb(255_255_255/0.04)] transition-colors focus-within:border-white/25" },
                ondragover: move |e| {
                    e.prevent_default();
                    if let Some(dt) = util::drag_event(&e).and_then(|d| d.data_transfer()) {
                        dt.set_drop_effect("copy");
                    }
                },
                ondrop: move |e| {
                    e.prevent_default();
                    let list = util::dropped_files(&e);
                    if !list.is_empty() {
                        files.upload(list);
                    }
                },
                if util::DRAGGING_FILES() {
                    div { class: "pointer-events-none absolute inset-0 z-10 grid place-items-center rounded-xl bg-card/80 text-sm font-medium text-primary backdrop-blur-sm",
                        div { class: "flex items-center gap-2",
                            icons::Paperclip { class: "size-4" }
                            {tr("Datei hier ablegen")}
                        }
                    }
                }
                if !files.get().is_empty() {
                    div { class: "flex flex-wrap gap-2 px-4 pt-3",
                        for a in files.get() {
                            div { key: "{a.uuid}", class: "group relative",
                                if a.content_type.starts_with("image/") {
                                    img {
                                        class: "size-14 cursor-zoom-in rounded-lg border object-cover",
                                        src: "{a.url}",
                                        alt: "{a.name}",
                                        onclick: {
                                            let all: Vec<(String, String)> = files.peek().iter().filter(|x| x.content_type.starts_with("image/")).map(|x| (x.url.clone(), x.name.clone())).collect();
                                            let idx = all.iter().position(|(u, _)| *u == a.url).unwrap_or(0);
                                            move |_| crate::components::overlay::view_images(all.clone(), idx)
                                        },
                                    }
                                } else {
                                    div { class: "flex h-14 items-center gap-1.5 rounded-lg border px-2.5 text-xs",
                                        icons::Paperclip { class: "size-3.5" }
                                        span { class: "max-w-40 truncate", "{a.name}" }
                                    }
                                }
                                button {
                                    class: "absolute -top-1.5 -right-1.5 hidden size-5 place-items-center rounded-full border bg-background text-muted-foreground group-hover:grid hover:text-foreground",
                                    title: tr("Entfernen"),
                                    onclick: {
                                        let uuid = a.uuid.clone();
                                        move |_| files.remove(&uuid)
                                    },
                                    icons::X { class: "size-3" }
                                }
                            }
                        }
                    }
                }
                div { class: "flex items-end gap-1 py-1 pr-1.5 pl-3",
                span { class: "shrink-0 pb-[9px] font-mono text-[15px] text-muted-foreground", "›" }
                textarea {
                    id: "{field_id}",
                    class: "block min-w-0 flex-1 resize-none bg-transparent px-1.5 py-2 text-[15px] leading-relaxed outline-none placeholder:text-muted-foreground",
                    rows: "1",
                    placeholder: if research { tr("Was soll recherchiert werden? Frage oder Thema …") } else if question.is_some() { tr("Antwort auf die Rückfrage …") } else if active { trf("Schreib an {0} … (geht raus, sobald er fertig ist)", &[&who]) } else if thread_read_only || (thread_id.is_none() && read_only()) { trf("Frag {0} zur Codebasis – er liest nur …", &[&who]) } else { trf("Schreib an {0} …", &[&who]) },
                    value: "{text}",
                    oninput: move |e| {
                        let v = e.value();
                        text.set(v.clone());
                        refresh_popup(&v);
                    },
                    onclick: move |_| {
                        let v = text.peek().clone();
                        refresh_popup(&v);
                    },
                    // Doppelklick ins Leere: Dateien anhängen (Auswahl des Systems). Auf ein Wort
                    // markiert er es wie gewohnt – dann keine Auswahl
                    ondoubleclick: move |_| {
                        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
                        if let Some(t) = doc.get_element_by_id(&fid.peek()).and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok()) {
                            if word_selected(&t) {
                                return;
                            }
                        }
                        let id = format!("{}-files", fid.peek());
                        if let Some(input) = doc.get_element_by_id(&id).and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok()) {
                            input.click();
                        }
                    },
                    onblur: move |_| {
                        spawn(async move {
                            gloo_timers::future::TimeoutFuture::new(150).await;
                            popup.set(None);
                        });
                    },
                    onkeydown: move |e: KeyboardEvent| {
                        let current = popup.peek().clone();
                        if let Some(p) = current {
                            if !p.items.is_empty() {
                                match e.key() {
                                    Key::ArrowDown => {
                                        e.prevent_default();
                                        if let Some(p) = popup.write().as_mut() {
                                            p.active = (p.active + 1) % p.items.len();
                                        }
                                        return;
                                    }
                                    Key::ArrowUp => {
                                        e.prevent_default();
                                        if let Some(p) = popup.write().as_mut() {
                                            p.active = (p.active + p.items.len() - 1) % p.items.len();
                                        }
                                        return;
                                    }
                                    Key::Enter | Key::Tab => {
                                        e.prevent_default();
                                        choose(p.items[p.active.min(p.items.len() - 1)].clone());
                                        return;
                                    }
                                    Key::Escape => {
                                        e.prevent_default();
                                        popup.set(None);
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if e.key() == Key::Enter && !e.modifiers().contains(Modifiers::SHIFT) && !e.is_composing() {
                            e.prevent_default();
                            // ⌘/Strg+Enter während der Agent arbeitet: zwischenrufen
                            send_as(active && (e.modifiers().contains(Modifiers::META) || e.modifiers().contains(Modifiers::CONTROL)));
                        }
                    },
                    onpaste: move |e: ClipboardEvent| {
                        // Aus nrzX kopierte Nachricht: Text und Anhänge (Bilder) übernehmen
                        if let Some((t, atts)) = pasted_nrzx(&e) {
                            e.prevent_default();
                            files.add(atts);
                            let mut v = text.peek().clone();
                            let at = cursor_of(&fid.peek(), &v);
                            v.insert_str(at.min(v.len()), &t);
                            text.set(v.clone());
                            set_cursor(fid.peek().clone(), v, at + t.len());
                            return;
                        }
                        let list = pasted_files(&e);
                        if !list.is_empty() {
                            e.prevent_default();
                            files.upload(list);
                        }
                    },
                }
                // Rechts nur das Mikrofon (wie im Bild): Senden mit Enter, Anhänge per Ablegen
                // oder Doppelklick ins Feld, Stoppen über die Zeile darüber
                div { class: "flex shrink-0 items-center pb-[3px]",
                    input {
                        id: "{field_id}-files",
                        r#type: "file",
                        multiple: true,
                        class: "hidden",
                        onchange: move |e| {
                            files.upload(util::input_files(&e));
                        },
                    }
                    if files.uploading() || sending() {
                        icons::LoaderCircle { class: "mr-2 size-4 animate-spin text-muted-foreground" }
                    }
                    DictateButton { field: fid.peek().clone(), text }
                }
                }
            }
            SubagentsDialog { open: sub_open, thread_id, project_id, sel: s }
        }
    }
}

/// Diktieren (live): Klick bzw. ⌥ startet das Mikrofon. Während man spricht, geht der Satz bis
/// jetzt immer wieder an Whisper auf dem Server (`POST /voice/transcribe`, Modell und Sprache aus
/// Einstellungen → Dieter) und erscheint gleich im Eingabefeld – Zeichen für Zeichen getippt. An
/// der Sprechpause ersetzt die Erkennung des ganzen Satzes den Zwischenstand. Noch einmal klicken
/// beendet – der angefangene Satz kommt noch mit. Fehlt das Sprachmodell, wird es geladen.
#[component]
fn DictateButton(field: String, text: Signal<String>) -> Element {
    let mut on = use_signal(|| false);
    let mut busy = use_signal(|| 0usize);
    let mut level = use_signal(|| 0.0f32);
    let owner = use_signal({
        let f = field.clone();
        move || f
    });
    use_drop(move || mic::dictate_stop(&owner.peek()));
    // Live: wohin das Feld getippt wird, Text vor dem laufenden Satz, Nummer des Satzes
    let mut goal = use_signal(|| None::<String>);
    let mut typing = use_signal(|| false);
    let mut base = use_signal(|| None::<String>);
    let mut utter = use_signal(|| 0u64);
    let mut partial_busy = use_signal(|| false);
    // Feste Wörter des laufenden Satzes (Zwischenstände ändern sie nicht mehr)
    let mut steady = use_signal(mic::Steady::default);
    // Schreibmaschine: tippt das Feld Zeichen für Zeichen zum Ziel. Weicht der Anfang ab (Whisper
    // hat sich korrigiert), wird ab dort sofort ersetzt und weiter getippt
    let mut type_to = move |g: String| {
        // Was schon dasteht, sofort angleichen (Whisper korrigiert sich – nichts wird neu getippt)
        let cur = text.peek().clone();
        let now: String = g.chars().take(cur.chars().count()).collect();
        if now != cur {
            let end = now.len();
            text.set(now.clone());
            set_cursor(owner.peek().clone(), now, end);
        }
        goal.set(Some(g));
        if *typing.peek() {
            return;
        }
        typing.set(true);
        spawn(async move {
            loop {
                let Some(g) = goal.peek().clone() else { break };
                let cur = text.peek().clone();
                if cur == g {
                    goal.set(None);
                    break;
                }
                // Nur was dazukommt, erscheint Zeichen für Zeichen
                let common = if g.starts_with(cur.as_str()) { cur.len() } else { common_prefix(&cur, &g) };
                let rest = &g[common..];
                let behind = rest.chars().count();
                let step = if behind > 48 { 4 } else if behind > 16 { 2 } else { 1 };
                let mut next = g[..common].to_string();
                next.extend(rest.chars().take(step));
                let end = next.len();
                text.set(next.clone());
                set_cursor(owner.peek().clone(), next, end);
                gloo_timers::future::TimeoutFuture::new(16).await;
            }
            typing.set(false);
        });
    };
    // Text vor dem laufenden Satz (beim ersten Ergebnis des Satzes festgehalten)
    let mut base_now = move || -> String {
        if let Some(b) = base.peek().clone() {
            return b;
        }
        let mut v = goal.peek().clone().unwrap_or_else(|| text.peek().clone());
        if !v.is_empty() && !v.ends_with(char::is_whitespace) {
            v.push(' ');
        }
        base.set(Some(v.clone()));
        v
    };
    let toggle = move |_: ()| {
        let id = owner.peek().clone();
        if on() || mic::dictating(&id) {
            mic::dictate_stop(&id);
            on.set(false);
            return;
        }
        on.set(true);
        goal.set(None);
        base.set(None);
        steady.write().reset();
        spawn(async move {
            let (tx, mut rx) = futures_channel::mpsc::unbounded::<mic::Event>();
            if let Err(e) = mic::dictate_start(&id, tx).await {
                on.set(false);
                toast_error(match e.as_str() {
                    "denied" => tr("Kein Zugriff aufs Mikrofon – bitte in den Einstellungen des Systems bzw. Browsers erlauben."),
                    "nodevice" => tr("Kein Mikrofon gefunden."),
                    "unsupported" => tr("Dieser Browser kann nicht aufnehmen (nur über HTTPS oder in der App)."),
                    _ => e,
                });
                return;
            }
            use futures_util::StreamExt;
            while let Some(ev) = rx.next().await {
                match ev {
                    mic::Event::Level(x) => level.set(x),
                    // Satz bis jetzt: erkennen lassen (immer nur einer unterwegs) und gleich tippen
                    mic::Event::Partial(pcm) => {
                        if *partial_busy.peek() {
                            continue;
                        }
                        partial_busy.set(true);
                        let n = *utter.peek();
                        spawn(async move {
                            let r = api::post_bytes::<nrzx_shared::voice::Transcription>("/voice/transcribe?fast=true", &mic::wav(&pcm), "audio/wav").await;
                            partial_busy.set(false);
                            // Satz inzwischen zu Ende: der genaue Text kommt gleich
                            if *utter.peek() != n {
                                return;
                            }
                            if let Ok(t) = r {
                                let said = t.text.trim();
                                if !said.is_empty() {
                                    let b = base_now();
                                    let shown = steady.write().push(said);
                                    type_to(format!("{b}{shown}"));
                                }
                            }
                        });
                    }
                    mic::Event::Utterance(pcm) => {
                        // Zwischenstände dieses Satzes zählen ab jetzt nicht mehr
                        *utter.write() += 1;
                        steady.write().reset();
                        *busy.write() += 1;
                        let r = api::post_bytes::<nrzx_shared::voice::Transcription>("/voice/transcribe", &mic::wav(&pcm), "audio/wav").await;
                        *busy.write() -= 1;
                        match r {
                            Ok(t) => {
                                let said = t.text.trim();
                                let b = base_now();
                                base.set(None);
                                // Nichts erkannt: Zwischenstand wieder weg
                                type_to(if said.is_empty() { b.trim_end().to_string() } else { format!("{b}{said}") });
                            }
                            Err(e) if e.status == 409 => {
                                mic::dictate_stop(&owner.peek());
                                on.set(false);
                                load_stt_model(e.message).await;
                            }
                            Err(e) => toast_error(e),
                        }
                    }
                }
            }
            // Kanal zu: beendet (oder ein anderes Feld diktiert jetzt)
            on.set(false);
            level.set(0.0);
        });
    };
    // ⌥ allein kurz drücken (ohne andere Taste, im eigenen Feld): Diktat an/aus. Die Listener am
    // Fenster gehören zu diesem Feld und gehen mit ihm – blieben sie (früher), griff jeder spätere
    // Tastendruck auf die Signale des geschlossenen Felds zu: Absturz und Neuladen der Oberfläche
    let mut alt_tick = use_signal(|| 0u32);
    let alt_keys = use_hook(move || {
        use wasm_bindgen::closure::Closure;
        let alone = std::rc::Rc::new(std::cell::Cell::new(false));
        let (a1, a2) = (alone.clone(), alone);
        let down = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| a1.set(e.key() == "Alt" && !e.repeat()));
        let up = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
            if !a2.replace(false) || e.key() != "Alt" {
                return;
            }
            let Ok(me) = owner.try_peek().map(|o| o.clone()) else { return };
            let mine = web_sys::window().and_then(|w| w.document()).and_then(|d| d.active_element()).is_some_and(|el| el.id() == me);
            if mine {
                if let Ok(mut t) = alt_tick.try_write() {
                    *t += 1;
                }
            }
        });
        std::rc::Rc::new(crate::util::WindowKeys::listen(vec![("keydown", down), ("keyup", up)]))
    });
    use_drop(move || alt_keys.remove());
    let mut handled = use_signal(|| 0u32);
    use_effect(move || {
        let t = alt_tick();
        if t > *handled.peek() {
            handled.set(t);
            let mut toggle = toggle;
            spawn(async move { toggle(()) });
        }
    });
    let lvl = (level() * 12.0).min(1.0);
    rsx! {
        button {
            class: if on() { "relative inline-flex h-8 items-center gap-1.5 rounded-lg border border-destructive/40 bg-destructive/15 px-2 text-destructive" } else { "relative inline-flex h-8 items-center gap-1.5 rounded-lg border border-white/[0.12] bg-white/[0.04] px-2 text-muted-foreground transition-colors hover:bg-white/[0.08] hover:text-foreground" },
            title: if on() { tr("Diktat beenden (⌥)") } else { tr("Diktieren – oder ⌥ kurz drücken") },
            onclick: move |_| {
                let mut toggle = toggle;
                toggle(());
            },
            if on() {
                // Pegel als weicher Rahmen
                span { class: "pointer-events-none absolute inset-0 rounded-lg border-2 border-destructive/60 transition-transform duration-75", style: "transform: scale({1.0 + lvl * 0.12:.2}); opacity: {0.25 + lvl * 0.75:.2}" }
            }
            if busy() > 0 && !on() {
                icons::LoaderCircle { class: "size-4 animate-spin" }
            } else {
                icons::Mic { class: "size-4" }
            }
            span { class: "text-[13px] leading-none", "⌥" }
        }
    }
}

/// Höhe des Eingabefelds nach dem Inhalt (höchstens etwa zwölf Zeilen, dann scrollt es). Zum
/// Messen ist das Feld kurz „auto“ hoch – die Fläche des Chats darüber würde dabei grösser und der
/// Browser schöbe ihre Stelle zurecht, der Text spränge beim Tippen. Darum bleibt die Stelle jedes
/// Chats (`data-chat-scroll`) gleich.
fn grow(id: &str) {
    const MAX: i32 = 300;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(t) = doc.get_element_by_id(id).and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok()) else {
        return;
    };
    let chats: Vec<(web_sys::Element, i32)> = doc
        .query_selector_all("[data-chat-scroll]")
        .map(|l| (0..l.length()).filter_map(|i| l.item(i)?.dyn_into::<web_sys::Element>().ok()).map(|e| {
            let top = e.scroll_top();
            (e, top)
        }).collect())
        .unwrap_or_default();
    let style = t.style();
    let _ = style.set_property("height", "auto");
    let h = t.scroll_height();
    let _ = style.set_property("height", &format!("{}px", h.min(MAX)));
    let _ = style.set_property("overflow-y", if h > MAX { "auto" } else { "hidden" });
    for (e, top) in chats {
        if e.scroll_top() != top {
            e.set_scroll_top(top);
        }
    }
}

/// Ist im Feld gerade Text (nicht nur Leerraum) markiert – etwa ein Wort per Doppelklick?
fn word_selected(t: &web_sys::HtmlTextAreaElement) -> bool {
    let (Ok(Some(a)), Ok(Some(b))) = (t.selection_start(), t.selection_end()) else { return false };
    if a >= b {
        return false;
    }
    // Die Positionen zählen UTF-16-Einheiten
    let units: Vec<u16> = t.value().encode_utf16().collect();
    let part = units.get(a as usize..(b as usize).min(units.len())).unwrap_or_default();
    !String::from_utf16_lossy(part).trim().is_empty()
}

/// Länge (Bytes, an Zeichengrenze) des gemeinsamen Anfangs zweier Texte.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices().zip(b.chars()).take_while(|((_, x), y)| x == y).last().map(|((i, c), _)| i + c.len_utf8()).unwrap_or(0)
}

/// Sprachmodell fehlt (409 von `/voice/transcribe`): das eingestellte Modell laden.
async fn load_stt_model(message: String) {
    let Ok(s) = api::get::<nrzx_shared::voice::VoiceSettings>("/voice/settings").await else {
        toast_error(message);
        return;
    };
    match api::action::<Vec<nrzx_shared::voice::SttModel>>(&format!("/voice/models/{}/download", api::enc(&s.stt_model))).await {
        Ok(_) => toast_error(tr("Das Sprachmodell für das Diktieren wird geladen – gleich noch einmal versuchen.")),
        Err(_) => toast_error(message),
    }
}

/// Wartende Nachrichten über dem Composer: zurückziehen (Text und Anhänge kommen zum
/// Bearbeiten in den Composer) oder sofort senden (unterbricht).
#[component]
fn QueueList(thread_id: i64, items: Vec<nrzx_shared::QueuedMessage>, on_withdraw: EventHandler<RetractedMessage>) -> Element {
    let act = move |path: String| {
        spawn(async move {
            // Die Liste aktualisiert sich über die Live-Ereignisse (ThreadUpdated)
            match api::action::<ThreadSummary>(&path).await {
                Ok(t) => state::upsert_thread(t),
                Err(e) => toast_error(e),
            }
        });
    };
    let withdraw = move |path: String| {
        spawn(async move {
            match api::action::<RetractedMessage>(&path).await {
                Ok(r) => {
                    state::upsert_thread(r.thread.clone());
                    on_withdraw.call(r);
                }
                // Schon gesendet: nichts zurücklegen, die Liste aktualisiert sich live
                Err(e) if e.status == 404 => {}
                Err(e) => toast_error(e),
            }
        });
    };
    rsx! {
        div { class: "mb-2 flex flex-col gap-1",
            for q in items {
                div { key: "{q.id}", class: "flex items-center gap-2 rounded-lg border border-dashed bg-card/50 px-3 py-1.5 text-sm",
                    if q.interrupt {
                        span { class: "shrink-0 text-warning", title: tr("Zwischenruf – geht im nächsten passenden Moment raus"), icons::Zap { class: "size-3.5" } }
                    } else {
                        icons::Clock { class: "size-3.5 shrink-0 text-muted-foreground" }
                    }
                    span { class: "min-w-0 flex-1 truncate", title: "{q.text}", "{q.text}" }
                    if !q.attachments.is_empty() {
                        span { class: "shrink-0 text-xs text-muted-foreground", "📎 {q.attachments.len()}" }
                    }
                    button {
                        class: "flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                        title: if q.interrupt { tr("Sofort senden – nicht auf einen passenden Moment warten") } else { tr("Jetzt senden – unterbricht den Agenten") },
                        onclick: {
                            let p = format!("/threads/{thread_id}/queue/{}/now", q.id);
                            move |_| act(p.clone())
                        },
                        icons::Zap { class: "size-3.5" }
                        {tr("Jetzt")}
                    }
                    button {
                        class: "grid size-6 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                        title: tr("Zurückziehen und bearbeiten"),
                        onclick: {
                            let p = format!("/threads/{thread_id}/queue/{}/withdraw", q.id);
                            move |_| withdraw(p.clone())
                        },
                        icons::Undo2 { class: "size-3.5" }
                    }
                }
            }
        }
    }
}

/// Aus nrzX kopierte Nachricht (`application/x-nrzx`): Text und Anhänge.
fn pasted_nrzx(e: &ClipboardEvent) -> Option<(String, Vec<Attachment>)> {
    let ev = e.data().downcast::<web_sys::Event>().cloned()?;
    let ce = ev.dyn_into::<web_sys::ClipboardEvent>().ok()?;
    let raw = ce.clipboard_data()?.get_data("application/x-nrzx").ok().filter(|s| !s.is_empty())?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let text = v["text"].as_str().unwrap_or("").to_string();
    let atts: Vec<Attachment> = serde_json::from_value(v["attachments"].clone()).unwrap_or_default();
    Some((text, atts))
}

/// Dateien (Bilder) aus der Zwischenablage.
pub(super) fn pasted_files(e: &ClipboardEvent) -> Vec<web_sys::File> {
    let Some(ev) = e.data().downcast::<web_sys::Event>().cloned() else { return Vec::new() };
    let Ok(ce) = ev.dyn_into::<web_sys::ClipboardEvent>() else { return Vec::new() };
    util::files_of(ce.clipboard_data().and_then(|d| d.files()))
}

/// Auswahl „✳ Opus 5.5 Hoch“ (aufgebaut wie T3 Code): links die Agenten als Zeichen, in der
/// Mitte Modelle mit Suche (⌘/Strg+1…9 wählt direkt, Enter übernimmt ein eigenes Modell), rechts
/// die Denk-Stufe, unten Konto, eigener Worktree (`worktree` gesetzt) und Zugriff: „Voller
/// Zugriff“ ist FreeStyle – der Agent fragt nicht mehr nach (`access` gesetzt = anbieten).
#[component]
pub(crate) fn Picker(
    open: Signal<bool>,
    sel: Selection,
    agents: Vec<nrzx_shared::agents::AgentInfo>,
    onchange: EventHandler<Selection>,
    #[props(default)] access: Option<bool>,
    #[props(default)] onaccess: Option<EventHandler<bool>>,
    #[props(default)] worktree: Option<Signal<bool>>,
) -> Element {
    let mut query = use_signal(String::new);
    let mut cli_models = use_signal(|| None::<(AgentKind, Vec<ModelOption>)>);
    let mut loading = use_signal(|| false);
    let mut access_menu = use_signal(|| false);
    // Anmelden direkt aus dem Menü (Konto anlegen, bei Bedarf Anmeldung im CLI)
    let mut sign_in = use_signal(|| None::<nrzx_shared::agents::AgentInfo>);
    let mut sign_in_open = use_signal(|| false);
    let current = agents.iter().find(|a| a.kind == sel.agent).cloned();
    let missing: Vec<nrzx_shared::agents::AgentInfo> = agents.iter().filter(|a| a.accounts.is_empty()).cloned().collect();
    let mut start_sign_in = move |a: nrzx_shared::agents::AgentInfo| {
        sign_in.set(Some(a));
        sign_in_open.set(true);
    };
    let label = sel.label();
    let models: Vec<ModelOption> = match (cli_models(), &current) {
        (Some((k, list)), _) if k == sel.agent => list,
        (_, Some(c)) => c.models.clone(),
        _ => Vec::new(),
    };
    let q = query().trim().to_lowercase();
    let shown: Vec<ModelOption> = models
        .iter()
        .filter(|m| q.is_empty() || m.label.to_lowercase().contains(&q) || m.id.to_lowercase().contains(&q))
        .cloned()
        .collect();
    let custom = (!q.is_empty() && !models.iter().any(|m| m.id.eq_ignore_ascii_case(query().trim()))).then(|| query().trim().to_string());
    let can_list = matches!(sel.agent, AgentKind::Codex | AgentKind::Opencode | AgentKind::Kilo);
    let full = access == Some(true);
    let pick_model = {
        let sel = sel.clone();
        move |id: String| {
            let mut s = sel.clone();
            s.model = id;
            onchange.call(s);
            query.set(String::new());
        }
    };
    // Farben wie die Seitenleiste (Glas): gewählt leicht hell, sonst nur beim Überfahren
    let row = |on: bool| {
        if on {
            "flex w-full min-w-0 items-center gap-2 rounded-lg bg-foreground/[0.08] px-2 py-1.5 text-left text-sm text-foreground"
        } else {
            "flex w-full min-w-0 items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm text-foreground/85 hover:bg-foreground/[0.05] hover:text-foreground"
        }
    };
    rsx! {
        div { class: "relative",
            button {
                class: "flex h-8 max-w-[18rem] items-center gap-1.5 rounded-full px-2.5 text-sm text-muted-foreground hover:bg-accent hover:text-foreground",
                title: tr("Agent, Konto, Modell, Stufe und Zugriff"),
                onclick: move |_| open.toggle(),
                AgentIcon { kind: sel.agent, size: 16 }
                span { class: "truncate", "{label}" }
                // Eingeschaltete Optionen als kleine Zeichen (Voller Zugriff bewusst nicht – die Zeile bleibt ruhig)
                if worktree.is_some_and(|w| w()) {
                    span { title: tr("Eigener Worktree"), icons::GitBranch { class: "size-3.5 shrink-0 text-primary" } }
                }
            }
            if open() {
                div { class: "fixed inset-0 z-40", onclick: move |_| { open.set(false); access_menu.set(false); } }
                div {
                    class: "nrx-pop absolute right-0 bottom-full z-50 mb-2 flex w-[min(36rem,calc(100vw-2rem))] flex-col overflow-hidden",
                    "data-popup": "1",
                    onkeydown: {
                        let shown = shown.clone();
                        let mut pick_model = pick_model.clone();
                        move |e: KeyboardEvent| {
                            if e.key() == Key::Escape {
                                open.set(false);
                                return;
                            }
                            let m = e.modifiers();
                            if m.meta() || m.ctrl() {
                                if let Key::Character(c) = e.key() {
                                    if let Some(n) = c.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                                        if let Some(model) = shown.get(n - 1) {
                                            e.prevent_default();
                                            pick_model(model.id.clone());
                                        }
                                    }
                                }
                            }
                        }
                    },
                    div { class: "flex h-[min(20rem,calc(50dvh_-_13rem))] min-h-0",
                        // Agenten als Zeichen
                        div { class: "flex w-11 shrink-0 flex-col items-center gap-1 overflow-y-auto border-r border-white/[0.06] py-1.5",
                            for a in agents.iter().cloned() {
                                button {
                                    key: "{a.kind.key()}",
                                    class: if a.kind == sel.agent { "relative grid size-8 shrink-0 place-items-center rounded-lg bg-foreground/[0.08]" } else if a.installed && !a.accounts.is_empty() { "relative grid size-8 shrink-0 place-items-center rounded-lg hover:bg-foreground/[0.05]" } else { "relative grid size-8 shrink-0 place-items-center rounded-lg opacity-45 hover:bg-foreground/[0.05] hover:opacity-80" },
                                    title: if !a.installed { format!("{} – {}", a.name, tr("nicht installiert")) } else if a.accounts.is_empty() { format!("{} – {}", a.name, tr("kein Konto")) } else { a.name.clone() },
                                    onclick: {
                                        let sel = sel.clone();
                                        let agents = agents.clone();
                                        move |_| {
                                            query.set(String::new());
                                            onchange.call(sel.with_agent(a.kind, &agents))
                                        }
                                    },
                                    if a.kind == sel.agent {
                                        span { class: "absolute -right-[7px] top-1.5 bottom-1.5 w-0.5 rounded-full bg-primary" }
                                    }
                                    AgentIcon { kind: a.kind, size: 18 }
                                }
                            }
                        }
                        // Modelle mit Suche
                        div { class: "flex min-w-0 flex-1 flex-col",
                            div { class: "flex shrink-0 items-center gap-1.5 border-b border-white/[0.06] px-2.5",
                                icons::Search { class: "size-4 shrink-0 text-muted-foreground" }
                                input {
                                    class: "h-10 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground",
                                    placeholder: tr("Modelle suchen …"),
                                    autofocus: true,
                                    value: "{query}",
                                    oninput: move |e| query.set(e.value()),
                                    onkeydown: {
                                        let shown = shown.clone();
                                        let custom = custom.clone();
                                        let mut pick_model = pick_model.clone();
                                        move |e: KeyboardEvent| {
                                            if e.key() == Key::Enter {
                                                e.prevent_default();
                                                if let Some(m) = shown.first() {
                                                    pick_model(m.id.clone());
                                                } else if let Some(c) = custom.clone() {
                                                    pick_model(c);
                                                }
                                            }
                                        }
                                    },
                                }
                                if can_list {
                                    button {
                                        class: "grid size-6 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground",
                                        title: tr("Modelle vom CLI laden"),
                                        onclick: {
                                            let sel = sel.clone();
                                            move |_| {
                                                let kind = sel.agent;
                                                let acc = sel.account_id;
                                                loading.set(true);
                                                spawn(async move {
                                                    let url = match acc {
                                                        Some(a) => format!("/agents/{}/models?account={a}", kind.key()),
                                                        None => format!("/agents/{}/models", kind.key()),
                                                    };
                                                    match api::get::<ModelList>(&url).await {
                                                        Ok(l) => cli_models.set(Some((kind, l.models))),
                                                        Err(e) => toast_error(e),
                                                    }
                                                    loading.set(false);
                                                });
                                            }
                                        },
                                        if loading() {
                                            icons::LoaderCircle { class: "size-3.5 animate-spin" }
                                        } else {
                                            icons::RefreshCw { class: "size-3.5" }
                                        }
                                    }
                                }
                            }
                            div { class: "min-h-0 flex-1 overflow-y-auto p-1",
                                if let Some(c) = current.clone() {
                                    if c.accounts.is_empty() {
                                        div { class: "m-1 grid gap-2 rounded-xl border border-white/[0.06] bg-foreground/[0.03] p-3 text-sm",
                                            p { class: "text-muted-foreground", {trf("Für {0} ist noch kein Konto eingerichtet.", &[&c.name])} }
                                            button {
                                                class: "inline-flex w-fit items-center gap-1.5 rounded-lg bg-primary px-3 py-1.5 text-[13px] font-medium text-primary-foreground hover:bg-primary/90",
                                                onclick: {
                                                    let c = c.clone();
                                                    move |_| start_sign_in(c.clone())
                                                },
                                                icons::LogIn { class: "size-3.5" }
                                                {trf("Bei {0} anmelden", &[&c.name])}
                                            }
                                        }
                                    }
                                }
                                if q.is_empty() {
                                    button {
                                        class: row(sel.model.is_empty()),
                                        onclick: {
                                            let mut pick_model = pick_model.clone();
                                            move |_| pick_model(String::new())
                                        },
                                        AgentIcon { kind: sel.agent, size: 15 }
                                        span { class: "min-w-0 flex-1 truncate", {tr("Standard")} " " span { class: "text-xs text-muted-foreground", {tr("(Vorgabe des CLIs)")} } }
                                        if sel.model.is_empty() {
                                            icons::Check { class: "size-4 shrink-0" }
                                        }
                                    }
                                }
                                for (i , m) in shown.iter().cloned().enumerate() {
                                    button {
                                        key: "{m.id}",
                                        class: row(m.id == sel.model),
                                        title: "{m.id}",
                                        onclick: {
                                            let mut pick_model = pick_model.clone();
                                            let id = m.id.clone();
                                            move |_| pick_model(id.clone())
                                        },
                                        AgentIcon { kind: sel.agent, size: 15 }
                                        span { class: "min-w-0 flex-1 truncate", {tr(&m.label)} }
                                        if m.id == sel.model {
                                            icons::Check { class: "size-4 shrink-0" }
                                        }
                                        if i < 9 {
                                            span { class: "shrink-0 rounded bg-muted px-1.5 py-px font-mono text-[10.5px] text-muted-foreground", {crate::util::keys(&format!("⌘{}", i + 1))} }
                                        }
                                    }
                                }
                                if let Some(c) = custom.clone() {
                                    button {
                                        class: row(false),
                                        onclick: {
                                            let mut pick_model = pick_model.clone();
                                            move |_| pick_model(c.clone())
                                        },
                                        icons::Plus { class: "size-4 shrink-0 text-muted-foreground" }
                                        span { class: "min-w-0 flex-1 truncate", {trf("„{0}“ als eigenes Modell", &[&c])} }
                                        span { class: "shrink-0 text-[11px] text-muted-foreground", "Enter" }
                                    }
                                }
                            }
                        }
                        // Denk-Stufe
                        if let Some(c) = current.clone().filter(|c| !c.efforts.is_empty()) {
                            div { class: "flex w-36 shrink-0 flex-col border-l border-white/[0.06]",
                                div { class: "flex h-10 shrink-0 items-center gap-2 border-b border-white/[0.06] px-3 text-xs",
                                    span { class: "flex-1 truncate font-medium text-muted-foreground", {tr("Denk-Stufe")} }
                                }
                                div { class: "min-h-0 flex-1 overflow-y-auto p-1",
                                    for e in std::iter::once(String::new()).chain(c.efforts.iter().cloned()) {
                                        button {
                                            key: "{e}",
                                            class: row(e == sel.effort),
                                            onclick: {
                                                let mut s = sel.clone();
                                                let e = e.clone();
                                                move |_| {
                                                    s.effort = e.clone();
                                                    onchange.call(s.clone());
                                                }
                                            },
                                            span { class: "flex-1 truncate", if e.is_empty() { {tr("Standard")} } else { {effort_label(&e)} } }
                                            if e == sel.effort {
                                                icons::Check { class: "size-4 shrink-0" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Weitere Agenten anmelden – ohne Umweg über die Einstellungen
                    if !missing.is_empty() {
                        div { class: "flex shrink-0 flex-wrap items-center gap-1.5 border-t border-white/[0.06] px-2.5 py-2 text-xs",
                            span { class: "mr-1 text-muted-foreground", {tr("Weitere Agenten anmelden:")} }
                            for a in missing.iter().cloned() {
                                button {
                                    key: "{a.kind.key()}",
                                    class: "inline-flex items-center gap-1.5 rounded-full border border-white/10 px-2.5 py-1 text-foreground/85 hover:bg-foreground/[0.06] hover:text-foreground",
                                    title: if a.installed { trf("{0} anmelden", &[&a.name]) } else { trf("{0} anmelden (das CLI wird bei Bedarf installiert)", &[&a.name]) },
                                    onclick: {
                                        let a = a.clone();
                                        move |_| start_sign_in(a.clone())
                                    },
                                    AgentIcon { kind: a.kind, size: 13 }
                                    "{a.name}"
                                }
                            }
                        }
                    }
                    // Unten: Konto, Worktree, Zugriff
                    div { class: "flex shrink-0 items-center gap-1 border-t border-white/[0.06] px-2 py-1.5 text-xs text-muted-foreground",
                        if let Some(c) = current.clone().filter(|c| !c.accounts.is_empty()) {
                            label { class: "flex min-w-0 items-center gap-1.5 rounded-md px-1.5 py-1 hover:bg-accent/60 hover:text-foreground", title: tr("Konto"),
                                icons::User { class: "size-3.5 shrink-0" }
                                select {
                                    class: "min-w-0 max-w-[10rem] cursor-pointer truncate bg-transparent outline-none",
                                    onchange: {
                                        let sel = sel.clone();
                                        move |e: FormEvent| {
                                            let mut s = sel.clone();
                                            s.account_id = e.value().parse::<i64>().ok();
                                            onchange.call(s);
                                        }
                                    },
                                    for acc in c.accounts.iter().cloned() {
                                        option { key: "{acc.id}", value: "{acc.id}", selected: Some(acc.id) == sel.account_id,
                                            {if acc.status == "ok" { acc.label.clone() } else { format!("{} ⚠", acc.label) }}
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(c) = current.clone().filter(|c| !c.accounts.is_empty()) {
                            button {
                                class: "flex items-center gap-1 rounded-md px-1.5 py-1 hover:bg-foreground/[0.05] hover:text-foreground",
                                title: trf("Weiteres Konto bei {0} anmelden", &[&c.name]),
                                onclick: {
                                    let c = c.clone();
                                    move |_| start_sign_in(c.clone())
                                },
                                icons::Plus { class: "size-3.5" }
                                {tr("Konto")}
                            }
                        }
                        button {
                            class: "grid size-6 shrink-0 place-items-center rounded-md hover:bg-foreground/[0.05] hover:text-foreground",
                            title: tr("Konten verwalten"),
                            onclick: move |_| {
                                open.set(false);
                                util::storage_set("nrz-settings-section", "agents");
                                state::open_settings();
                            },
                            icons::Settings2 { class: "size-3.5" }
                        }
                        div { class: "flex-1" }
                        if let Some(mut w) = worktree {
                            button {
                                class: if w() { "flex items-center gap-1.5 rounded-md bg-accent/70 px-2 py-1 text-foreground" } else { "flex items-center gap-1.5 rounded-md px-2 py-1 hover:bg-accent/60 hover:text-foreground" },
                                title: tr("Eigener Branch und Ordner statt direkt auf dem Standard-Branch"),
                                onclick: move |_| w.toggle(),
                                icons::GitBranch { class: "size-3.5" }
                                {tr("Worktree")}
                            }
                        }
                        if let (Some(on), Some(set)) = (access, onaccess) {
                            div { class: "relative",
                                button {
                                    class: if full { "flex items-center gap-1.5 rounded-md px-2 py-1 text-warning hover:bg-accent/60" } else { "flex items-center gap-1.5 rounded-md px-2 py-1 hover:bg-accent/60 hover:text-foreground" },
                                    title: tr("Zugriff: ob der Agent nachfragt"),
                                    onclick: move |_| access_menu.toggle(),
                                    if on { icons::ShieldOff { class: "size-3.5" } } else { icons::ShieldCheck { class: "size-3.5" } }
                                    if on { {tr("Voller Zugriff")} } else { {tr("Fragt nach")} }
                                    icons::ChevronUp { class: "size-3.5" }
                                }
                                if access_menu() {
                                    div { class: "nrx-pop absolute right-0 bottom-full z-10 mb-1.5 grid w-64 gap-0.5 p-1.5",
                                        for (val , title , detail) in [(false, tr("Fragt nach"), tr("Rückfragen und Freigaben wie gewohnt")), (true, tr("Voller Zugriff"), tr("Der Agent entscheidet alles selbst und fragt nicht mehr nach"))] {
                                            button {
                                                key: "{val}",
                                                class: if val == on { "flex items-start gap-2 rounded-md bg-accent/70 px-2 py-1.5 text-left" } else { "flex items-start gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent/60" },
                                                onclick: move |_| {
                                                    access_menu.set(false);
                                                    if val != on {
                                                        set.call(val);
                                                    }
                                                },
                                                span { class: if val { "mt-0.5 text-warning" } else { "mt-0.5 text-muted-foreground" },
                                                    if val { icons::ShieldOff { class: "size-4" } } else { icons::ShieldCheck { class: "size-4" } }
                                                }
                                                span { class: "min-w-0 flex-1",
                                                    span { class: "block text-sm text-foreground", "{title}" }
                                                    span { class: "block text-[11px] leading-snug text-muted-foreground", "{detail}" }
                                                }
                                                if val == on {
                                                    icons::Check { class: "mt-0.5 size-4 shrink-0 text-foreground" }
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
            if let Some(a) = sign_in() {
                super::accounts::SignIn { key: "{a.kind.key()}", open: sign_in_open, info: a }
            }
        }
    }
}

/// Schlüssel im Browser-Speicher: FreeStyle für neue Chats.
pub const FREESTYLE_KEY: &str = "nrz-freestyle";


#[derive(Clone, PartialEq)]
struct Draft {
    title: String,
    prompt: String,
    agent: AgentKind,
}

/// Dialog „Subagenten starten“: mehrere Aufgaben, je Agent wählbar.
#[component]
pub fn SubagentsDialog(open: Signal<bool>, thread_id: Option<i64>, project_id: Option<i64>, sel: Selection) -> Element {
    let nav = use_navigator();
    let agent = sel.agent;
    let mut drafts = use_signal(move || vec![Draft { title: String::new(), prompt: String::new(), agent }, Draft { title: String::new(), prompt: String::new(), agent }]);
    let mut busy = use_signal(|| false);
    let agents = AGENTS.read().clone().unwrap_or_default();
    let usable: Vec<AgentKind> = agents.iter().filter(|a| !a.accounts.is_empty()).map(|a| a.kind).collect();
    let submit = {
        let sel = sel.clone();
        move |e: FormEvent| {
            e.prevent_default();
            let tasks: Vec<SubagentTask> = drafts()
                .into_iter()
                .filter(|d| !d.prompt.trim().is_empty())
                .map(|d| SubagentTask { title: d.title.trim().to_string(), prompt: d.prompt.trim().to_string(), agent: Some(d.agent), ..Default::default() })
                .collect();
            if tasks.is_empty() {
                toast_error(tr("Bitte mindestens eine Aufgabe beschreiben."));
                return;
            }
            busy.set(true);
            let sel = sel.clone();
            spawn(async move {
                let result = match thread_id {
                    Some(id) => api::post::<Vec<ThreadSummary>, _>(&format!("/threads/{id}/subagents"), &NewSubagents { tasks }).await.map(|kids| (id, kids)),
                    None => {
                        let req = NewThread {
                            project_id,
                            agent: sel.agent,
                            account_id: sel.account_id,
                            model: sel.model.clone(),
                            effort: sel.effort.clone(),
                            tasks,
                            ..Default::default()
                        };
                        api::post::<ThreadSummary, _>("/threads", &req).await.map(|t| (t.id, vec![t]))
                    }
                };
                match result {
                    Ok((id, list)) => {
                        for t in list {
                            state::upsert_thread(t);
                        }
                        open.set(false);
                        drafts.set(vec![Draft { title: String::new(), prompt: String::new(), agent: sel.agent }]);
                        if thread_id.is_none() {
                            nav.push(Route::ThreadPage { id });
                        }
                    }
                    Err(e) => toast_error(e),
                }
                busy.set(false);
            });
        }
    };
    rsx! {
        Modal {
            open,
            title: tr("Subagenten starten"),
            description: tr("Jede Aufgabe läuft als eigener Thread mit eigenem Worktree, alle parallel. Beschreibe jede Aufgabe vollständig – Subagenten sehen dieses Gespräch nicht."),
            class: "max-w-2xl",
            form { class: "flex flex-col gap-4", onsubmit: submit,
                for (i , d) in drafts().into_iter().enumerate() {
                    div { key: "{i}", class: "flex flex-col gap-2 rounded-xl border p-3",
                        div { class: "flex items-center gap-2",
                            input {
                                class: "{INPUT}",
                                placeholder: trf("Titel der Aufgabe {0}", &[&(i + 1)]),
                                value: "{d.title}",
                                oninput: move |e| drafts.write()[i].title = e.value(),
                            }
                            select {
                                class: "{SELECT} w-40",
                                value: "{d.agent.key()}",
                                onchange: move |e| {
                                    if let Some(k) = AgentKind::parse(&e.value()) {
                                        drafts.write()[i].agent = k;
                                    }
                                },
                                for k in AgentKind::ALL {
                                    option { value: "{k.key()}", disabled: !usable.contains(&k), selected: k == d.agent, "{k.name()}" }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "{btn(BTN_GHOST)} {BTN_ICON}",
                                title: tr("Entfernen"),
                                onclick: move |_| {
                                    drafts.write().remove(i);
                                },
                                icons::X { class: "size-4" }
                            }
                        }
                        textarea {
                            class: "{TEXTAREA}",
                            rows: "3",
                            placeholder: tr("Was soll dieser Subagent tun?"),
                            value: "{d.prompt}",
                            oninput: move |e| drafts.write()[i].prompt = e.value(),
                        }
                    }
                }
                div { class: "flex items-center justify-between gap-2",
                    button {
                        r#type: "button",
                        class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                        disabled: drafts().len() >= 10,
                        onclick: move |_| drafts.write().push(Draft { title: String::new(), prompt: String::new(), agent }),
                        icons::Plus { class: "size-4" }
                        {tr("Aufgabe")}
                    }
                    button { r#type: "submit", class: "{btn(BTN_PRIMARY)}", disabled: busy(),
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        }
                        {tr("Starten")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{token_at, REPO};

    #[test]
    fn tokens() {
        assert_eq!(token_at("schau @src/ma", 13), Some(('@', 6, "src/ma".to_string())));
        assert_eq!(token_at("integriere @@nrz", 16), Some((REPO, 11, "nrz".to_string())));
        assert_eq!(token_at("mit @@", 6), Some((REPO, 4, String::new())));
        assert_eq!(token_at("/mod", 4), Some(('/', 0, "mod".to_string())));
        assert_eq!(token_at("a /mod", 6), None);
        assert_eq!(token_at("über $rev", 10), Some(('$', 6, "rev".to_string())));
        assert_eq!(token_at("nichts", 6), None);
    }
}
