//! Windows: WebView2Loader.dll steckt im Programm, nrzX.exe läuft als einzelne Datei.
//!
//! webview2-com-sys bindet den Lader unter dem gnu-Ziel über die Importbibliothek ein – dann
//! steht WebView2Loader.dll in der Importtabelle und Windows bricht schon vor `main` ab, wenn die
//! Datei nicht neben der exe liegt. Darum stellt dieses Modul die Importzeiger (`__imp_…`) der
//! fünf Funktionen (und die Funktionen selbst) bereit: Der Linker zieht dann nichts aus der Importbibliothek, und die
//! Aufrufe von wry landen hier. Beim ersten Aufruf (bzw. schon beim Start über [`load`]) wird die
//! eingebettete DLL nach `%LOCALAPPDATA%\nrzX\WebView2Loader.dll` geschrieben (nur wenn sie
//! fehlt oder anders ist), mit vollem Pfad geladen und die Funktion per GetProcAddress aufgerufen.
//! scripts/windows.sh prüft, dass die DLL nicht in der Importtabelle steht.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Der Lader von Microsoft (aus webview2-com-sys, Pfad von build.rs).
static DLL: &[u8] = include_bytes!(env!("NRZ_WEBVIEW2_LOADER_DLL"));

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

struct Module(*mut c_void);
// Ein Modul-Handle ist prozessweit gültig.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

static MODULE: OnceLock<Result<Module, String>> = OnceLock::new();

/// DLL entpacken und laden; beim Start aufrufen, damit Fehler klar gemeldet werden.
pub fn load() -> Result<(), String> {
    MODULE.get_or_init(init).as_ref().map(|_| ()).map_err(Clone::clone)
}

fn init() -> Result<Module, String> {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("nrzX");
    let path = extract(&dir)?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!("{} lässt sich nicht laden: {}", path.display(), std::io::Error::last_os_error()));
    }
    tracing::info!("WebView2-Lader: {}", path.display());
    Ok(Module(module))
}

/// `dir\WebView2Loader.dll` schreiben, falls sie fehlt oder anders ist. Lässt sie sich nicht
/// ersetzen (z. B. noch von einer alten Instanz geladen), kommt eine Datei mit Prüfsumme im Namen.
fn extract(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let main = dir.join("WebView2Loader.dll");
    if same(&main) {
        return Ok(main);
    }
    if write(&main).is_ok() {
        return Ok(main);
    }
    use sha2::Digest;
    let hash = hex::encode(sha2::Sha256::digest(DLL));
    let alt = dir.join(format!("WebView2Loader-{}.dll", &hash[..12]));
    if same(&alt) {
        return Ok(alt);
    }
    write(&alt).map(|()| alt.clone()).map_err(|e| format!("{}: {e}", alt.display()))
}

fn same(path: &Path) -> bool {
    std::fs::read(path).map(|b| b == DLL).unwrap_or(false)
}

/// Erst in eine Nachbardatei schreiben, dann umbenennen – nie eine halbe DLL liegen lassen.
fn write(path: &Path) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, DLL)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn proc(name: &[u8]) -> Option<*mut c_void> {
    let module = MODULE.get_or_init(init).as_ref().ok()?;
    let p = unsafe { GetProcAddress(module.0, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// HRESULT_FROM_WIN32(ERROR_MOD_NOT_FOUND)
const MOD_NOT_FOUND: i32 = 0x8007_007E_u32 as i32;

/// Stellt `<name>` und den Importzeiger `__imp_<name>` bereit: Rust ruft Funktionen aus anderen
/// Kisten unter dem gnu-Ziel teils direkt über `<name>` auf (ohne dllimport), teils über den
/// Zeiger. Fehlt eines der beiden, zieht der Linker das Element aus der Importbibliothek – und
/// WebView2Loader.dll stünde wieder in der Importtabelle (bzw. es gibt doppelte Symbole).
macro_rules! forward {
    ($name:ident / $imp:ident ($($arg:ident: $ty:ty),*)) => {
        #[no_mangle]
        pub unsafe extern "system" fn $name($($arg: $ty),*) -> i32 {
            match proc(concat!(stringify!($name), "\0").as_bytes()) {
                Some(p) => {
                    let g: unsafe extern "system" fn($($ty),*) -> i32 = std::mem::transmute(p);
                    g($($arg),*)
                }
                None => MOD_NOT_FOUND,
            }
        }

        #[allow(non_upper_case_globals)]
        #[no_mangle]
        #[used]
        static $imp: unsafe extern "system" fn($($ty),*) -> i32 = $name;
    };
}

type Pcwstr = *const u16;
type Ptr = *mut c_void;

forward!(CompareBrowserVersions / __imp_CompareBrowserVersions(a: Pcwstr, b: Pcwstr, result: *mut i32));
forward!(CreateCoreWebView2Environment / __imp_CreateCoreWebView2Environment(handler: Ptr));
forward!(CreateCoreWebView2EnvironmentWithOptions / __imp_CreateCoreWebView2EnvironmentWithOptions(
    browser: Pcwstr,
    data: Pcwstr,
    options: Ptr,
    handler: Ptr
));
forward!(GetAvailableCoreWebView2BrowserVersionString / __imp_GetAvailableCoreWebView2BrowserVersionString(
    browser: Pcwstr,
    version: *mut *mut u16
));
forward!(GetAvailableCoreWebView2BrowserVersionStringWithOptions
    / __imp_GetAvailableCoreWebView2BrowserVersionStringWithOptions(browser: Pcwstr, options: Ptr, version: *mut *mut u16));
