//! Bauen und Veröffentlichen von nrzX – in Rust statt Shell-Skripten.
//!
//! ```text
//! cargo xtask ui                    Oberfläche (Dioxus) nach ui/dist/public – vor jedem Programm-Build
//! cargo xtask macos [universal]     nrzX.app und DMG nach dist-macos/ (SKIP_UI=1: Oberfläche nicht neu)
//! cargo xtask release [notes.md]    DMG als Release v<version> in nrzGit veröffentlichen (NRZGIT_TOKEN)
//! cargo xtask mobile                nrzXMobile: Oberfläche und Server für diesen Rechner
//! ```
//!
//! Braucht: rustup mit `wasm32-unknown-unknown`, die Dioxus-CLI `dx` (0.7) und für macOS die
//! Werkzeuge des Systems (`sips`, `iconutil`, `codesign`, `security`, `hdiutil`, `openssl`).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

type Res<T = ()> = Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run = match args.first().map(String::as_str) {
        Some("ui") => ui(),
        Some("macos") => macos(args.get(1).map(String::as_str) == Some("universal")),
        Some("release") => release(args.get(1).map(PathBuf::from)),
        Some("mobile") => mobile(),
        _ => Err("Befehle: ui | macos [universal] | release [notes.md] | mobile".into()),
    };
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fehler: {e}");
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------------ Hilfen

/// Wurzel des Repositorys (über `xtask/`).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("Repository").to_path_buf()
}

/// Befehl ausführen; Fehler mit Namen.
fn run(dir: &Path, program: &str, args: &[&str]) -> Res {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("PATH", path_env())
        .status()
        .map_err(|e| format!("{program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {} ist fehlgeschlagen ({status})", args.join(" ")))
    }
}

/// Ausgabe eines Befehls (getrimmt).
fn output(dir: &Path, program: &str, args: &[&str]) -> Res<String> {
    let out = Command::new(program).args(args).current_dir(dir).env("PATH", path_env()).output().map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// PATH mit `~/.cargo/bin` (für `dx`) und Homebrew.
fn path_env() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.cargo/bin:/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default())
}

/// `version = "…"` aus einer Cargo.toml.
fn version_of(manifest: &Path) -> Res<String> {
    let text = std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    text.lines()
        .find_map(|l| l.strip_prefix("version = \"").and_then(|r| r.strip_suffix('"')))
        .map(str::to_string)
        .ok_or_else(|| format!("keine Version in {}", manifest.display()))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn copy_dir(from: &Path, to: &Path) -> Res {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let target = to.join(e.file_name());
        if e.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target).map_err(|err| format!("{}: {err}", e.path().display()))?;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Res<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn size_mb(path: &Path) -> String {
    let n = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    format!("{:.1} MB", n as f64 / 1_048_576.0)
}

// ------------------------------------------------------------------ Oberfläche

mod ui_theme {
    include!("../../ui/theme.rs");
}
mod mobile_theme {
    include!("../../mobile/ui/theme.rs");
}

/// Dioxus-Oberfläche bauen: Stile aus `theme.rs` nach `tailwind.css`, `dx bundle` in einen frischen
/// Ordner (dx räumt seinen Zwischenordner nicht auf), dann `dist/public` ersetzen.
fn bundle(dir: &Path, css: &str) -> Res {
    std::fs::write(dir.join("tailwind.css"), css).map_err(|e| e.to_string())?;
    // Zwischenordner von dx leeren – sonst kommen alte WebAssembly-Dateien früherer Builds mit
    if let Ok(crates) = std::fs::read_dir(dir.join("target/dx")) {
        for c in crates.flatten() {
            let _ = std::fs::remove_dir_all(c.path().join("release/web/public"));
        }
    }
    let out = format!("target/ui-bundle-{}", now());
    run(dir, "dx", &["bundle", "--web", "--release", "--out-dir", &out])?;
    let public = dir.join("dist/public");
    let _ = std::fs::remove_dir_all(&public);
    copy_dir(&dir.join(&out).join("public"), &public)?;
    let _ = std::fs::remove_dir_all(dir.join(&out));
    println!("Oberfläche gebaut: {}", public.display());
    Ok(())
}

fn ui() -> Res {
    bundle(&root().join("ui"), ui_theme::TAILWIND_CSS)
}

fn mobile() -> Res {
    let dir = root().join("mobile");
    bundle(&dir.join("ui"), mobile_theme::TAILWIND_CSS)?;
    run(&dir, "cargo", &["build", "--release", "--locked"])?;
    println!("Fertig: {}", dir.join("target/release/nrzx-mobile").display());
    Ok(())
}

// ------------------------------------------------------------------ macOS

/// nrzX.app und DMG nach dist-macos/.
fn macos(universal: bool) -> Res {
    let root = root();
    let version = version_of(&root.join("Cargo.toml"))?;
    let app_version = version_of(&root.join("macos/Cargo.toml"))?;
    if version != app_version {
        return Err(format!("Version in Cargo.toml ({version}) und macos/Cargo.toml ({app_version}) ist verschieden"));
    }
    if std::env::var("SKIP_UI").as_deref() != Ok("1") || !root.join("ui/dist/public/index.html").is_file() {
        ui()?;
    }
    let mac = root.join("macos");
    let (bin, arch) = if universal {
        run(&mac, "rustup", &["target", "add", "aarch64-apple-darwin", "x86_64-apple-darwin"])?;
        run(&mac, "cargo", &["build", "--release", "--locked", "--target", "aarch64-apple-darwin"])?;
        run(&mac, "cargo", &["build", "--release", "--locked", "--target", "x86_64-apple-darwin"])?;
        std::fs::create_dir_all(mac.join("target/universal")).map_err(|e| e.to_string())?;
        run(
            &mac,
            "lipo",
            &["-create", "-output", "target/universal/nrzx-macos", "target/aarch64-apple-darwin/release/nrzx-macos", "target/x86_64-apple-darwin/release/nrzx-macos"],
        )?;
        (mac.join("target/universal/nrzx-macos"), "universal")
    } else {
        run(&mac, "cargo", &["build", "--release", "--locked"])?;
        (mac.join("target/release/nrzx-macos"), if std::env::consts::ARCH == "aarch64" { "arm64" } else { "x86_64" })
    };

    let out = root.join("dist-macos");
    let app = out.join("nrzX.app");
    let _ = std::fs::remove_dir_all(&app);
    let contents = app.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(contents.join("Resources")).map_err(|e| e.to_string())?;
    std::fs::copy(&bin, contents.join("MacOS/nrzx-macos")).map_err(|e| format!("{}: {e}", bin.display()))?;
    let plist = std::fs::read_to_string(mac.join("resources/Info.plist")).map_err(|e| e.to_string())?.replace("__VERSION__", &version);
    std::fs::write(contents.join("Info.plist"), plist).map_err(|e| e.to_string())?;
    std::fs::write(contents.join("PkgInfo"), "APPL????").map_err(|e| e.to_string())?;
    icon(&root, &contents.join("Resources/nrzX.icns"))?;
    sign(&root, &app)?;

    // DMG: App plus Verknüpfung auf /Programme zum Hineinziehen
    let dmg = out.join(format!("nrzX-{version}-{arch}.dmg"));
    let stage = std::env::temp_dir().join(format!("nrzx-dmg-{}", now()));
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    run(&root, "cp", &["-R", &app.to_string_lossy(), &stage.to_string_lossy()])?;
    std::os::unix::fs::symlink("/Applications", stage.join("Applications")).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&dmg);
    run(
        &root,
        "hdiutil",
        &["create", "-volname", &format!("nrzX {version}"), "-srcfolder", &stage.to_string_lossy(), "-fs", "HFS+", "-format", "UDZO", "-imagekey", "zlib-level=9", "-ov", &dmg.to_string_lossy()],
    )?;
    let _ = std::fs::remove_dir_all(&stage);
    let name = dmg.file_name().unwrap().to_string_lossy().to_string();
    std::fs::write(out.join("SHA256SUMS"), format!("{}  {name}\n", sha256_file(&dmg)?)).map_err(|e| e.to_string())?;
    println!("Fertig: {}\n        {} ({})", app.display(), dmg.display(), size_mb(&dmg));
    Ok(())
}

/// Symbol aus static/icon.png (alle Grössen, mit @2x).
fn icon(root: &Path, icns: &Path) -> Res {
    let set = std::env::temp_dir().join(format!("nrzx-{}/nrzX.iconset", now()));
    std::fs::create_dir_all(&set).map_err(|e| e.to_string())?;
    let png = root.join("static/icon.png");
    for s in [16, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let px = (s * scale).to_string();
            let file = set.join(format!("icon_{s}x{s}{suffix}.png"));
            output(root, "sips", &["-z", &px, &px, &png.to_string_lossy(), "--out", &file.to_string_lossy()])?;
        }
    }
    run(root, "iconutil", &["-c", "icns", &set.to_string_lossy(), "-o", &icns.to_string_lossy()])?;
    let _ = std::fs::remove_dir_all(set.parent().unwrap());
    Ok(())
}

/// Signieren. Mit `SIGN_IDENTITY="Developer ID Application: …"` richtig (Apple-Entwicklerkonto).
/// Sonst mit einem eigenen, immer gleichen Zertifikat „nrzX Local Signing“: macOS merkt sich
/// erteilte Rechte (Ordner, Mikrofon …) nach der Signatur – ad hoc ändert sie sich mit jedem Build,
/// und nach jedem Update fehlen die Rechte wieder. Das Zertifikat entsteht beim ersten Mal in einem
/// eigenen Schlüsselbund unter `NRZX_SIGN_DIR` (Vorgabe `~/Library/Application Support/nrzX-signing`,
/// nie im Repository). `SIGN_IDENTITY=-` signiert ad hoc.
fn sign(root: &Path, app: &Path) -> Res {
    let ent = root.join("macos/resources/nrzX.entitlements");
    let (ent, app_s) = (ent.to_string_lossy().to_string(), app.to_string_lossy().to_string());
    match std::env::var("SIGN_IDENTITY").ok().as_deref() {
        Some(id) if id != "-" && !id.is_empty() => {
            run(root, "codesign", &["--force", "--deep", "--options", "runtime", "--timestamp", "--entitlements", &ent, "--sign", id, &app_s])?;
        }
        Some("-") => run(root, "codesign", &["--force", "--deep", "--sign", "-", &app_s])?,
        _ => match local_identity(root) {
            Ok((kc, name)) => run(root, "codesign", &["--force", "--deep", "--entitlements", &ent, "--keychain", &kc, "--sign", &name, &app_s])?,
            Err(e) => {
                eprintln!("Warnung: kein eigenes Zertifikat ({e}) – ad hoc signiert (macOS fragt nach Updates erneut nach Rechten).");
                run(root, "codesign", &["--force", "--deep", "--sign", "-", &app_s])?;
            }
        },
    }
    run(root, "codesign", &["--verify", "--deep", "--strict", &app_s])
}

/// Eigenes Signatur-Zertifikat (einmal anlegen, danach nur entsperren): (Schlüsselbund, Name).
fn local_identity(root: &Path) -> Res<(String, String)> {
    let name = "nrzX Local Signing";
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = std::env::var("NRZX_SIGN_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(&home).join("Library/Application Support/nrzX-signing"));
    let kc = dir.join("signing.keychain-db");
    let pwfile = dir.join("password");
    let kc_s = kc.to_string_lossy().to_string();
    if !kc.is_file() || !pwfile.is_file() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        run(root, "chmod", &["700", &dir.to_string_lossy()])?;
        let _ = std::fs::remove_file(&kc);
        let pw = output(root, "openssl", &["rand", "-hex", "24"])?;
        let tmp = std::env::temp_dir().join(format!("nrzx-cert-{}", now()));
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let cnf = "[req]\ndistinguished_name = dn\nx509_extensions = ext\nprompt = no\n[dn]\nCN = nrzX Local Signing\nO = nrzStudio\n\
                   [ext]\nbasicConstraints = critical,CA:false\nkeyUsage = critical,digitalSignature\nextendedKeyUsage = critical,codeSigning\nsubjectKeyIdentifier = hash\n";
        std::fs::write(tmp.join("cert.cnf"), cnf).map_err(|e| e.to_string())?;
        let t = |f: &str| tmp.join(f).to_string_lossy().to_string();
        output(root, "openssl", &["req", "-x509", "-newkey", "rsa:2048", "-sha256", "-days", "7300", "-nodes", "-keyout", &t("key.pem"), "-out", &t("cert.pem"), "-config", &t("cert.cnf")])?;
        let pass = format!("pass:{pw}");
        // OpenSSL 3+: altes PKCS#12-Format, das `security import` lesen kann (LibreSSL kennt -legacy nicht)
        let p12 = ["pkcs12", "-export", "-inkey", &t("key.pem"), "-in", &t("cert.pem"), "-name", name, "-out", &t("id.p12"), "-passout", &pass];
        let mut legacy = p12.to_vec();
        legacy.insert(2, "-legacy");
        output(root, "openssl", &legacy).or_else(|_| output(root, "openssl", &p12))?;
        // create-keychain trägt den Schlüsselbund in die Suchliste ein – danach wie vorher
        let before = output(root, "security", &["list-keychains", "-d", "user"])?;
        output(root, "security", &["create-keychain", "-p", &pw, &kc_s])?;
        let list: Vec<String> = before.lines().map(|l| l.trim().trim_matches('"').to_string()).filter(|l| !l.is_empty()).collect();
        let mut args = vec!["list-keychains", "-d", "user", "-s"];
        args.extend(list.iter().map(String::as_str));
        let _ = output(root, "security", &args);
        output(root, "security", &["set-keychain-settings", &kc_s])?;
        output(root, "security", &["unlock-keychain", "-p", &pw, &kc_s])?;
        output(root, "security", &["import", &t("id.p12"), "-k", &kc_s, "-P", &pw, "-T", "/usr/bin/codesign"])?;
        let _ = output(root, "security", &["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", &pw, &kc_s]);
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::write(&pwfile, &pw).map_err(|e| e.to_string())?;
        run(root, "chmod", &["600", &pwfile.to_string_lossy()])?;
        println!("Neues Signatur-Zertifikat „{name}“ in {} angelegt (aufheben – sonst vergisst macOS die Rechte beim nächsten Update).", dir.display());
    }
    let pw = std::fs::read_to_string(&pwfile).map_err(|e| e.to_string())?;
    output(root, "security", &["unlock-keychain", "-p", pw.trim(), &kc_s])?;
    Ok((kc_s, name.to_string()))
}

// ------------------------------------------------------------------ Release

/// MCP-Aufruf an nrzGit (JSON-RPC über HTTP); gibt den Text des Ergebnisses zurück.
fn mcp(http: &reqwest::blocking::Client, url: &str, token: &str, tool: &str, args: serde_json::Value) -> Res<String> {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
    let text = http
        .post(format!("{url}/mcp"))
        .bearer_auth(token)
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| format!("{tool}: {e}"))?;
    // Antwort als JSON oder als einzelnes SSE-Ereignis (`data: {…}`)
    let json = text.lines().find_map(|l| l.strip_prefix("data: ").or_else(|| l.starts_with('{').then_some(l))).ok_or_else(|| format!("{tool}: leere Antwort"))?;
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("{tool}: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(format!("{tool}: {}", e["message"].as_str().unwrap_or("Fehler")));
    }
    let content = v["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if v["result"]["isError"].as_bool() == Some(true) {
        return Err(format!("{tool}: {content}"));
    }
    Ok(content)
}

/// DMG(s) aus dist-macos/ als Release `v<version>` in nrzGit veröffentlichen. Der Tag kommt auf den
/// aktuellen Branch (muss gepusht sein). Optional: NRZGIT_URL, NRZGIT_REPO, NRZGIT_TARGET.
fn release(notes: Option<PathBuf>) -> Res {
    let root = root();
    let token = std::env::var("NRZGIT_TOKEN").map_err(|_| "NRZGIT_TOKEN fehlt (nrzGit → Einstellungen → Zugriffstoken)".to_string())?;
    let url = std::env::var("NRZGIT_URL").unwrap_or_else(|_| "https://git.zweers.ch".into()).trim_end_matches('/').to_string();
    let repo = std::env::var("NRZGIT_REPO").unwrap_or_else(|_| "nicorenezweers/nrzX".into());
    let (owner, name) = repo.split_once('/').ok_or("NRZGIT_REPO: owner/repo")?;
    let version = version_of(&root.join("Cargo.toml"))?;
    let tag = format!("v{version}");
    let target = match std::env::var("NRZGIT_TARGET") {
        Ok(t) => t,
        Err(_) => output(&root, "git", &["rev-parse", "--abbrev-ref", "HEAD"])?,
    };
    // Gepusht? Sonst landet der Tag auf einem anderen Stand
    let local = output(&root, "git", &["rev-parse", "HEAD"])?;
    let remote = output(&root, "git", &["ls-remote", "origin", &format!("refs/heads/{target}")])?;
    if !remote.starts_with(&local) {
        return Err(format!("Branch {target} ist nicht gepusht (lokal {local:.8}) – erst `git push`"));
    }
    let dist = root.join("dist-macos");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dist)
        .map_err(|e| format!("{}: {e} – zuerst `cargo xtask macos`", dist.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("nrzX-{version}-")) && n.to_string_lossy().ends_with(".dmg")))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("keine DMG für {version} in dist-macos/ – zuerst `cargo xtask macos`"));
    }
    let mut sums = String::new();
    for f in &files {
        sums += &format!("{}  {}\n", sha256_file(f)?, f.file_name().unwrap().to_string_lossy());
    }
    let notes = match notes {
        Some(p) => std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
        None => format!(
            "# nrzX {version}\n\n**macOS:** DMG öffnen und **nrzX** in **Programme** ziehen. Beim ersten Start: Rechtsklick auf die App → \
             **Öffnen** (nicht von Apple notarisiert).\n\nInstallierte Apps finden neue Versionen selbst und zeigen unten in der Seitenleiste den Knopf \
             **Update auf … installieren**."
        ),
    };
    let http = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(600)).build().map_err(|e| e.to_string())?;
    let base = serde_json::json!({"owner": owner, "repo": name});
    let with = |extra: serde_json::Value| {
        let mut v = base.clone();
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    if mcp(&http, &url, &token, "get_release_by_tag", with(serde_json::json!({"tag": tag}))).is_ok() {
        return Err(format!("Release {tag} gibt es schon – zuerst löschen oder Version erhöhen"));
    }
    println!("Release {tag} anlegen (Entwurf) …");
    let created = mcp(
        &http,
        &url,
        &token,
        "create_release",
        with(serde_json::json!({"tag_name": tag, "target": target, "title": format!("nrzX {version} (macOS)"), "body": notes, "is_draft": true})),
    )?;
    let id = serde_json::from_str::<serde_json::Value>(&created).ok().and_then(|v| v["id"].as_i64()).ok_or("create_release: keine id")?;
    use base64::Engine;
    let mut uploads: Vec<(String, Vec<u8>)> = files.iter().map(|f| (f.file_name().unwrap().to_string_lossy().to_string(), std::fs::read(f).unwrap_or_default())).collect();
    uploads.push(("SHA256SUMS".into(), sums.into_bytes()));
    for (file, bytes) in uploads {
        println!("Hochladen: {file} ({:.1} MB) …", bytes.len() as f64 / 1_048_576.0);
        let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
        mcp(&http, &url, &token, "upload_release_asset", with(serde_json::json!({"id": id, "name": file, "content_base64": content})))?;
    }
    println!("Veröffentlichen …");
    mcp(&http, &url, &token, "publish_release", with(serde_json::json!({"id": id})))?;
    println!("Fertig: {url}/{repo}/releases/tag/{tag}");
    Ok(())
}
