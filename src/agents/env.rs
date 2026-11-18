//! Umgebung der Agent-Prozesse und Terminals: Die Umgebung des Servers wird nicht vererbt
//! (sonst sähe jeder Agent z. B. einen API-Schlüssel des Hosts), nur eine kleine Auswahl
//! (Sprache, Zeitzone, Zertifikate, Proxy, Docker-Socket). `PATH` beginnt mit den von nrzX
//! installierten Werkzeugen (`tools/bin`, `tools/npm/bin`, `tools/node`).

use crate::AppState;
use std::path::PathBuf;

/// Variablen des Servers, die Agenten erben dürfen.
const INHERIT: &[&str] = &[
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "USER",
    "LOGNAME",
    "SHELL",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "DOCKER_HOST",
    "DOCKER_CONFIG",
];

/// `tools/bin`, `tools/npm/bin` und das Node.js von nrzX (von nrzX installierte Programme).
pub fn tool_dirs(state: &AppState) -> Vec<PathBuf> {
    let tools = state.config.tools_dir();
    // npm legt unter Windows die Programme direkt in den Prefix
    let npm = if cfg!(windows) { tools.join("npm") } else { tools.join("npm").join("bin") };
    let node = super::tools::own_node_dir(&tools);
    vec![tools.join("bin"), npm, node]
}

/// `PATH` für Agenten: eigene Werkzeuge zuerst, dann der `PATH` des Servers.
pub fn path_var(state: &AppState) -> String {
    path_var_with(state, &[])
}

/// Wie [`path_var`], mit `extra` zwischen den eigenen Werkzeugen und dem `PATH` des Servers.
pub fn path_var_with(state: &AppState, extra: &[PathBuf]) -> String {
    let mut parts: Vec<PathBuf> = tool_dirs(state);
    parts.extend(extra.iter().cloned());
    if let Some(p) = std::env::var_os("PATH") {
        parts.extend(std::env::split_paths(&p));
    } else {
        parts.extend(["/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    }
    std::env::join_paths(parts).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Grundumgebung (ohne Konto): Auswahl des Servers plus `PATH`.
pub fn base(state: &AppState) -> Vec<(String, String)> {
    let windows: &[&str] = if cfg!(windows) { crate::platform::WINDOWS_INHERIT } else { &[] };
    let mut env: Vec<(String, String)> = INHERIT
        .iter()
        .chain(windows.iter())
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    if !env.iter().any(|(k, _)| k == "LANG" || k == "LC_ALL") {
        env.push(("LANG".into(), "C.UTF-8".into()));
    }
    env.push(("PATH".into(), path_var(state)));
    // Ohne Konto: das Home des Servers (z. B. für Terminals ohne Konto)
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        env.extend(crate::platform::home_vars(std::path::Path::new(&home)));
    }
    env
}

/// Grundumgebung auf einen Befehl anwenden (vorher alles löschen).
pub fn apply_base(cmd: &mut tokio::process::Command, state: &AppState) {
    cmd.env_clear();
    for (k, v) in base(state) {
        cmd.env(k, v);
    }
}

/// Variablen zusammenführen: spätere überschreiben frühere.
pub fn merge(lists: Vec<Vec<(String, String)>>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for list in lists {
        for (k, v) in list {
            match out.iter_mut().find(|(x, _)| *x == k) {
                Some(slot) => slot.1 = v,
                None => out.push((k, v)),
            }
        }
    }
    out
}
