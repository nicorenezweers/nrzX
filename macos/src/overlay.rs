//! Was die App selbst in die Oberfläche bringt: die Startseite (bis der Server antwortet) und die
//! Skripte fürs Fenster. Ein Update zeigt die Oberfläche selbst als Knopf unten in der Seitenleiste
//! (`update_ui` in `main.rs`, Antworten über `window.ipc.postMessage`: `update:install` …).

/// Seite, solange der Server startet.
pub fn loading_page(error: Option<&str>) -> String {
    let (title, detail) = match error {
        None => ("nrzX startet …".to_string(), String::new()),
        Some(e) => ("nrzX konnte nicht starten".to_string(), html_escape(e)),
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
:root {{ color-scheme: light dark; }}
html,body {{ margin:0; height:100%; font: 14px -apple-system, BlinkMacSystemFont, "Inter", sans-serif; background:#f4f4f5; color:#18181b; }}
@media (prefers-color-scheme: dark) {{ html,body {{ background:#18181b; color:#e4e4e7; }} .d {{ color:#a1a1aa; }} }}
.w {{ height:100%; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:14px; -webkit-user-select:none; cursor:default; }}
.s {{ width:22px; height:22px; border-radius:50%; border:2px solid currentColor; border-right-color:transparent; animation:r .8s linear infinite; opacity:.6; }}
.d {{ color:#71717a; max-width:520px; text-align:center; white-space:pre-wrap; -webkit-user-select:text; }}
@keyframes r {{ to {{ transform:rotate(360deg); }} }}
</style></head><body><div class="w">{spinner}<div>{title}</div><div class="d">{detail}</div></div></body></html>"#,
        spinner = if error.is_none() { r#"<div class="s"></div>"# } else { "" },
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// JSON-String für eingebettetes JavaScript.
pub fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Nur im Hauptfenster: Titelleiste der App in der Oberfläche (Klasse `nrz-mac` – die Kopfzeilen
/// lassen links Platz für die Ampel), Fenster an `[data-drag]` ziehen bzw. mit Doppelklick
/// zoomen, und native Mitteilungen für die Benachrichtigungen der Oberfläche.
pub const WINDOW_SCRIPT: &str = r#"
(function () {
  if (window.__nrzNative) return;
  function send(m) { try { window.ipc.postMessage(m); } catch (e) {} }
  function mark() { document.documentElement.classList.add('nrz-mac'); }
  // Klassen am <html> werden neu gesetzt – Markierung erhalten. Erst, wenn es <html> gibt: sonst
  // bräche das Skript hier ab und das Ziehen unten fehlte.
  function watch() {
    mark();
    new MutationObserver(function () { if (!document.documentElement.classList.contains('nrz-mac')) mark(); })
      .observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
  }
  if (document.documentElement) watch(); else document.addEventListener('DOMContentLoaded', watch);
  var INTERACTIVE = 'button,a,input,select,textarea,label,[role=button],[contenteditable=true]';
  function dragZone(e) {
    if (e.button !== 0 || !e.target.closest || e.target.closest(INTERACTIVE)) return false;
    if (e.target.closest('[data-drag]')) return true;
    // Seiten ohne Kopfzeile (Start, Sperre): oberster Streifen
    return e.clientY < 32 && !document.querySelector('[data-drag]');
  }
  window.addEventListener('mousedown', function (e) {
    if (!dragZone(e)) return;
    if (e.detail === 2) { send('window:zoom'); return; }
    send('window:drag');
  }, true);
  window.__nrzNative = {
    notify: function (title, body) { send('notify:' + JSON.stringify({ title: String(title || ''), body: String(body || '') })); }
  };
})();
"#;

/// Nur im Hauptfenster, wenn Milchglas dahinter liegt (macOS Vibrancy, Windows Acryl bzw. Weichzeichnen):
/// `window.__nrzGlass`. Die Oberfläche tönt dann nur noch (`html.nrz-glass`, abschaltbar unter
/// Einstellungen → Erscheinungsbild) – schon hier gesetzt, damit beim Laden nichts deckend aufblitzt.
pub const GLASS_SCRIPT: &str = r#"
(function () {
  window.__nrzGlass = true;
  try { if (localStorage.getItem('nrz-glass') === '0') return; } catch (e) {}
  function mark() { document.documentElement.classList.add('nrz-glass'); }
  if (document.documentElement) mark(); else document.addEventListener('DOMContentLoaded', mark);
})();
"#;

/// Milchglas liess sich doch nicht anbringen: Oberfläche wieder deckend (Antwort auf `theme:`).
pub const NO_GLASS_SCRIPT: &str =
    "window.__nrzGlass = false; document.documentElement && document.documentElement.classList.remove('nrz-glass');";

/// Hauptfenster unter Windows (ohne Titelleiste und Menü, wie am Mac): Klasse `nrz-win` am
/// `<html>` (die Oberfläche lässt rechts oben Platz), rechts oben Minimieren/Maximieren/Schliessen
/// im Stil von Windows 11, Ziehen an Kopfzeilen (`[data-drag]`, sonst oberster Streifen),
/// Doppelklick dort maximiert, der obere Rand ändert die Grösse (links, rechts und unten macht
/// das der Fensterrahmen des Systems). Strg+, öffnet die Einstellungen.
/// Die App meldet maximiert/Vollbild über `window.__nrzWinState(max, full)`.
pub const WIN_SCRIPT: &str = r#"
(function () {
  if (window.__nrzWin) return;
  window.__nrzWin = true;
  function send(m) { try { window.ipc.postMessage(m); } catch (e) {} }
  var de = (navigator.language || '').toLowerCase().indexOf('de') === 0;
  var state = { max: false, full: false };
  function apply() {
    var c = document.documentElement.classList;
    if (!c.contains('nrz-win')) c.add('nrz-win');
    if (c.contains('nrz-max') !== state.max) c.toggle('nrz-max', state.max);
    if (c.contains('nrz-full') !== state.full) c.toggle('nrz-full', state.full);
    var b = document.getElementById('nrz-cap-max');
    if (b) {
      b.textContent = state.max ? '' : '';
      b.title = state.max ? (de ? 'Verkleinern' : 'Restore') : (de ? 'Maximieren' : 'Maximize');
    }
  }
  window.__nrzWinState = function (max, full) { state.max = !!max; state.full = !!full; apply(); };
  if (document.documentElement) {
    apply();
    // Klassen am <html> werden beim Themenwechsel neu gesetzt – Markierung erhalten
    new MutationObserver(apply).observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
  }
  var css = '#nrz-caption{position:fixed;top:0;right:0;z-index:2147483647;display:flex;height:32px;-webkit-user-select:none;user-select:none}'
    + '#nrz-caption button{all:unset;box-sizing:border-box;width:46px;height:32px;display:grid;place-items:center;font:10px "Segoe Fluent Icons","Segoe MDL2 Assets";color:var(--foreground,#e5e5e5);cursor:default}'
    + '#nrz-caption button:hover{background:rgba(128,128,128,.22)}'
    + '#nrz-caption button:active{background:rgba(128,128,128,.34)}'
    + '#nrz-caption button.nrz-close:hover{background:#c42b1c;color:#fff}'
    + '#nrz-caption button.nrz-close:active{background:#b22a1b;color:#fff}'
    + '.nrz-edge{position:fixed;z-index:2147483647}'
    + '#nrz-edge-n{top:0;left:8px;right:8px;height:4px;cursor:n-resize}'
    + '#nrz-edge-nw{top:0;left:0;width:8px;height:6px;cursor:nw-resize}'
    + '#nrz-edge-ne{top:0;right:0;width:8px;height:6px;cursor:ne-resize}'
    + '.nrz-max .nrz-edge,.nrz-full .nrz-edge,.nrz-full #nrz-caption{display:none}';
  var EDGES = ['n', 'nw', 'ne'];
  function el(tag, id, cls) { var e = document.createElement(tag); e.id = id; if (cls) e.className = cls; return e; }
  function build() {
    if (!document.body || document.getElementById('nrz-caption')) return;
    var st = el('style', 'nrz-caption-css'); st.textContent = css;
    (document.head || document.documentElement).appendChild(st);
    var bar = el('div', 'nrz-caption');
    [['nrz-cap-min', '', '', de ? 'Minimieren' : 'Minimize', 'window:minimize'],
     ['nrz-cap-max', '', '', de ? 'Maximieren' : 'Maximize', 'window:zoom'],
     ['nrz-cap-close', 'nrz-close', '', de ? 'Schliessen' : 'Close', 'window:close']].forEach(function (d) {
      var b = el('button', d[0], d[1]);
      b.type = 'button'; b.tabIndex = -1; b.textContent = d[2]; b.title = d[3];
      b.addEventListener('mousedown', function (e) { e.stopPropagation(); });
      b.addEventListener('dblclick', function (e) { e.stopPropagation(); });
      b.addEventListener('click', function (e) { e.preventDefault(); e.stopPropagation(); send(d[4]); });
      bar.appendChild(b);
    });
    document.body.appendChild(bar);
    EDGES.forEach(function (d) {
      var e = el('div', 'nrz-edge-' + d, 'nrz-edge');
      e.addEventListener('mousedown', function (ev) {
        if (ev.button !== 0) return;
        ev.preventDefault(); ev.stopPropagation();
        send('window:resize:' + d);
      });
      document.body.appendChild(e);
    });
    apply();
    // Immer zuoberst (auch über der Sperre): ans Ende von <body>, wenn dort etwas dazukommt
    new MutationObserver(function () {
      var last = document.body.lastElementChild;
      if (last && last.id !== 'nrz-edge-ne') {
        document.body.appendChild(bar);
        EDGES.forEach(function (d) { var x = document.getElementById('nrz-edge-' + d); if (x) document.body.appendChild(x); });
      }
    }).observe(document.body, { childList: true });
  }
  if (document.body) build(); else document.addEventListener('DOMContentLoaded', build);
  // Ziehen erst nach einer kleinen Bewegung (so bleibt der Doppelklick zum Maximieren erhalten)
  var INTERACTIVE = 'button,a,input,select,textarea,label,[role=button],[contenteditable=true],#nrz-caption,.nrz-edge';
  function dragZone(e) {
    if (e.button !== 0 || !e.target.closest || e.target.closest(INTERACTIVE)) return false;
    if (e.target.closest('[data-drag]')) return true;
    // Seiten ohne Kopfzeile (Start, Sperre): oberster Streifen
    return e.clientY < 32 && !document.querySelector('[data-drag]');
  }
  var press = null;
  window.addEventListener('mousedown', function (e) { press = dragZone(e) ? { x: e.screenX, y: e.screenY } : null; }, true);
  window.addEventListener('mousemove', function (e) {
    if (!press) return;
    if (!(e.buttons & 1)) { press = null; return; }
    if (Math.abs(e.screenX - press.x) + Math.abs(e.screenY - press.y) < 4) return;
    press = null;
    send('window:drag');
  }, true);
  window.addEventListener('mouseup', function () { press = null; }, true);
  window.addEventListener('dblclick', function (e) { if (dragZone(e)) send('window:zoom'); }, true);
  window.addEventListener('keydown', function (e) {
    if (e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey && e.key === ',') {
      e.preventDefault();
      if (window.__nrzOpenSettings) window.__nrzOpenSettings(); else send('settings:open');
    }
  }, true);
})();
"#;

/// Hauptfenster auf allen Systemen: soll sich wie eine native App anfühlen, nicht wie eine
/// Webseite – nichts ziehbar (Bilder, Symbole, Links), kein Browser-Kontextmenü (Neu laden,
/// Zurück; nur Eingabefelder behalten Kopieren/Einfügen), kein Neuladen oder Vor/Zurück per
/// Tastenkürzel oder Maustaste, keine Textauswahl auf Bedienelementen. Inhalte wie Chat und
/// Code bleiben auswählbar (Kopieren mit ⌘C).
pub const NATIVE_SCRIPT: &str = r#"
(function () {
  if (window.__nrzFeel) return;
  window.__nrzFeel = true;
  var css = 'img,svg,a,video,canvas{-webkit-user-drag:none;user-drag:none}'
    + 'button,[role=button],[role=menuitem],[role=tab],[role=option],label,nav,svg,img,[data-drag]{-webkit-user-select:none;user-select:none}'
    + 'a:not([href^="http"]){-webkit-touch-callout:none}'
    // Wie eine App: kein Gummiband am Rand des Fensters, Pfeil statt Hand über Einträgen, keine
    // Auswahl in Listen, kein Fokusrahmen nach Mausklicks
    + 'html,body{overscroll-behavior:none;overflow:hidden}'
    + 'a,[role=link]{cursor:default}'
    + 'aside a,nav a{-webkit-user-select:none;user-select:none}'
    + ':focus:not(:focus-visible){outline:none}';
  function style() {
    if (document.getElementById('nrz-feel-css')) return;
    var s = document.createElement('style'); s.id = 'nrz-feel-css'; s.textContent = css;
    (document.head || document.documentElement).appendChild(s);
  }
  if (document.documentElement) style(); else document.addEventListener('DOMContentLoaded', style);
  document.addEventListener('DOMContentLoaded', style);
  function editable(t) {
    return !!(t && t.closest && t.closest('input,textarea,select,[contenteditable=""],[contenteditable=true]'));
  }
  // Ziehen nur, wo die Oberfläche es ausdrücklich erlaubt (draggable="true", z. B. Tabs)
  window.addEventListener('dragstart', function (e) {
    var t = e.target;
    if (t && t.nodeType === 1 && t.closest && t.closest('[draggable="true"]')) return;
    if (editable(t)) return;
    e.preventDefault();
  }, true);
  // Browser-Kontextmenü nirgends – nur in Eingabefeldern (Kopieren/Einfügen). Eigene Menüs der
  // Oberfläche (z. B. Rechtsklick auf einen Thread) laufen vorher und bleiben. Ein Rechtsklick
  // markiert unter macOS das Wort darunter – das wird hier wieder aufgehoben.
  window.addEventListener('contextmenu', function (e) {
    if (editable(e.target)) return;
    e.preventDefault();
  }, true);
  window.addEventListener('mousedown', function (e) {
    if (e.button === 2 && !editable(e.target)) e.preventDefault();
  }, true);
  window.addEventListener('keydown', function (e) {
    var k = (e.key || '').toLowerCase(), mod = e.metaKey || e.ctrlKey;
    var block = k === 'f5' || k === 'browserrefresh' || k === 'browserback' || k === 'browserforward'
      || (mod && !e.altKey && (k === 'r' || k === 'p'))
      || (e.metaKey && (k === '[' || k === ']'))
      || (e.altKey && !mod && (k === 'arrowleft' || k === 'arrowright') && !editable(e.target))
      || (k === 'f3' || k === 'f7' || k === 'caretbrowsing')
      || (mod && e.shiftKey && (k === 'i' || k === 'j' || k === 'c') && !window.__nrzDevtools);
    if (block) { e.preventDefault(); }
  }, true);
  // Maustasten 4/5 (Zurück/Vor)
  ['mouseup', 'mousedown', 'auxclick'].forEach(function (t) {
    window.addEventListener(t, function (e) { if (e.button === 3 || e.button === 4) e.preventDefault(); }, true);
  });
  // Links auf fremde Seiten (Chat, Markdown, localhost der Agenten …) nie im Fenster von nrzX,
  // immer im Browser des Systems – window.open geht über den Handler der App nach draussen
  function foreignLink(e) {
    if ((e.type === 'click' && e.button !== 0) || (e.type === 'auxclick' && e.button !== 1)) return;
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!a || a.hasAttribute('download')) return;
    var u;
    try { u = new URL(a.getAttribute('href'), location.href); } catch (x) { return; }
    if (u.origin === location.origin || !/^(https?|mailto|tel):$/.test(u.protocol)) return;
    e.preventDefault();
    e.stopPropagation();
    window.open(u.href, '_blank');
  }
  window.addEventListener('click', foreignLink, true);
  window.addEventListener('auxclick', foreignLink, true);
  // Dateien, die neben eine Ablagefläche fallen, nicht als Seite öffnen
  window.addEventListener('dragover', function (e) { if (!e.defaultPrevented) e.preventDefault(); });
  window.addEventListener('drop', function (e) { if (!e.defaultPrevented) e.preventDefault(); });
})();
"#;
