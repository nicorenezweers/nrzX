//! Windows: Milchglas hinter dem Hauptfenster.
//!
//! Bis 1.0.29 war das Fenster flach grau statt durchscheinend: Windows füllte die Fläche des
//! Fensters weiss, und unter dem durchsichtigen WebView lag dieses Weiss statt des Glases (siehe
//! [`clear_background`]). Dazu folgt das Acryl des Systems (`DWMSBT_TRANSIENTWINDOW`) hell/dunkel
//! des Systems statt dem Thema der Oberfläche.
//!
//! Darum bekommt das Fenster einen durchsichtigen Grund und zeichnet sein Acryl selbst (`SetWindowCompositionAttribute`,
//! `ACCENT_ENABLE_ACRYLICBLURBEHIND`): Der Schreibtisch dahinter bleibt weichgezeichnet, ob das
//! Fenster vorn ist oder nicht, und die Tönung kommt vom Thema der Oberfläche (dunkel, hell,
//! Mondnacht), nicht vom System. Vor Windows 11 22H2 ruckelt dieses Acryl beim Ziehen – dort
//! bleibt es beim einfachen Weichzeichnen.
//!
//! `NRZX_GLASS` wählt die Art von Hand: `acrylic` (Vorgabe ab 22H2), `blur`, `system` (Acryl
//! des Systems), `mica`, `off`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, Ordering};

/// Tönung des Glases – wie das Thema der Oberfläche.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tint {
    Dark,
    Light,
    Mondnacht,
}

impl Tint {
    /// Farbe unter dem Inhalt als (R, G, B, Deckkraft). Die Oberfläche tönt darüber noch einmal
    /// (`html.nrz-glass` in ui/tailwind.css) – hier nur so viel, dass helle Fenster dahinter
    /// nicht durchschlagen.
    fn rgba(self) -> (u8, u8, u8, u8) {
        match self {
            Tint::Dark => (24, 24, 27, 70),
            Tint::Light => (246, 246, 248, 110),
            Tint::Mondnacht => (16, 15, 39, 80),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Off = 0,
    Acrylic = 1,
    Blur = 2,
    System = 3,
    Mica = 4,
}

/// Gewählte Art (für [`retint`]).
static MODE: AtomicU8 = AtomicU8::new(Mode::Off as u8);

fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Acrylic,
        2 => Mode::Blur,
        3 => Mode::System,
        4 => Mode::Mica,
        _ => Mode::Off,
    }
}

fn pick_mode() -> Mode {
    let build = windows_version::OsVersion::current().build;
    match std::env::var("NRZX_GLASS").unwrap_or_default().trim().to_lowercase().as_str() {
        "off" | "0" => Mode::Off,
        "blur" => Mode::Blur,
        "system" if build >= 22523 => Mode::System,
        "mica" if build >= 22523 => Mode::Mica,
        "acrylic" if build >= 17763 => Mode::Acrylic,
        // Windows 11 ab 22H2: eigenes Acryl. Davor: Weichzeichnen (ab Windows 10 1809).
        _ if build >= 22621 => Mode::Acrylic,
        _ if build >= 17763 => Mode::Blur,
        _ => Mode::Off,
    }
}

/// Milchglas anbringen. `Err`, wenn das System es nicht kann oder es abgeschaltet ist – dann
/// zeichnet die Oberfläche deckend.
pub fn apply(hwnd: isize, tint: Tint) -> Result<(), String> {
    let m = pick_mode();
    MODE.store(m as u8, Ordering::Relaxed);
    if m != Mode::Off {
        clear_background(hwnd);
    }
    tracing::info!("Milchglas: {m:?}");
    draw(hwnd, m, tint)
}

/// Tönung an das Thema der Oberfläche anpassen (nach `theme:`).
pub fn retint(hwnd: isize, tint: Tint) {
    let m = mode();
    if m != Mode::Off {
        let _ = draw(hwnd, m, tint);
    }
}

fn draw(hwnd: isize, m: Mode, tint: Tint) -> Result<(), String> {
    let dark = (tint != Tint::Light) as u32;
    match m {
        Mode::Off => Err("abgeschaltet".into()),
        Mode::Acrylic => {
            let (r, g, b, a) = tint.rgba();
            accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, r, g, b, a)
        }
        Mode::Blur => {
            let (r, g, b, _) = tint.rgba();
            accent(hwnd, ACCENT_ENABLE_BLURBEHIND, r, g, b, 0)
        }
        Mode::System | Mode::Mica => {
            // Das Material des Systems folgt dem Dunkelmodus des Fensters
            dwm_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, dark)?;
            dwm_attr(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, if m == Mode::Mica { DWMSBT_MAINWINDOW } else { DWMSBT_TRANSIENTWINDOW })
        }
    }
}

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
const DWMSBT_MAINWINDOW: u32 = 2;
const DWMSBT_TRANSIENTWINDOW: u32 = 3;

const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    /// 0xAABBGGRR
    color: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attrib: u32,
    data: *mut c_void,
    size: usize,
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const c_void, size: u32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn SetClassLongPtrW(hwnd: isize, index: i32, value: isize) -> usize;
    fn InvalidateRect(hwnd: isize, rect: *const c_void, erase: i32) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn GetStockObject(i: i32) -> *mut c_void;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// Fenstergrund schwarz statt weiss: Ohne eigenen Pinsel füllt Windows die Fläche des Fensters
/// weiss – unter dem durchsichtigen WebView lag dann Weiss statt des Glases, und die getönte
/// Oberfläche darüber ergab das flache Grau. Schwarz ohne Deckkraft ist für den Fenstermanager
/// durchsichtig: dahinter erscheint das Acryl.
fn clear_background(hwnd: isize) {
    const GCLP_HBRBACKGROUND: i32 = -10;
    const BLACK_BRUSH: i32 = 4;
    unsafe {
        SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, GetStockObject(BLACK_BRUSH) as isize);
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

fn dwm_attr(hwnd: isize, attr: u32, value: u32) -> Result<(), String> {
    let hr = unsafe { DwmSetWindowAttribute(hwnd, attr, &value as *const u32 as *const c_void, 4) };
    if hr == 0 {
        Ok(())
    } else {
        Err(format!("DwmSetWindowAttribute({attr}): {hr:#x}"))
    }
}

/// `SetWindowCompositionAttribute` (user32, nicht dokumentiert – darum per Name gesucht statt
/// gelinkt): Akzent hinter dem Fenster setzen.
fn accent(hwnd: isize, state: u32, r: u8, g: u8, b: u8, a: u8) -> Result<(), String> {
    type Swca = unsafe extern "system" fn(isize, *mut CompositionData) -> i32;
    let user32: Vec<u16> = "user32.dll\0".encode_utf16().collect();
    let f = unsafe {
        let module = GetModuleHandleW(user32.as_ptr());
        if module.is_null() {
            return Err("user32.dll nicht geladen".into());
        }
        GetProcAddress(module, c"SetWindowCompositionAttribute".as_ptr() as *const u8)
    };
    if f.is_null() {
        return Err("SetWindowCompositionAttribute fehlt".into());
    }
    let f: Swca = unsafe { std::mem::transmute(f) };
    let acrylic = state == ACCENT_ENABLE_ACRYLICBLURBEHIND;
    // Acryl mit Deckkraft 0 zeichnet Windows gar nicht
    let a = if acrylic { a.max(1) } else { a };
    let mut policy = AccentPolicy {
        state,
        flags: if acrylic { 0 } else { 2 },
        color: (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24),
        animation: 0,
    };
    let mut data = CompositionData { attrib: WCA_ACCENT_POLICY, data: &mut policy as *mut AccentPolicy as *mut c_void, size: std::mem::size_of::<AccentPolicy>() };
    if unsafe { f(hwnd, &mut data) } != 0 {
        Ok(())
    } else {
        Err("SetWindowCompositionAttribute abgelehnt".into())
    }
}
