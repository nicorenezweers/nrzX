#![cfg_attr(windows, windows_subsystem = "windows")]
//! nrzX als native App für macOS und Windows – komplett in Rust.
//! (Windows: WebView2 statt WKWebView, Menü im Fenster, Rechte-Abschnitt und Ampel nur unter macOS.)
//!
//! * Der nrzX-Server läuft im selben Prozess auf `127.0.0.1` (Daten in
//!   `~/Library/Application Support/nrzX`, Protokoll in `~/Library/Logs/nrzX`).
//! * Kein Login: Die App meldet sich mit einem Token an, das nur sie kennt (`/__desktop/login`).
//!   einstellbar unter nrzX → Einstellungen … (⌘,).
//! * Dort lassen sich auch die Rechte von macOS (Ordner, Mikrofon, Automation, Bedienungshilfen …)
//!   vorab erteilen, damit kein Dialog mitten in einem Agenten-Lauf aufgeht (siehe `permissions`).
//!   Beim Start fragt die App fehlende Ordnerrechte selbst an; verweigert macOS später einen Zugriff
//!   (Server meldet „Kein Zugriff: macOS …“), öffnen sich die Rechte (`perm:hint`).
//! * Die Oberfläche läuft in einem nativen Fenster (WKWebView über `wry`, Fenster über `tao`),
//!   mit macOS-Menüleiste (`muda`): Bearbeiten (Kopieren/Einfügen), Ansicht, Fenster, Hilfe.
//! * Updates: Die App fragt die Releases in nrzGit ab (`macos-<version>`), zeigt einen Hinweis mit
//!   Knopf, lädt die DMG, ersetzt das Bundle und startet neu (siehe `updater`).

#[cfg(windows)]
mod glass;
#[cfg(target_os = "macos")]
mod lights;
mod overlay;
#[cfg(target_os = "macos")]
mod permissions;
mod server;
#[cfg(target_os = "macos")]
mod updater;
#[cfg(windows)]
mod webview2_loader;

use muda::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use updater::{Installed, Update};
use wry::{BackgroundThrottlingPolicy, NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder};


#[derive(Debug)]
enum UserEvent {
    Menu(MenuEvent),
    ServerReady(Result<u16, String>),
    /// Ergebnis einer Update-Prüfung; `manual` = über das Menü bzw. die Oberfläche angestossen,
    /// `quiet` = ohne Dialog (Knopf in der Seitenleiste – das Ergebnis zeigt die Oberfläche).
    UpdateChecked { result: Result<Option<Update>, String>, manual: bool, quiet: bool },
    Progress(f64),
    Downloaded(Result<PathBuf, String>),
    /// Nachricht aus einer Seite; `trusted` = von der eigenen Oberfläche im Hauptfenster.
    Ipc { msg: String, trusted: bool, window: WindowId },
    PageLoaded(WindowId),
    OpenWindow(String),
    /// Einstellungen im eigenen Fenster öffnen (Menü „Einstellungen …“, ⌘,).
    OpenSettings,
    /// Das Hauptfenster hat eine fremde Seite geladen (Link ohne `target`, Weiterleitung …):
    /// im Browser des Systems öffnen und die Oberfläche zurückholen.
    Escaped(String),
    /// Rechte fehlen (beim Start verweigerte Ordner oder ein verweigerter Zugriff): Hinweis mit
    /// Weg in die Systemeinstellungen.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    PermissionHint,
    /// Der Inhaltsprozess von WebKit ist beendet worden (Absturz, Speicherdruck): das Fenster wäre
    /// sonst leer – die Seite neu laden.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    WebContentTerminated(WindowId),
}

/// Zustand des Updaters.
#[derive(Default)]
struct UpdateState {
    available: Option<Update>,
    busy: bool,
}

struct MenuIds {
    settings: MenuItem,
    check: MenuItem,
    install: MenuItem,
    quit: MenuItem,
}

#[derive(serde::Deserialize)]
struct NotifyMsg {
    title: String,
    #[serde(default)]
    body: String,
}

/// Lage der Ampel an die Oberfläche (`--nrz-tl-y` Mitte, `--nrz-tl-r` rechter Rand): die Reihe
/// oben richtet sich danach.
#[cfg(target_os = "macos")]
fn send_lights(window: &Window, wv: &WebView) {
    if let Some((y, r)) = lights::spot(window) {
        let _ = wv.evaluate_script(&format!(
            "document.documentElement.style.setProperty('--nrz-tl-y','{y:.1}px');document.documentElement.style.setProperty('--nrz-tl-r','{r:.1}px')"
        ));
    }
}

fn main() {
    let log_guard = init_logging();

    // Nur eine Instanz: zwei Server auf derselben Datenbank vertragen sich nicht.
    let _lock_file = match single_instance_lock() {
        Ok(l) => l,
        Err(e) => {
            message("nrzX", &e, rfd::MessageLevel::Info);
            return;
        }
    };

    updater::cleanup();

    // WebView2Loader.dll steckt im Programm: vor dem ersten WebView auspacken und laden
    #[cfg(windows)]
    if let Err(e) = webview2_loader::load() {
        tracing::error!("WebView2-Lader: {e}");
        message("nrzX", &format!("WebView2 kann nicht gestartet werden:\n{e}"), rfd::MessageLevel::Error);
        return;
    }

    // Umgebung (PATH, Ports, Datenordner, Desktop-Token) vor allen weiteren Threads setzen
    let ports = server::prepare_env();

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    {
        let p = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = p.send_event(UserEvent::Menu(e));
        }));
    }

    let (menu, ids) = build_menu();

    // Titelleiste in die Oberfläche integriert: transparent, Inhalt bis oben. Die Ampel setzt
    // Die Ampel setzt eine leere native Toolbar tiefer (`lights`); die Oberfläche meldet „drag“.
    let builder = WindowBuilder::new()
        .with_title("nrzX")
        .with_inner_size(tao::dpi::LogicalSize::new(1440.0, 920.0))
        // Schmaler als 768 px zeigt die Oberfläche das Handy-Layout – in der App nie
        .with_min_inner_size(tao::dpi::LogicalSize::new(820.0, 520.0))
        // Durchsichtig: dahinter liegt das Milchglas des Systems (`apply_glass`)
        .with_transparent(true);
    #[cfg(target_os = "macos")]
    let builder = {
        use tao::platform::macos::WindowBuilderExtMacOS;
        builder
            .with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_fullsize_content_view(true)
    };
    // Windows: wie am Mac ohne Titelleiste und Menü – die Oberfläche reicht bis oben, rechts oben
    // zeichnet sie Minimieren/Maximieren/Schliessen (`overlay::WIN_SCRIPT`). Der Schatten des
    // Systems bleibt (und mit ihm die Ränder zum Ziehen links, rechts und unten).
    #[cfg(windows)]
    let builder = {
        use tao::platform::windows::WindowBuilderExtWindows;
        builder.with_decorations(false).with_undecorated_shadow(true)
    };
    let window = builder.build(&event_loop).expect("Fenster");
    #[cfg(target_os = "macos")]
    lights::install(&window, false);
    let main_id = window.id();
    // Milchglas hinter der Oberfläche (davon hängen Durchsichtigkeit und `GLASS_SCRIPT` des WebViews
    // ab); ohne zeichnet die Oberfläche deckend. Windows: am Fenster, schon vor dem WebView.
    #[cfg(windows)]
    let glass = apply_glass(&window);
    #[cfg(not(windows))]
    let glass = cfg!(target_os = "macos");
    GLASS.store(glass, Ordering::Relaxed);

    let server_port: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let webview = build_webview(&window, &proxy, server_port.clone(), true, false, None).expect("WebView");
    // macOS: erst jetzt – wry setzt eine eigene Inhaltsansicht ein, das Milchglas gehört unter den
    // WebView darin (sonst wäre das Fenster nur durchsichtig, nicht weichgezeichnet)
    #[cfg(target_os = "macos")]
    if glass && !apply_glass(&window) {
        // Dann deckend: die Oberfläche meldet beim Laden ihr Thema (`theme:`), die Antwort schaltet
        // das Milchglas dort ab
        GLASS.store(false, Ordering::Relaxed);
    }

    // Server starten (blockiert nicht das Fenster)
    let server: Arc<Mutex<Option<server::Server>>> = Arc::new(Mutex::new(None));
    match ports {
        Ok((port, preview)) => {
            tracing::info!("Server auf 127.0.0.1:{port}, Vorschau auf {preview}");
            let p = proxy.clone();
            let slot = server.clone();
            std::thread::spawn(move || {
                let r = server::start(port).map(|s| {
                    let port = s.port;
                    *slot.lock().unwrap() = Some(s);
                    port
                });
                let _ = p.send_event(UserEvent::ServerReady(r));
            });
        }
        Err(e) => {
            let _ = proxy.send_event(UserEvent::ServerReady(Err(e)));
        }
    }

    // Updater: eigene kleine Laufzeit
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("Tokio");
    {
        let p = proxy.clone();
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                let result = updater::check(&updater::current_version()).await;
                if let Err(e) = &result {
                    tracing::warn!("Update-Prüfung: {e}");
                }
                let _ = p.send_event(UserEvent::UpdateChecked { result, manual: false, quiet: false });
                tokio::time::sleep(updater::CHECK_INTERVAL).await;
            }
        });
    }

    let mut updates = UpdateState::default();
    // Letzter automatisch gezeigter Hinweis auf fehlende Rechte (nicht öfter als einmal je Minute)
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut, unused_variables))]
    let mut perm_hint_at: Option<std::time::Instant> = None;
    let mut extra: HashMap<WindowId, (Window, WebView)> = HashMap::new();
    let mut main_window = Some((window, webview));
    // Einstellungen im eigenen Fenster (höchstens eins)
    let mut settings_win: Option<(Window, WebView)> = None;
    let _log_guard = log_guard;

    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let quit = |control_flow: &mut ControlFlow| {
            if let Some(s) = server.lock().unwrap().as_mut() {
                s.stop(Duration::from_secs(4));
            }
            *control_flow = ControlFlow::Exit;
        };
        let port = *server_port.lock().unwrap();
        match event {
            Event::NewEvents(StartCause::Init) => {
                #[cfg(target_os = "macos")]
                menu.init_for_nsapp();
                // Windows: kein Menü
                #[cfg(windows)]
                let _ = &menu;
            }
            Event::Reopen { .. } => {
                if let Some((w, _)) = &main_window {
                    w.set_visible(true);
                    w.set_focus();
                }
            }
            #[cfg(windows)]
            Event::WindowEvent { window_id, event: WindowEvent::Resized(_), .. } if window_id == main_id => {
                if let Some((w, wv)) = &main_window {
                    send_window_state(w, wv);
                }
            }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                if window_id == main_id {
                    if cfg!(target_os = "macos") {
                        // Wie bei macOS üblich: Fenster schliessen, App (und Agenten) laufen weiter
                        if let Some((w, _)) = &main_window {
                            w.set_visible(false);
                        }
                    } else {
                        // Windows: ohne Dock – Fenster schliessen beendet die App
                        quit(control_flow);
                    }
                } else if settings_win.as_ref().is_some_and(|(w, _)| w.id() == window_id) {
                    settings_win = None;
                } else {
                    extra.remove(&window_id);
                }
            }
            Event::UserEvent(ev) => match ev {
                UserEvent::ServerReady(Ok(p)) => {
                    *server_port.lock().unwrap() = Some(p);
                    // Fehlende Ordnerrechte jetzt anfragen (Dialoge von macOS), nicht erst mitten in
                    // einem Agenten-Lauf; verweigerte zeigt die App einmal je Version
                    #[cfg(target_os = "macos")]
                    {
                        let p = proxy.clone();
                        std::thread::spawn(move || {
                            let denied = permissions::ask_missing_folders();
                            if !denied.is_empty() && permissions::hint_due() {
                                let _ = p.send_event(UserEvent::PermissionHint);
                            }
                        });
                    }
                    if let Some((w, wv)) = &main_window {
                        let _ = wv.load_url(&server::login_url(p));
                        w.set_focus();
                    }
                }
                UserEvent::ServerReady(Err(e)) => {
                    tracing::error!("Server: {e}");
                    if let Some((_, wv)) = &main_window {
                        let _ = wv.load_html(&overlay::loading_page(Some(&e)));
                    }
                }
                UserEvent::PageLoaded(id) => {
                    if id == main_id {
                        if let Some((_w, wv)) = &main_window {
                            #[cfg(target_os = "macos")]
                            send_lights(_w, wv);
                            #[cfg(windows)]
                            send_window_state(_w, wv);
                            // Ein schon gefundenes Update holt sich die Oberfläche selbst
                            // (`update:state`), sobald ihr Knopf in der Seitenleiste bereit ist
                        }
                    }
                }
                UserEvent::PermissionHint => {
                    #[cfg(target_os = "macos")]
                    if perm_hint_at.map(|t: std::time::Instant| t.elapsed() > Duration::from_secs(60)).unwrap_or(true) {
                        perm_hint_at = Some(std::time::Instant::now());
                        permissions::show_hint();
                    }
                }
                UserEvent::WebContentTerminated(id) => {
                    tracing::warn!("Inhaltsprozess der Oberfläche beendet – lade neu");
                    let all = main_window.iter().chain(settings_win.iter()).chain(extra.values());
                    if let Some((_, wv)) = all.into_iter().find(|(w, _)| w.id() == id) {
                        let _ = wv.reload();
                    }
                }
                UserEvent::UpdateChecked { result, manual, quiet } => {
                    // Knopf unten in der Seitenleiste: ein gefundenes Update melden (nach einer
                    // Prüfung ohne Dialog auch „aktuell“ bzw. den Fehler); lädt es schon, bleibt
                    // der Knopf beim Fortschritt
                    if let Some((_, wv)) = &main_window {
                        let state = match &result {
                            Ok(Some(_)) if updates.busy => None,
                            Ok(Some(u)) => Some(("available", u.version.to_string())),
                            Ok(None) if manual => Some(("current", updater::current_version().to_string())),
                            Err(e) if manual => Some(("error", e.clone())),
                            _ => None,
                        };
                        if let Some((status, text)) = state {
                            update_ui(wv, status, &text);
                        }
                    }
                    match result {
                        Ok(Some(u)) => {
                            ids.install.set_text(format!("Update auf {} installieren …", u.version));
                            ids.install.set_enabled(true);
                            // Kein Hinweis-Kasten mehr – nur der Knopf in der Seitenleiste (oben
                            // gemeldet). Über das Menü gesucht: Fenster nach vorn, damit man ihn sieht
                            if manual && !quiet {
                                if let Some((w, _)) = &main_window {
                                    w.set_visible(true);
                                    w.set_focus();
                                }
                            }
                            updates.available = Some(u);
                            // NRZX_AUTO_UPDATE=1: ohne Rückfrage laden und installieren
                            if std::env::var("NRZX_AUTO_UPDATE").is_ok_and(|v| v == "1") {
                                start_download(&rt, &proxy, &mut updates, &main_window);
                            }
                        }
                        Ok(None) => {
                            if manual && !quiet {
                                message(
                                    "Keine Updates",
                                    &format!("nrzX {} ist die neueste Version.", updater::current_version()),
                                    rfd::MessageLevel::Info,
                                );
                            }
                        }
                        Err(e) => {
                            if manual && !quiet {
                                message("Update-Prüfung fehlgeschlagen", &e, rfd::MessageLevel::Warning);
                            }
                        }
                    }
                }
                UserEvent::Ipc { msg, trusted, window } => {
                    if !trusted {
                        tracing::warn!("Nachricht aus fremder Seite ignoriert");
                    } else if msg == "settings:open" {
                        let _ = proxy.send_event(UserEvent::OpenSettings);
                    } else if msg == "settings:close" {
                        settings_win = None;
                    } else if msg == "window:drag" {
                        // Das Fenster, aus dem die Nachricht kommt (Hauptfenster oder Einstellungen)
                        if let Some(w) = window_of(window, &main_window, &settings_win) {
                            let _ = w.drag_window();
                        }
                    } else if let Some(url) = msg.strip_prefix("open-external:") {
                        // Integrierter Browser: „Im Browser öffnen“ – Adressen und lokale Dateien
                        // im Browser des Systems (nur aus der eigenen Oberfläche, nie Programme)
                        let lower = url.to_ascii_lowercase();
                        let ok = lower.starts_with("http://") || lower.starts_with("https://") || (lower.starts_with("file://") && (lower.ends_with(".html") || lower.ends_with(".htm")));
                        if ok {
                            open_external(url);
                        }
                    } else if msg == "window:zoom" {
                        if let Some(w) = window_of(window, &main_window, &settings_win) {
                            w.set_maximized(!w.is_maximized());
                        }
                    } else if msg == "window:fullscreen" {
                        // Vollbild umschalten
                        if let Some((w, _wv)) = &main_window {
                            let full = w.fullscreen().is_some();
                            w.set_fullscreen(if full { None } else { Some(tao::window::Fullscreen::Borderless(None)) });
                            #[cfg(windows)]
                            send_window_state(w, _wv);
                        }
                    } else if msg == "window:minimize" {
                        if let Some((w, _)) = &main_window {
                            w.set_minimized(true);
                        }
                    } else if msg == "window:close" {
                        // Wie der Schliessen-Knopf des Systems
                        if cfg!(target_os = "macos") {
                            if let Some((w, _)) = &main_window {
                                w.set_visible(false);
                            }
                        } else {
                            quit(control_flow);
                        }
                    } else if let Some(dir) = msg.strip_prefix("window:resize:") {
                        // Rahmenloses Fenster (Windows): am oberen Rand ziehen ändert die Grösse
                        if let (Some((w, _)), Some(d)) = (&main_window, resize_direction(dir)) {
                            let _ = w.drag_resize_window(d);
                        }
                    } else if msg == "update:check" {
                        let p = proxy.clone();
                        rt.spawn(async move {
                            let result = updater::check(&updater::current_version()).await;
                            let _ = p.send_event(UserEvent::UpdateChecked { result, manual: true, quiet: false });
                        });
                    } else if msg == "app:data" {
                        open_external(&server::data_dir().to_string_lossy());
                    } else if msg == "app:logs" {
                        open_external(&server::log_dir().join("nrzx.log").to_string_lossy());
                    } else if let Some(json) = msg.strip_prefix("notify:") {
                        if let Ok(n) = serde_json::from_str::<NotifyMsg>(json) {
                            native_notify(&n.title, &n.body);
                        }
                    } else if msg == "perm:hint" {
                        let _ = proxy.send_event(UserEvent::PermissionHint);
                    } else if let Some(theme) = msg.strip_prefix("theme:") {
                        set_window_theme(&main_window, theme);
                        if !GLASS.load(Ordering::Relaxed) {
                            if let Some((_, wv)) = &main_window {
                                let _ = wv.evaluate_script(overlay::NO_GLASS_SCRIPT);
                            }
                        }
                    } else {
                        match msg.as_str() {
                            "update:install" => start_download(&rt, &proxy, &mut updates, &main_window),
                            "update:check:quiet" => {
                                let p = proxy.clone();
                                rt.spawn(async move {
                                    let result = updater::check(&updater::current_version()).await;
                                    let _ = p.send_event(UserEvent::UpdateChecked { result, manual: true, quiet: true });
                                });
                            }
                            // Oberfläche (neu) geladen: ein schon gefundenes Update zeigen (lädt es
                            // gerade, kommt gleich der nächste Fortschritt)
                            "update:state" => {
                                if let (Some((_, wv)), Some(u), false) = (&main_window, &updates.available, updates.busy) {
                                    update_ui(wv, "available", &u.version.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                UserEvent::Progress(p) => {
                    update_main(&main_window, "progress", &p.to_string());
                }
                UserEvent::Downloaded(Err(e)) => {
                    updates.busy = false;
                    tracing::error!("Update: {e}");
                    update_main(&main_window, "failed", &e);
                }
                UserEvent::Downloaded(Ok(dmg)) => {
                    update_main(&main_window, "installing", "");
                    match updater::install(&dmg) {
                        Ok(Installed::Replaced(bundle)) => {
                            tracing::info!("Update installiert, Neustart");
                            updater::relaunch_after_exit(&bundle);
                            quit(control_flow);
                        }
                        Ok(Installed::OpenedDmg(text)) => {
                            updates.busy = false;
                            if let Some(u) = &updates.available {
                                update_main(&main_window, "available", &u.version.to_string());
                            }
                            // Von Hand zu Ende installieren: Hinweis des Systems statt Kasten
                            message("Update bereit", &text, rfd::MessageLevel::Info);
                        }
                        Err(e) => {
                            updates.busy = false;
                            update_main(&main_window, "failed", &e);
                        }
                    }
                }
                UserEvent::Escaped(url) => {
                    tracing::warn!("Hauptfenster verliess die App: {url} – im Browser geöffnet");
                    open_external(&url);
                    if let (Some(p), Some((_, wv))) = (port, &main_window) {
                        let _ = wv.load_url(&server::login_url(p));
                    }
                }
                UserEvent::OpenWindow(url) => {
                    open_window(target, &proxy, &server_port, &mut extra, &url);
                }
                UserEvent::OpenSettings => {
                    // Eigenes Fenster wie das Hauptfenster (Milchglas, Ampel in der Oberfläche)
                    if settings_win.is_none() {
                        if let Some(p) = port {
                            settings_win = open_settings_window(target, &proxy, &server_port, p);
                        }
                    }
                    if let Some((w, _)) = &settings_win {
                        w.set_visible(true);
                        w.set_focus();
                    }
                }
                UserEvent::Menu(e) => {
                    let id = e.id();
                    if id == ids.quit.id() {
                        quit(control_flow);
                    } else if id == ids.settings.id() {
                        let _ = proxy.send_event(UserEvent::OpenSettings);
                    } else if id == ids.check.id() {
                        let p = proxy.clone();
                        rt.spawn(async move {
                            let result = updater::check(&updater::current_version()).await;
                            let _ = p.send_event(UserEvent::UpdateChecked { result, manual: true, quiet: false });
                        });
                    } else if id == ids.install.id() {
                        start_download(&rt, &proxy, &mut updates, &main_window);
                    }
                }
            },
            Event::LoopDestroyed => {
                main_window.take();
                if let Some(s) = server.lock().unwrap().as_mut() {
                    s.stop(Duration::from_secs(4));
                }
            }
            _ => {}
        }
    });
}

fn build_webview(
    window: &Window,
    proxy: &EventLoopProxy<UserEvent>,
    server_port: Arc<Mutex<Option<u16>>>,
    is_main: bool,
    settings: bool,
    url: Option<&str>,
) -> wry::Result<WebView> {
    let id = window.id();
    let p_ipc = proxy.clone();
    let p_load = proxy.clone();
    let p_new = proxy.clone();
    let port_ipc = server_port.clone();
    let port_load = server_port.clone();
    // Eigene Oberfläche: Hauptfenster und das Fenster der Einstellungen
    let ui = is_main || settings;
    // Entwicklerwerkzeuge nur im Debug-Build oder mit NRZX_DEVTOOLS=1
    let devtools = cfg!(debug_assertions) || std::env::var("NRZX_DEVTOOLS").is_ok_and(|v| v == "1");
    let glass = GLASS.load(Ordering::Relaxed);
    let builder = WebViewBuilder::new()
        .with_initialization_script_for_main_only(if ui && cfg!(target_os = "macos") { overlay::WINDOW_SCRIPT } else { "" }, true)
        .with_initialization_script_for_main_only(if ui && cfg!(windows) { overlay::WIN_SCRIPT } else { "" }, true)
        .with_initialization_script_for_main_only(if ui { overlay::NATIVE_SCRIPT } else { "" }, true)
        .with_initialization_script_for_main_only(if ui && glass { overlay::GLASS_SCRIPT } else { "" }, true)
        // Mit Milchglas durchsichtig – die Oberfläche tönt es nur
        .with_transparent(ui && glass)
        .with_ipc_handler(move |req| {
            // Nur Hauptfenster und Einstellungen mit der eigenen Oberfläche (oder der Startseite der
            // App) dürfen die App steuern – nicht Vorschauen oder fremde Seiten.
            let uri = req.uri();
            let own = match (uri.host(), uri.port_u16(), *port_ipc.lock().unwrap()) {
                (Some("127.0.0.1"), Some(p), Some(sp)) => p == sp,
                (None, None, _) => true,
                _ => uri.scheme_str() == Some("about") || uri.to_string().starts_with("about:"),
            };
            let trusted = ui && own;
            let _ = p_ipc.send_event(UserEvent::Ipc { msg: req.body().clone(), trusted, window: id });
        })
        .with_on_page_load_handler(move |ev, url| {
            // Das Hauptfenster zeigt nur die eigene Oberfläche: eine fremde Seite (nur Hauptrahmen –
            // Vorschau und Browser im Canvas sind iframes) geht in den Browser des Systems, sonst
            // bliebe die App darin hängen (ohne Zurück)
            if is_main && is_web(&url) && !is_own(&url, *port_load.lock().unwrap()) {
                let _ = p_load.send_event(UserEvent::Escaped(url));
                return;
            }
            if matches!(ev, PageLoadEvent::Finished) {
                let _ = p_load.send_event(UserEvent::PageLoaded(id));
            }
        })
        .with_new_window_req_handler(move |url, _features| {
            // Seiten des eingebauten Servers (Anhänge) in einem App-Fenster, alles andere – auch
            // localhost anderer Ports (Server der Agenten) – im Browser des Systems
            if is_own(&url, *server_port.lock().unwrap()) {
                let _ = p_new.send_event(UserEvent::OpenWindow(url));
            } else {
                open_external(&url);
            }
            NewWindowResponse::Deny
        })
        .with_download_started_handler(|url, path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty())
                .or_else(|| url.rsplit('/').next().map(|s| s.split('?').next().unwrap_or(s).to_string()))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "download".into());
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            *path = unique_path(home.join("Downloads").join(name));
            true
        })
        .with_download_completed_handler(|_url, path, ok| {
            if let (Some(p), true) = (path, ok) {
                reveal(&p);
            }
        })
        // Mikrofon für Dieter: die eigene Oberfläche darf (macOS fragt beim ersten Mal selbst),
        // fremde Seiten in Zusatzfenstern fragen wie im Browser
        .with_permission_handler(move |kind| match kind {
            wry::PermissionKind::Microphone if is_main => wry::PermissionResponse::Allow,
            _ => wry::PermissionResponse::Default,
        })
        .with_clipboard(true)
        // Merkmal der App: nur so antwortet der eingebaute Server (nicht im Browser)
        .with_user_agent(server::user_agent())
        .with_devtools(devtools)
        .with_back_forward_navigation_gestures(false);
    // Windows: Browser-Kürzel von WebView2 (F5, Strg+R, Alt+←, Strg+P …) aus; Kopieren/Einfügen bleibt
    #[cfg(windows)]
    let builder = {
        use wry::WebViewBuilderExtWindows;
        builder.with_browser_accelerator_keys(!is_main)
    };
    // Eigene Oberfläche im Hintergrund nicht drosseln: WebKit hielte eine verborgene WebView sonst
    // nach einigen Minuten an und entlüde sie (macOS 14+) – zurück im Vordergrund stünde die
    // Oberfläche neu geladen da (offene Reiter, Terminals, Stelle im Chat weg)
    let builder = if ui { builder.with_background_throttling(BackgroundThrottlingPolicy::Disabled) } else { builder };
    // Endet der Inhaltsprozess trotzdem (Absturz, Speicherdruck), bliebe das Fenster leer: neu laden
    #[cfg(target_os = "macos")]
    let builder = {
        use wry::WebViewBuilderExtDarwin;
        let p_term = proxy.clone();
        builder.with_on_web_content_process_terminate_handler(move || {
            let _ = p_term.send_event(UserEvent::WebContentTerminated(id));
        })
    };
    let builder = match url {
        Some(u) => builder.with_url(u),
        None => builder.with_html(overlay::loading_page(None)),
    };
    builder.build(window)
}

fn open_window(
    target: &EventLoopWindowTarget<UserEvent>,
    proxy: &EventLoopProxy<UserEvent>,
    server_port: &Arc<Mutex<Option<u16>>>,
    extra: &mut HashMap<WindowId, (Window, WebView)>,
    url: &str,
) {
    let Ok(window) = WindowBuilder::new()
        .with_title("nrzX")
        .with_inner_size(tao::dpi::LogicalSize::new(1100.0, 780.0))
        .build(target)
    else {
        return;
    };
    // Nachrichten aus Zusatzfenstern gelten nie als vertrauenswürdig
    match build_webview(&window, proxy, server_port.clone(), false, false, Some(url)) {
        Ok(wv) => {
            extra.insert(window.id(), (window, wv));
        }
        Err(e) => tracing::error!("Fenster: {e}"),
    }
}

/// Fenster zu einer Nachricht: Hauptfenster oder Einstellungen.
fn window_of<'a>(id: WindowId, main: &'a Option<(Window, WebView)>, settings: &'a Option<(Window, WebView)>) -> Option<&'a Window> {
    [main, settings].into_iter().flatten().map(|(w, _)| w).find(|w| w.id() == id)
}

/// Einstellungen im eigenen Fenster: wie das Hauptfenster ohne Titelleiste, mit Milchglas und
/// der Ampel in der Oberfläche; die Oberfläche darf die App steuern (`settings:close` …).
fn open_settings_window(
    target: &EventLoopWindowTarget<UserEvent>,
    proxy: &EventLoopProxy<UserEvent>,
    server_port: &Arc<Mutex<Option<u16>>>,
    port: u16,
) -> Option<(Window, WebView)> {
    let builder = WindowBuilder::new()
        .with_title("Einstellungen")
        .with_inner_size(tao::dpi::LogicalSize::new(980.0, 720.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(640.0, 420.0))
        .with_transparent(true);
    #[cfg(target_os = "macos")]
    let builder = {
        use tao::platform::macos::WindowBuilderExtMacOS;
        builder
            .with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_fullsize_content_view(true)
    };
    let window = builder.build(target).ok()?;
    // Ampel wie im Hauptfenster über eine (kompakte) Toolbar
    #[cfg(target_os = "macos")]
    lights::install(&window, true);
    #[cfg(windows)]
    let _ = apply_glass(&window);
    let url = format!("http://127.0.0.1:{port}/settings/import");
    match build_webview(&window, proxy, server_port.clone(), false, true, Some(&url)) {
        Ok(wv) => {
            #[cfg(target_os = "macos")]
            if GLASS.load(Ordering::Relaxed) {
                let _ = apply_glass(&window);
            }
            Some((window, wv))
        }
        Err(e) => {
            tracing::error!("Einstellungen: {e}");
            None
        }
    }
}

/// Rand für `window:resize:<n|ne|nw|…>` (rahmenloses Fenster unter Windows).
fn resize_direction(d: &str) -> Option<tao::window::ResizeDirection> {
    use tao::window::ResizeDirection as R;
    Some(match d {
        "n" => R::North,
        "s" => R::South,
        "e" => R::East,
        "w" => R::West,
        "ne" => R::NorthEast,
        "nw" => R::NorthWest,
        "se" => R::SouthEast,
        "sw" => R::SouthWest,
        _ => return None,
    })
}

/// Windows: maximiert/Vollbild an die Oberfläche (Symbol des Maximieren-Knopfs, Ränder).
#[cfg(windows)]
fn send_window_state(window: &Window, wv: &WebView) {
    let _ = wv.evaluate_script(&format!(
        "window.__nrzWinState && window.__nrzWinState({}, {})",
        window.is_maximized(),
        window.fullscreen().is_some()
    ));
}

/// Seite des eingebauten Servers (`http://127.0.0.1:<port>` bzw. `localhost`)?
fn is_own(url: &str, port: Option<u16>) -> bool {
    let Some(p) = port else { return false };
    ["127.0.0.1", "localhost"].iter().any(|h| {
        let base = format!("http://{h}:{p}");
        url.strip_prefix(&base).is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#']))
    })
}

/// Adresse im Netz (nicht `about:`, `data:` oder die Ladeseite)?
fn is_web(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://")
}

/// Liegt Milchglas hinter dem Hauptfenster? (Gesetzt vor dem ersten WebView.)
static GLASS: AtomicBool = AtomicBool::new(false);

/// Milchglas hinter dem Hauptfenster – der Schreibtisch und Fenster dahinter scheinen
/// weichgezeichnet durch, nicht nur durchsichtig:
/// - macOS: Vibrancy (`NSVisualEffectView`, wie die Seitenleisten nativer Apps) unter dem WebView,
///   nach dessen Bau (siehe [`content_view`]).
/// - Windows 11 (ab 22H2): eigenes Acryl, getönt nach dem Thema der Oberfläche – bleibt auch,
///   wenn das Fenster nicht vorn ist. Windows 10 und 11 21H2: Weichzeichnen hinter dem Fenster –
///   deren Acryl ruckelt beim Ziehen und Vergrössern. Siehe [`glass`].
///
/// `false`, wenn das System es nicht kann – dann zeichnet die Oberfläche deckend.
fn apply_glass(window: &Window) -> bool {
    #[cfg(target_os = "macos")]
    let result = match content_view(window) {
        Some(view) => window_vibrancy::apply_vibrancy(
            view,
            window_vibrancy::NSVisualEffectMaterial::UnderWindowBackground,
            Some(window_vibrancy::NSVisualEffectState::Active),
            None,
        )
        .map_err(|e| format!("{e:?}")),
        None => Err("keine Inhaltsansicht".to_string()),
    };
    #[cfg(windows)]
    let result = {
        use tao::platform::windows::WindowExtWindows;
        // Tönung vorerst dunkel – die Oberfläche meldet ihr Thema beim Laden (`theme:`)
        glass::apply(window.hwnd(), glass::Tint::Dark)
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let result: Result<(), String> = {
        let _ = window;
        Err("nicht unterstützt".into())
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            tracing::info!("Kein Milchglas: {e}");
            false
        }
    }
}

/// Aktuelle Inhaltsansicht des Fensters als Handle für window-vibrancy. wry ersetzt die Ansicht
/// von tao durch eine eigene mit dem WebView darin; `Window` meldet aber weiter die alte, die
/// nicht mehr im Fenster hängt – Vibrancy dort bliebe unsichtbar.
#[cfg(target_os = "macos")]
struct ContentView(std::ptr::NonNull<std::ffi::c_void>);

#[cfg(target_os = "macos")]
impl tao::rwh_06::HasWindowHandle for ContentView {
    fn window_handle(&self) -> Result<tao::rwh_06::WindowHandle<'_>, tao::rwh_06::HandleError> {
        let raw = tao::rwh_06::RawWindowHandle::AppKit(tao::rwh_06::AppKitWindowHandle::new(self.0));
        // SAFETY: Das Fenster hält seine Inhaltsansicht, solange es offen ist; das Handle wird nur
        // sofort (im Hauptthread) benutzt.
        Ok(unsafe { tao::rwh_06::WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(target_os = "macos")]
fn content_view(window: &Window) -> Option<ContentView> {
    use objc2_app_kit::NSWindow;
    use tao::platform::macos::WindowExtMacOS;
    let ns = unsafe { (window.ns_window() as *const NSWindow).as_ref()? };
    let view = ns.contentView()?;
    std::ptr::NonNull::new(objc2::rc::Retained::as_ptr(&view) as *mut std::ffi::c_void).map(ContentView)
}

/// Erscheinungsbild des Fensters wie das Thema der Oberfläche (`dark`, `light`, `mondnacht`,
/// sonst wie das System) – das Milchglas dahinter (Vibrancy bzw. Acryl) wird damit dunkel bzw.
/// hell, unter Windows zusätzlich in der Farbe des Themas getönt.
fn set_window_theme(main: &Option<(Window, WebView)>, theme: &str) {
    let Some((w, _)) = main else { return };
    let native = match theme {
        "dark" | "mondnacht" => Some(tao::window::Theme::Dark),
        "light" => Some(tao::window::Theme::Light),
        _ => None,
    };
    w.set_theme(native);
    #[cfg(windows)]
    if GLASS.load(Ordering::Relaxed) {
        use tao::platform::windows::WindowExtWindows;
        let tint = match theme {
            "mondnacht" => glass::Tint::Mondnacht,
            "light" => glass::Tint::Light,
            "dark" => glass::Tint::Dark,
            // wie das System
            _ if w.theme() == tao::window::Theme::Light => glass::Tint::Light,
            _ => glass::Tint::Dark,
        };
        glass::retint(w.hwnd(), tint);
    }
}

/// Stand des Updates an die Oberfläche – sie zeigt ihn als Knopf unten in der Seitenleiste
/// (`ui/src/pages/workspace/update.rs`): `available` (mit Version), `current` (mit der eigenen
/// Version), `error` (Prüfung fehlgeschlagen, mit Text), `progress` (Anteil 0–1, -1 unbekannt),
/// `installing` oder `failed` (Laden bzw. Installieren fehlgeschlagen, mit Text).
fn update_ui(wv: &WebView, status: &str, text: &str) {
    let _ = wv.evaluate_script(&format!(
        "window.__nrzUpdateState && window.__nrzUpdateState({}, {})",
        overlay::js_str(status),
        overlay::js_str(text)
    ));
}

/// `update_ui` im Hauptfenster, falls es offen ist.
fn update_main(main: &Option<(Window, WebView)>, status: &str, text: &str) {
    if let Some((_, wv)) = main {
        update_ui(wv, status, text);
    }
}

fn start_download(
    rt: &tokio::runtime::Runtime,
    proxy: &EventLoopProxy<UserEvent>,
    updates: &mut UpdateState,
    main: &Option<(Window, WebView)>,
) {
    let Some(u) = updates.available.clone() else {
        return;
    };
    if updates.busy {
        return;
    }
    updates.busy = true;
    if let Some((w, wv)) = main {
        w.set_visible(true);
        update_ui(wv, "progress", "0");
    }
    let p = proxy.clone();
    rt.spawn(async move {
        let pp = p.clone();
        let last = Mutex::new(-1i64);
        let r = updater::download(&u, move |f| {
            // Nur bei ganzen Prozenten melden
            let step = if f < 0.0 { -1 } else { (f * 100.0) as i64 };
            let mut l = last.lock().unwrap();
            if *l != step {
                *l = step;
                let _ = pp.send_event(UserEvent::Progress(f));
            }
        })
        .await;
        let _ = p.send_event(UserEvent::Downloaded(r));
    });
}

fn build_menu() -> (Menu, MenuIds) {
    let version = updater::current_version().to_string();
    let ids = MenuIds {
        settings: MenuItem::new("Einstellungen …", true, Some("CmdOrCtrl+Comma".parse().unwrap())),
        check: MenuItem::new("Nach Updates suchen …", true, None),
        install: MenuItem::new("Update installieren …", false, None),
        quit: MenuItem::new("nrzX beenden", true, Some("CmdOrCtrl+Q".parse().unwrap())),
    };
    let about = AboutMetadata {
        name: Some("nrzX".into()),
        version: Some(version),
        comments: Some("Coding-Agenten, verbunden mit nrzGit – komplett in Rust.".into()),
        license: Some("AGPL-3.0-or-later".into()),
        ..Default::default()
    };
    let edit = Submenu::with_items(
        "Bearbeiten",
        true,
        &[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ],
    )
    .unwrap();
    let app = Submenu::with_items(
        "nrzX",
        true,
        &[
            &PredefinedMenuItem::about(Some("Über nrzX"), Some(about)),
            &PredefinedMenuItem::separator(),
            &ids.check,
            &ids.install,
            &ids.settings,
            &PredefinedMenuItem::separator(),
            &edit,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &ids.quit,
        ],
    )
    .unwrap();

    // Oben nur „nrzX“; „Bearbeiten“ liegt darin als Untermenü – ohne die Einträge gingen ⌘C, ⌘V,
    // ⌘X, ⌘A und ⌘Z im WebView nicht (macOS verteilt die Kürzel über das Menü)
    let menu = Menu::with_items(&[&app]).unwrap();
    (menu, ids)
}

/// Mitteilung von macOS (im Mitteilungszentrum, mit Ton).
fn native_notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    {
        use objc2_foundation::{NSString, NSUserNotification, NSUserNotificationCenter, NSUserNotificationDefaultSoundName};
        let title: String = title.chars().take(120).collect();
        let body: String = body.chars().take(400).collect();
        // SAFETY: Aufruf im Hauptthread (Event-Loop); deprecated, aber ohne Signatur nutzbar.
        #[allow(deprecated)]
        unsafe {
            let n = NSUserNotification::new();
            n.setTitle(Some(&NSString::from_str(&title)));
            n.setInformativeText(Some(&NSString::from_str(&body)));
            n.setSoundName(Some(NSUserNotificationDefaultSoundName));
            NSUserNotificationCenter::defaultUserNotificationCenter().deliverNotification(&n);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (title, body);
}

fn open_external(target: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // start "" <ziel> öffnet Adressen im Browser und Dateien/Ordner im Explorer
        let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(target).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(not(windows))]
    let _ = std::process::Command::new("/usr/bin/open").arg(target).spawn();
}

/// Datei im Finder bzw. Explorer zeigen.
fn reveal(path: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
}

fn message(title: &str, text: &str, level: rfd::MessageLevel) {
    rfd::MessageDialog::new().set_title(title).set_description(text).set_level(level).set_buttons(rfd::MessageButtons::Ok).show();
}

fn unique_path(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = p.parent().map(PathBuf::from).unwrap_or_default();
    (2..1000).map(|i| dir.join(format!("{stem} {i}{ext}"))).find(|c| !c.exists()).unwrap_or(p)
}

fn single_instance_lock() -> Result<std::fs::File, String> {
    // Nach einem Update startet die neue Version, während die alte noch beendet: kurz warten
    let relaunched = std::env::args().any(|a| a == "--relaunched");
    let deadline = std::time::Instant::now() + Duration::from_secs(if relaunched { 20 } else { 0 });
    loop {
        match try_instance_lock() {
            Ok(f) => return Ok(f),
            Err(e) if std::time::Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn try_instance_lock() -> Result<std::fs::File, String> {
    let dir = server::data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("app.lock"))
        .map_err(|e| e.to_string())?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(_) => Err("nrzX läuft bereits.".into()),
    }
}

/// Protokoll nach `~/Library/Logs/nrzX/nrzx.log`.
fn init_logging() -> Option<()> {
    use tracing_subscriber::EnvFilter;
    let dir = server::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let filter = || {
        EnvFilter::try_from_env("NRZX_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info,rocket::server=warn,rocket::launch::_=warn,rocket::shield=warn"))
    };
    let path = dir.join("nrzx.log");
    // Altes Protokoll ab 10 MB beiseitelegen
    if std::fs::metadata(&path).map(|m| m.len() > 10 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("nrzx.old.log"));
    }
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            tracing_subscriber::fmt().with_ansi(false).with_env_filter(filter()).with_writer(Mutex::new(file)).init();
        }
        Err(_) => {
            tracing_subscriber::fmt().with_env_filter(filter()).init();
        }
    }
    Some(())
}
