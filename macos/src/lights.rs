//! Ampel (Schliessen, Minimieren, Zoomen) tiefer und weiter innen – so, wie macOS sie selbst in
//! Fenstern mit Toolbar setzt: eine leere native Toolbar (Stil „unified“, bzw. „compact“ für die
//! Einstellungen) macht die Titelleiste höher, und AppKit hält die Knöpfe darin mittig – beim
//! Vergrössern mit der Maus, Maximieren, nach Titelwechsel und Vollbild. Nichts kämpft gegen das
//! Layout von AppKit (früher setzten tao/wry bzw. nrzX die Knöpfe nach, und AppKit legte sie bei
//! jedem neuen Layout zurück – die Ampel sprang).
//!
//! Gemessen (unsichtbares Testfenster): unified Mitte 26 pt, Schliessen ab x 19, Zoomen bis x 79;
//! compact Mitte 20 pt. Klicks in dem Bereich gehen weiter an den WebView. Die tatsächliche Lage
//! meldet [`spot`] der Oberfläche (`--nrz-tl-y`, `--nrz-tl-r`), damit sich die Reihe oben danach
//! richtet.

use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSToolbar, NSWindow, NSWindowButton, NSWindowToolbarStyle};
use objc2_foundation::NSString;
use tao::platform::macos::WindowExtMacOS;
use tao::window::Window;

fn ns(window: &Window) -> Option<&NSWindow> {
    // SAFETY: NSWindow des lebenden tao-Fensters, Aufruf im Haupt-Thread.
    unsafe { (window.ns_window() as *const NSWindow).as_ref() }
}

/// Leere Toolbar ans Fenster: die Ampel sitzt danach so tief wie in nativen Apps mit Toolbar.
pub fn install(window: &Window, compact: bool) {
    let (Some(ns), Some(mtm)) = (ns(window), MainThreadMarker::new()) else { return };
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &NSString::from_str(if compact { "nrzx-compact" } else { "nrzx" }));
    ns.setToolbar(Some(&toolbar));
    ns.setToolbarStyle(if compact { NSWindowToolbarStyle::UnifiedCompact } else { NSWindowToolbarStyle::Unified });
}

/// Lage der Ampel in Punkten: Mitte unter der Oberkante und rechter Rand des Zoom-Knopfs.
pub fn spot(window: &Window) -> Option<(f64, f64)> {
    let ns = ns(window)?;
    let close = ns.standardWindowButton(NSWindowButton::CloseButton)?;
    let zoom = ns.standardWindowButton(NSWindowButton::ZoomButton)?;
    if close.isHidden() {
        return None;
    }
    let h = ns.frame().size.height;
    let c = close.convertRect_toView(close.bounds(), None);
    let z = zoom.convertRect_toView(zoom.bounds(), None);
    let center = h - (c.origin.y + c.size.height / 2.0);
    let right = z.origin.x + z.size.width;
    (center > 0.0 && center < 80.0 && right > 0.0 && right < 200.0).then_some((center, right))
}
