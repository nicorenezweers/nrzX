//! Bearbeitungen der Agenten genau zeigen: Vor einer Bearbeitung merkt sich der Lauf den Inhalt der
//! Datei, danach schneidet er aus alt und neu die geänderten Abschnitte mit Zeilennummern und drei
//! Zeilen Kontext (`EditDiff::line`, `context_before`/`context_after`) – der Chat zeigt sie als
//! Code-Block rot/grün.
//!
//! Alter Inhalt, in dieser Reihenfolge: Stand beim Ankündigen des Werkzeugs (Claude, Gemini, …),
//! aus dem neuen Inhalt zurückgerechnet (Bearbeiten mit altem/neuem Text), zuletzt in diesem Lauf
//! gesehen, sonst der letzte Commit (Codex meldet Änderungen erst, wenn sie geschehen sind).

use nrzx_shared::agents::{EditDiff, ToolCall};
use nrzx_shared::diff::hunks;
use std::path::{Path, PathBuf};

/// Grösste Datei, die gelesen wird.
const MAX_FILE: u64 = 1024 * 1024;
/// Höchstens so viele Abschnitte je Datei, Zeilen je Seite eines Abschnitts und Zeichen je Zeile.
const MAX_HUNKS: usize = 12;
const MAX_LINES: usize = 160;
const MAX_CHARS: usize = 400;
const CONTEXT: usize = 3;

/// Dateien, die ein Werkzeug bearbeitet (absolut).
pub fn paths(call: &ToolCall, workdir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: &str| {
        let p = p.trim();
        if p.is_empty() {
            return;
        }
        let pb = Path::new(p);
        let abs = if pb.is_absolute() { pb.to_path_buf() } else { workdir.join(pb) };
        if !out.contains(&abs) {
            out.push(abs);
        }
    };
    if let Some(list) = call.input["changes"].as_array() {
        for c in list {
            add(c["path"].as_str().unwrap_or(""));
        }
    }
    for k in ["file_path", "filePath", "absolute_path", "path", "notebook_path", "file"] {
        if let Some(p) = call.input[k].as_str() {
            add(p);
            break;
        }
    }
    out
}

/// Inhalt einer Datei: `Some(None)` = gibt es (noch) nicht, `None` = nicht lesbar (zu gross, kein
/// Text, kein Zugriff).
pub async fn snapshot(p: &Path) -> Option<Option<String>> {
    match tokio::fs::metadata(p).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
        Ok(m) if !m.is_file() || m.len() > MAX_FILE => None,
        Ok(_) => tokio::fs::read(p).await.ok().and_then(|b| String::from_utf8(b).ok()).map(Some),
    }
}

/// Alten Inhalt aus dem neuen zurückrechnen (eine Bearbeitung mit altem/neuem Text): `after` an
/// seiner ersten Stelle wieder durch `before` ersetzen. Gekürzte Texte taugen dafür nicht.
pub fn reconstruct(new: &str, diffs: &[EditDiff]) -> Option<String> {
    let [d] = diffs else { return None };
    let cut = |t: &str| t.ends_with('…') && t.chars().count() > 4000;
    if d.after.is_empty() || cut(&d.after) || cut(&d.before) || d.before == d.after {
        return None;
    }
    let at = new.find(&d.after)?;
    Some(format!("{}{}{}", &new[..at], d.before, &new[at + d.after.len()..]))
}

/// Inhalt im letzten Commit (`Some("")`: Datei ist neu; `None`: kein Repository, kein Commit).
pub async fn git_head(workdir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(workdir).ok()?;
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .arg("show")
        .arg(format!("HEAD:{}", rel.to_string_lossy()))
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if out.status.success() {
        return String::from_utf8(out.stdout).ok();
    }
    let err = String::from_utf8_lossy(&out.stderr);
    (err.contains("exists on disk, but not in") || err.contains("does not exist in")).then(String::new)
}

/// Pfad wie im Chat (relativ zum Arbeitsordner, sonst absolut).
pub fn shown_path(path: &Path, workdirs: &[String]) -> String {
    for w in workdirs {
        if let Ok(rel) = path.strip_prefix(w.trim_end_matches('/')) {
            let r = rel.to_string_lossy();
            if !r.is_empty() {
                return r.into_owned();
            }
        }
    }
    path.to_string_lossy().into_owned()
}

fn clip_line(l: &str) -> String {
    if l.chars().count() > MAX_CHARS {
        format!("{}…", l.chars().take(MAX_CHARS).collect::<String>())
    } else {
        l.to_string()
    }
}

/// Geänderte Abschnitte von `old` nach `new` als Diffs für den Chat (leer: nichts geändert).
pub fn region_diffs(path: &str, old: &str, new: &str) -> Vec<EditDiff> {
    let all = hunks(old, new, CONTEXT);
    let join = |lines: &[String]| lines.iter().take(MAX_LINES).map(|l| clip_line(l)).collect::<Vec<_>>().join("\n");
    let mut out: Vec<EditDiff> = all
        .iter()
        .take(MAX_HUNKS)
        .map(|h| EditDiff {
            path: path.to_string(),
            before: join(&h.before),
            after: join(&h.after),
            adds: h.adds as i64,
            dels: h.dels as i64,
            line: h.new_start as i64 + 1,
            old_line: h.old_start as i64 + 1,
            context_before: h.context_before.iter().map(|l| clip_line(l)).collect(),
            context_after: h.context_after.iter().map(|l| clip_line(l)).collect(),
            truncated: h.before.len() > MAX_LINES || h.after.len() > MAX_LINES,
        })
        .collect();
    // Weitere Abschnitte nur noch mitzählen
    if all.len() > MAX_HUNKS {
        let (adds, dels) = all[MAX_HUNKS..].iter().fold((0, 0), |(a, d), h| (a + h.adds as i64, d + h.dels as i64));
        out.push(EditDiff { path: path.to_string(), adds, dels, truncated: true, ..Default::default() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_from_inputs() {
        let w = Path::new("/w");
        let call = ToolCall { input: json!({"file_path": "/w/src/a.rs"}), ..Default::default() };
        assert_eq!(paths(&call, w), vec![PathBuf::from("/w/src/a.rs")]);
        let call = ToolCall { input: json!({"changes": [{"path": "src/b.rs", "kind": "update"}, {"path": "/w/c.rs"}]}), ..Default::default() };
        assert_eq!(paths(&call, w), vec![PathBuf::from("/w/src/b.rs"), PathBuf::from("/w/c.rs")]);
    }

    #[test]
    fn reconstruct_and_regions() {
        let new = "a\nb\nNEU\nd\ne\n";
        let d = EditDiff { before: "c".into(), after: "NEU".into(), ..Default::default() };
        let old = reconstruct(new, &[d]).unwrap();
        assert_eq!(old, "a\nb\nc\nd\ne\n");
        let r = region_diffs("x.txt", &old, new);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].line, r[0].old_line, r[0].adds, r[0].dels), (3, 3, 1, 1));
        assert_eq!(r[0].context_before, vec!["a", "b"]);
        assert_eq!(r[0].context_after, vec!["d", "e"]);
        assert_eq!((r[0].before.as_str(), r[0].after.as_str()), ("c", "NEU"));
        // Neue Datei: alles hinzugefügt ab Zeile 1
        let r = region_diffs("n.txt", "", "x\ny\n");
        assert_eq!((r[0].line, r[0].adds, r[0].dels), (1, 2, 0));
        assert!(region_diffs("s.txt", "same", "same").is_empty());
    }
}
