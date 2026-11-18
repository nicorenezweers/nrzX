//! Aufklappende Menüs und Popups (`data-popup`) passen immer ins Fenster: Ist über dem Knopf zu
//! wenig Platz, klappen sie nach unten auf (und umgekehrt); reicht keine Seite, wird die Höhe
//! auf den Platz begrenzt und der Inhalt scrollt (Spalten lesen die Höhe aus `--popup-h`).
//! Seitlich werden sie in das Fenster geschoben. Neu ausgerichtet wird beim Öffnen, wenn sich
//! der Inhalt ändert (z. B. Modelle nachgeladen) und wenn sich das Fenster ändert.

const SCRIPT: &str = r#"
(function () {
  if (window.__nrzPopups) return;
  window.__nrzPopups = true;
  var M = 8, GAP = 6;
  function fit(el) {
    var s = el.style;
    s.top = s.bottom = s.marginTop = s.marginBottom = s.maxHeight = s.overflowY = s.translate = '';
    s.removeProperty('--popup-h');
    var anchor = el.parentElement;
    if (!anchor || !el.isConnected) return;
    var r = el.getBoundingClientRect(), a = anchor.getBoundingClientRect(), H = window.innerHeight, W = window.innerWidth;
    var up = r.top < a.top;
    var above = a.top - M - GAP, below = H - a.bottom - M - GAP, need = r.height;
    var goUp = up ? (need <= above || (need > below && above >= below))
                  : (need > below && (need <= above || above > below));
    if (goUp !== up) {
      if (goUp) { s.top = 'auto'; s.bottom = '100%'; s.marginTop = '0'; s.marginBottom = GAP + 'px'; }
      else { s.bottom = 'auto'; s.top = '100%'; s.marginBottom = '0'; s.marginTop = GAP + 'px'; }
    }
    var avail = Math.max(120, Math.floor(goUp ? above : below));
    if (need > avail) {
      s.setProperty('--popup-h', avail + 'px');
      s.maxHeight = avail + 'px';
      s.overflowY = 'auto';
    }
    r = el.getBoundingClientRect();
    var dx = 0;
    if (r.right > W - M) dx = W - M - r.right;
    if (r.left + dx < M) dx = M - r.left;
    if (dx) s.translate = dx + 'px 0';
  }
  var pending = new Set(), queued = false;
  function later(el) {
    pending.add(el);
    if (queued) return;
    queued = true;
    requestAnimationFrame(function () { queued = false; var l = Array.from(pending); pending.clear(); l.forEach(fit); });
  }
  function scan(n) {
    if (!n || n.nodeType !== 1) return;
    var p = n.closest('[data-popup]');
    if (p) later(p);
    n.querySelectorAll('[data-popup]').forEach(later);
  }
  function start() {
    new MutationObserver(function (list) {
      list.forEach(function (m) {
        scan(m.target.nodeType === 1 ? m.target : m.target.parentElement);
        m.addedNodes.forEach(scan);
      });
    }).observe(document.body, { childList: true, subtree: true, characterData: true });
    window.addEventListener('resize', function () { document.querySelectorAll('[data-popup]').forEach(later); });
  }
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);
})();
"#;

/// Einmal beim Start.
pub fn install() {
    let _ = js_sys::eval(SCRIPT);
}
