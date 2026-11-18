//! Unterschiede zwischen Unix (Linux, macOS) und Windows an einer Stelle: Prozesse starten
//! (unter Windows ohne Konsolenfenster und npm-Shims `*.cmd` direkt über `node`), ganze
//! Prozessbäume beenden, ausführbare Dateien erkennen und Umgebungsvariablen, die Programme
//! unter Windows brauchen.

use std::path::{Path, PathBuf};

/// `CREATE_NO_WINDOW`: Konsolenprogramme aus der Fenster-App ohne aufblitzendes Fenster.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Befehl für ein Programm. Unter Windows werden npm-Shims aufgelöst: Startet `claude.cmd` ein
/// Programm (`claude.exe`), läuft dieses direkt, ein Skript (`codex.js`, `npm-cli.js`) über
/// `node <skript>` – `.cmd`-Dateien vertragen keine Argumente mit Zeilenumbrüchen (Systemprompt).
pub fn command(program: impl AsRef<Path>) -> tokio::process::Command {
    let (program, pre) = resolve_program(program.as_ref());
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(pre);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Programm und vorangestellte Argumente für einen Prozess – unter Windows mit aufgelöstem
/// npm-Shim wie bei [`command`] (`node <skript>` bzw. das Programm dahinter). Auch für Terminals.
pub fn resolve_program(program: &Path) -> (PathBuf, Vec<PathBuf>) {
    #[cfg(windows)]
    match resolve_shim(program) {
        Some(Shim::Node { node, script }) => return (node, vec![script]),
        Some(Shim::Exe(exe)) => return (exe, Vec::new()),
        None => {}
    }
    (program.to_path_buf(), Vec::new())
}

/// Prozess in eine eigene Gruppe legen (Unix), damit sich alle Kinder zusammen beenden lassen.
pub fn own_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Prozess samt Kindern beenden; `force` = sofort (sonst höflich, Unix: SIGTERM).
pub fn kill_tree(pid: u32, force: bool) {
    #[cfg(unix)]
    // SAFETY: killpg sendet nur ein Signal an die eigene Prozessgruppe des Kindes.
    unsafe {
        libc::killpg(pid as libc::pid_t, if force { libc::SIGKILL } else { libc::SIGTERM });
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = force;
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Ist das eine ausführbare Datei?
pub fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        matches!(ext.as_str(), "exe" | "cmd" | "bat" | "com") && p.is_file()
    }
}

/// Mögliche Dateinamen eines Programms (Windows: mit `.exe`, `.cmd` …).
pub fn program_names(program: &str) -> Vec<String> {
    if cfg!(windows) && Path::new(program).extension().is_none() {
        ["exe", "cmd", "bat"].iter().map(|e| format!("{program}.{e}")).collect()
    } else {
        vec![program.to_string()]
    }
}

/// Ausführbare Datei als „von nrzX ausführbar“ markieren (Unix: 0755).
pub fn make_executable(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        Ok(())
    }
}

/// Variablen, die Programme unter Windows brauchen (sonst scheitern z. B. Netzwerk und Node).
pub const WINDOWS_INHERIT: &[&str] = &[
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "TEMP",
    "TMP",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "ProgramData",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "USERNAME",
    "USERDOMAIN",
    "COMPUTERNAME",
    "PSModulePath",
];

/// Home-Variablen eines Kontos: unter Windows lesen die CLIs `USERPROFILE`/`APPDATA` statt `HOME`.
pub fn home_vars(home: &Path) -> Vec<(String, String)> {
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let mut v = vec![("HOME".to_string(), s(home.to_path_buf()))];
    if cfg!(windows) {
        v.push(("USERPROFILE".into(), s(home.to_path_buf())));
        v.push(("APPDATA".into(), s(home.join("AppData").join("Roaming"))));
        v.push(("LOCALAPPDATA".into(), s(home.join("AppData").join("Local"))));
    }
    v
}

/// Shell für Terminals.
pub fn default_shell() -> String {
    if cfg!(windows) {
        // PowerShell ist überall vorhanden; ComSpec (cmd.exe) als Rückfall
        if let Some(ps) = which_in_path("pwsh.exe").or_else(|| which_in_path("powershell.exe")) {
            return ps.to_string_lossy().into_owned();
        }
        return std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
    }
    String::new()
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Was ein npm-Shim startet.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq)]
enum Shim {
    /// Skript für `node` (`node <skript> …`).
    Node { node: PathBuf, script: PathBuf },
    /// Programm, das der Shim direkt startet (z. B. `claude.exe`).
    Exe(PathBuf),
}

/// npm-Shim (`foo.cmd`) auflösen. `None`, wenn es kein Shim ist oder sein Ziel fehlt.
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_shim(program: &Path) -> Option<Shim> {
    let ext = program.extension()?.to_string_lossy().to_lowercase();
    if ext != "cmd" && ext != "bat" {
        return None;
    }
    let text = std::fs::read_to_string(program).ok()?;
    let (target, via_node) = shim_target(&text)?;
    let dir = program.parent()?;
    let target = dir.join(target.replace('\\', std::path::MAIN_SEPARATOR_STR));
    if !target.is_file() {
        return None;
    }
    if !via_node {
        return Some(Shim::Exe(target));
    }
    // node neben dem Shim (npm-Prefix mit eigenem Node), das Node.js von nrzX neben seinem
    // npm-Prefix (`tools/node` neben `tools/npm`) oder aus dem PATH
    let node = [Some(dir.join("node.exe")), dir.parent().map(|p| p.join("node").join("node.exe"))]
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
        .or_else(|| which_in_path("node.exe"))?;
    Some(Shim::Node { node, script: target })
}

/// Ziel eines npm-Shims (relativ zum Shim) und ob es über `node` läuft:
/// - cmd-Shim für ein Skript: `"%_prog%"  "%dp0%\node_modules\…\cli.js" %*` (`_prog` = node.exe)
/// - cmd-Shim für ein Programm: `"%dp0%\node_modules\…\bin\claude.exe"   %*`
/// - `npm.cmd`/`npx.cmd`: `SET "NPM_CLI_JS=%~dp0\node_modules\npm\bin\npm-cli.js"`. Das davor
///   genannte `npm-prefix.js` sucht nur den Prefix – startete man es, „gelänge“ jedes
///   `npm install`, ohne etwas zu installieren.
///
/// Das Ziel ist der letzte `%dp0%`-Pfad außer `node.exe` und `npm-prefix.js`. Skripte ohne Endung
/// (`bin/kilo` mit `#!/usr/bin/env node`) laufen nur über node, wenn der Shim node startet.
fn shim_target(text: &str) -> Option<(String, bool)> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"%~?dp0%?\\?([^"%\r\n]+)"#).unwrap());
    let target = RE
        .captures_iter(text)
        .map(|c| c[1].trim().trim_start_matches('\\').to_string())
        .filter(|t| {
            let name = t.rsplit('\\').next().unwrap_or(t).to_ascii_lowercase();
            !name.is_empty() && name != "node.exe" && name != "npm-prefix.js"
        })
        .last()?;
    let lower = target.to_ascii_lowercase();
    if lower.ends_with(".exe") || lower.ends_with(".com") {
        return Some((target, false));
    }
    text.to_ascii_lowercase().contains("node.exe").then_some((target, true))
}

/// Ersatz für `security` (macOS) am Anfang von `PATH` der Konto-Prozesse. Konten laufen mit
/// eigenem `HOME`, dort findet macOS keinen Schlüsselbund. Will ein CLI trotzdem etwas
/// hineinschreiben (Claude Code beim Erneuern der Anmeldung), zeigt macOS den Dialog
/// „Schlüsselbund nicht gefunden“. Solche Schreibversuche scheitern hier still mit dem Code für
/// „kein Standard-Schlüsselbund“ (37); die CLIs speichern dann wie bisher in ihrer Datei im Home
/// des Kontos (`.claude/.credentials.json`, die auch die Nutzungsanzeige liest). Lesen,
/// ausdrücklich genannte Schlüsselbunde und Homes mit Schlüsselbund gehen unverändert an
/// `/usr/bin/security` (`NRZX_REAL_SECURITY` nur für Tests).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SECURITY_SHIM: &str = r#"#!/bin/sh
# nrzX: Schreiben in den Standard-Schlüsselbund still ablehnen, wenn es im (Konto-)Home
# keinen gibt – sonst zeigt macOS „Schlüsselbund nicht gefunden“. Alles andere: /usr/bin/security.
real=${NRZX_REAL_SECURITY:-/usr/bin/security}
cmd= skip= interactive=
for a in "$@"; do
  if [ -n "$skip" ]; then skip=; continue; fi
  case "$a" in
    -p) skip=1; interactive=1 ;;
    -p*|-*i*) interactive=1 ;;
    -*) ;;
    *) cmd=$a; break ;;
  esac
done
write=
case "$cmd" in
  add-generic-password|add-internet-password)
    write=1
    for a in "$@"; do
      case "$a" in *.keychain|*.keychain-db) write= ;; esac
    done ;;
  '') [ -n "$interactive" ] && write=1 ;;
esac
if [ -n "$write" ] && ! "$real" default-keychain >/dev/null 2>&1; then
  echo "security: SecKeychainCopyDefault: A default keychain could not be found." >&2
  exit 37
fi
exec "$real" "$@"
"#;

/// Ordner mit dem `security`-Ersatz ([`SECURITY_SHIM`]) für den Anfang von `PATH` der
/// Konto-Prozesse; legt ihn bei Bedarf an. Nur macOS, sonst `None`.
pub fn keychain_shim_dir(data_dir: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let dir = data_dir.join("shims");
        return write_security_shim(&dir).ok().map(|_| dir);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = data_dir;
        None
    }
}

/// `<dir>/security` schreiben (nur wenn er fehlt oder veraltet ist; atomar ersetzt).
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn write_security_shim(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("security");
    if std::fs::read_to_string(&path).is_ok_and(|s| s == SECURITY_SHIM) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".security-{}", std::process::id()));
    std::fs::write(&tmp, SECURITY_SHIM)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    const HEAD: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n";

    /// cmd-Shim für ein Skript mit `#!/usr/bin/env <prog>` (wie ihn npm unter Windows anlegt).
    fn script_shim(prog: &str, target: &str) -> String {
        format!("{HEAD}\r\nIF EXIST \"%dp0%\\{prog}.exe\" (\r\n  SET \"_prog=%dp0%\\{prog}.exe\"\r\n) ELSE (\r\n  SET \"_prog={prog}\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\{target}\" %*\r\n")
    }

    /// cmd-Shim für ein Programm ohne Shebang (Claude Code, OpenCode: `bin/<name>.exe`).
    fn exe_shim(target: &str) -> String {
        format!("{HEAD}\"%dp0%\\{target}\"   %*\r\n")
    }

    /// `npm.cmd` aus Node.js 26.13.0 (npm 11.20.0), Byte für Byte.
    const NPM_CMD: &str = ":: Created by npm, please don't edit manually.\r\n@ECHO OFF\r\n\r\nSETLOCAL\r\n\r\nSET \"NODE_EXE=%~dp0\\node.exe\"\r\nIF NOT EXIST \"%NODE_EXE%\" (\r\n  SET \"NODE_EXE=node\"\r\n)\r\n\r\nSET \"NPM_PREFIX_JS=%~dp0\\node_modules\\npm\\bin\\npm-prefix.js\"\r\nSET \"NPM_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npm-cli.js\"\r\nFOR /F \"delims=\" %%F IN ('CALL \"%NODE_EXE%\" \"%NPM_PREFIX_JS%\"') DO (\r\n  SET \"NPM_PREFIX_NPM_CLI_JS=%%F\\node_modules\\npm\\bin\\npm-cli.js\"\r\n)\r\nIF EXIST \"%NPM_PREFIX_NPM_CLI_JS%\" (\r\n  SET \"NPM_CLI_JS=%NPM_PREFIX_NPM_CLI_JS%\"\r\n)\r\n\r\n\"%NODE_EXE%\" \"%NPM_CLI_JS%\" %*\r\n";

    #[test]
    fn npm_shims() {
        let t = super::shim_target;
        let codex = script_shim("node", "node_modules\\@openai\\codex\\bin\\codex.js");
        assert_eq!(t(&codex), Some(("node_modules\\@openai\\codex\\bin\\codex.js".into(), true)));
        // npm.cmd: npm-cli.js, nicht das vorher genannte npm-prefix.js
        assert_eq!(t(NPM_CMD), Some(("node_modules\\npm\\bin\\npm-cli.js".into(), true)));
        assert_eq!(t(&NPM_CMD.replace("NPM_CLI_JS", "NPX_CLI_JS").replace("npm-cli.js", "npx-cli.js")), Some(("node_modules\\npm\\bin\\npx-cli.js".into(), true)));
        // Programm direkt (claude.exe), Skript ohne Endung über node (kilo)
        let claude = exe_shim("node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe");
        assert_eq!(t(&claude), Some(("node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe".into(), false)));
        let kilo = script_shim("node", "node_modules\\@kilocode\\cli\\bin\\kilo");
        assert_eq!(t(&kilo), Some(("node_modules\\@kilocode\\cli\\bin\\kilo".into(), true)));
        // anderes Programm als node (bun, sh): den Shim selbst starten
        assert_eq!(t(&script_shim("bun", "node_modules\\x\\cli.js")), None);
        assert_eq!(t(&script_shim("sh", "node_modules\\x\\run")), None);
        assert_eq!(t("echo hi"), None);
    }

    /// Shims auflösen wie unter Windows: Programm direkt, Skript mit dem Node.js von nrzX
    /// (`tools/node` neben dem npm-Prefix `tools/npm`).
    #[test]
    fn resolve_shims() {
        use super::Shim;
        let tmp = std::env::temp_dir().join(format!("nrzx-shims-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let npm = tmp.join("tools").join("npm");
        let file = |rel: &str, text: &str| {
            let p = rel.split('\\').fold(npm.clone(), |p, s| p.join(s));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            p
        };
        let exe = file("node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe", "MZ");
        let claude = file("claude.cmd", &exe_shim("node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe"));
        assert_eq!(super::resolve_shim(&claude), Some(Shim::Exe(exe)));
        let script = file("node_modules\\@kilocode\\cli\\bin\\kilo", "#!/usr/bin/env node");
        let kilo = file("kilo.cmd", &script_shim("node", "node_modules\\@kilocode\\cli\\bin\\kilo"));
        let node = tmp.join("tools").join("node").join("node.exe");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::write(&node, "MZ").unwrap();
        assert_eq!(super::resolve_shim(&kilo), Some(Shim::Node { node, script }));
        // Ziel fehlt oder kein Shim
        let gone = file("gone.cmd", &exe_shim("node_modules\\gone\\gone.exe"));
        assert_eq!(super::resolve_shim(&gone), None);
        assert_eq!(super::resolve_shim(&tmp.join("claude.exe")), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `security`-Ersatz mit einem nachgemachten `/usr/bin/security`, der nur mitschreibt
    /// (der echte wird nie aufgerufen – kein Eintrag im Schlüsselbund, kein Dialog).
    #[cfg(unix)]
    #[test]
    fn security_shim() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};
        let tmp = std::env::temp_dir().join(format!("nrzx-shim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        super::write_security_shim(&tmp).unwrap();
        let log = tmp.join("log");
        let fake = tmp.join("real-security");
        std::fs::write(&fake, "#!/bin/sh\necho \"$*\" >> \"$FAKE_LOG\"\n[ \"$1\" = default-keychain ] && exit \"${FAKE_DEFAULT:-1}\"\nexit 0\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |args: &[&str], has_default: bool| {
            let _ = std::fs::remove_file(&log);
            let mut child = Command::new(tmp.join("security"))
                .args(args)
                .env("NRZX_REAL_SECURITY", &fake)
                .env("FAKE_LOG", &log)
                .env("FAKE_DEFAULT", if has_default { "0" } else { "1" })
                .stdin(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            // wie Claude Code: Befehl über stdin (`security -i`)
            let _ = child.stdin.take().unwrap().write_all(b"add-generic-password -U -a u -s s -X 00\n");
            let code = child.wait().unwrap().code();
            (code, std::fs::read_to_string(&log).unwrap_or_default())
        };
        // Ohne Schlüsselbund: Schreiben scheitert still, nur die Prüfung läuft
        for args in [&["add-generic-password", "-U", "-a", "u", "-s", "s", "-X", "00"][..], &["-i"], &["-q", "add-internet-password", "-s", "h"], &["-p", "sec> "]] {
            assert_eq!(run(args, false), (Some(37), "default-keychain\n".to_string()), "{args:?}");
        }
        // Lesen, ausdrücklicher Schlüsselbund oder vorhandener Standard: durchreichen
        assert_eq!(run(&["find-generic-password", "-a", "u", "-w", "-s", "s"], false), (Some(0), "find-generic-password -a u -w -s s\n".to_string()));
        assert_eq!(run(&["add-generic-password", "-a", "u", "-s", "s", "-w", "p", "/tmp/x.keychain-db"], false).1, "add-generic-password -a u -s s -w p /tmp/x.keychain-db\n");
        assert_eq!(run(&["-p", "-i", "list-keychains"], false).1, "-p -i list-keychains\n");
        assert_eq!(run(&["add-generic-password", "-a", "u", "-s", "s"], true), (Some(0), "default-keychain\nadd-generic-password -a u -s s\n".to_string()));
        // Zweites Schreiben ändert nichts, veralteter Inhalt wird ersetzt
        super::write_security_shim(&tmp).unwrap();
        std::fs::write(tmp.join("security"), "alt").unwrap();
        super::write_security_shim(&tmp).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.join("security")).unwrap(), super::SECURITY_SHIM);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn names() {
        let n = super::program_names("claude");
        assert!(n.contains(&if cfg!(windows) { "claude.exe".to_string() } else { "claude".to_string() }));
    }
}
