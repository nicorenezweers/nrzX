//! Windows: Symbol und Versionsangaben ins Programm (`resources/nrzX.ico`, mit windres aus
//! mingw-w64). Unter macOS kommt das Symbol über das Bundle (scripts/macos.sh).
//! Außerdem den Pfad von WebView2Loader.dll für `include_bytes!` (src/webview2_loader.rs).

fn main() {
    println!("cargo:rerun-if-changed=resources/nrzX.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    webview2_loader();
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("nrzX.ico");
    let version = env!("CARGO_PKG_VERSION");
    let nums: Vec<&str> = version.split('.').chain(std::iter::repeat("0")).take(4).collect();
    let rc = format!(
        "1 ICON \"{}\"\n1 VERSIONINFO\nFILEVERSION {n}\nPRODUCTVERSION {n}\nBEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040704B0\"\n  BEGIN\n   VALUE \"CompanyName\", \"nrzStudio\"\n   VALUE \"FileDescription\", \"nrzX\"\n   VALUE \"FileVersion\", \"{version}\"\n   VALUE \"ProductName\", \"nrzX\"\n   VALUE \"ProductVersion\", \"{version}\"\n   VALUE \"LegalCopyright\", \"AGPL-3.0-or-later\"\n   VALUE \"OriginalFilename\", \"nrzX.exe\"\n  END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x407, 1200\n END\nEND\n",
        ico.display().to_string().replace('\\', "/"),
        n = nums.join(","),
    );
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).unwrap();
    let res = out.join("app.res.o");
    // mingw-w64 unter macOS/Linux bringt `x86_64-w64-mingw32-windres` mit, unter Windows
    // (WinLibs, MSYS2) heisst es nur `windres`
    println!("cargo:rerun-if-env-changed=WINDRES");
    let candidates: Vec<String> = match std::env::var("WINDRES") {
        Ok(w) => vec![w],
        Err(_) => vec!["x86_64-w64-mingw32-windres".into(), "windres".into()],
    };
    let ok = candidates.iter().any(|windres| {
        std::process::Command::new(windres).arg(&rc_path).args(["-O", "coff", "-o"]).arg(&res).status().is_ok_and(|s| s.success())
    });
    if ok {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        println!("cargo:warning={} nicht gefunden – nrzX.exe ohne Symbol", candidates.join(" / "));
    }
}

/// WebView2Loader.dll aus webview2-com-sys (Version aus Cargo.lock) im Cargo-Register suchen und
/// als `NRZ_WEBVIEW2_LOADER_DLL` weitergeben. Eigener Pfad: Umgebungsvariable WEBVIEW2_LOADER_DLL.
fn webview2_loader() {
    use std::path::PathBuf;
    println!("cargo:rerun-if-env-changed=WEBVIEW2_LOADER_DLL");
    println!("cargo:rerun-if-changed=Cargo.lock");
    let found = std::env::var_os("WEBVIEW2_LOADER_DLL").map(PathBuf::from).or_else(|| {
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => "x64",
            Ok("aarch64") => "arm64",
            _ => "x86",
        };
        let lock = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock")).ok()?;
        let mut lines = lock.lines();
        lines.find(|l| *l == "name = \"webview2-com-sys\"")?;
        let version = lines.next()?.strip_prefix("version = \"")?.trim_end_matches('"').to_string();
        let homes = [
            std::env::var_os("CARGO_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")),
            std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".cargo")),
        ];
        homes.into_iter().flatten().find_map(|home| {
            std::fs::read_dir(home.join("registry").join("src")).ok()?.flatten().find_map(|index| {
                let dll = index.path().join(format!("webview2-com-sys-{version}")).join(arch).join("WebView2Loader.dll");
                dll.is_file().then_some(dll)
            })
        })
    });
    match found {
        Some(dll) => println!("cargo:rustc-env=NRZ_WEBVIEW2_LOADER_DLL={}", dll.display()),
        None => panic!("WebView2Loader.dll von webview2-com-sys nicht gefunden (CARGO_HOME setzen oder WEBVIEW2_LOADER_DLL=<Pfad>)"),
    }
}
