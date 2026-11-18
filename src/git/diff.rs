//! Unified Diffs (`git diff`, `git show`) in Dateien, Abschnitte und Zeilen zerlegen.

use crate::error::AppResult;
use crate::AppState;
use nrzx_shared::{Diff, DiffFile, DiffHunk, DiffLine, DiffLineKind};
use std::path::Path;

/// Grenzen, damit riesige Diffs die Oberfläche nicht lahmlegen.
pub const MAX_FILES: usize = 300;
pub const MAX_LINES_PER_FILE: usize = 5_000;

/// `git diff` mit weiteren Argumenten ausführen und parsen, z. B. `["base...head"]` oder
/// `["sha^", "sha"]`. Erkennt Umbenennungen (`-M`).
pub async fn run(state: &AppState, dir: &Path, args: &[&str]) -> AppResult<Diff> {
    let mut all = vec!["diff", "--no-color", "--no-ext-diff", "-M", "-p"];
    all.extend_from_slice(args);
    let out = crate::git::run(state, dir, &all).await?;
    Ok(parse(&String::from_utf8_lossy(&out)))
}

/// Diff eines Commits gegen seinen ersten Elternteil (Wurzel-Commit: gegen leer).
pub async fn commit(state: &AppState, dir: &Path, sha: &str) -> AppResult<Diff> {
    let out = crate::git::run(
        state,
        dir,
        &["show", "--no-color", "--no-ext-diff", "-M", "-p", "--first-parent", "--format=", "-m", sha, "--"],
    )
    .await?;
    Ok(parse(&String::from_utf8_lossy(&out)))
}

pub fn parse(text: &str) -> Diff {
    let mut diff = Diff::default();
    let mut file: Option<DiffFile> = None;
    let mut hunk: Option<DiffHunk> = None;
    let (mut old_no, mut new_no) = (0i64, 0i64);
    let mut lines_in_file = 0usize;

    let finish_hunk = |file: &mut Option<DiffFile>, hunk: &mut Option<DiffHunk>| {
        if let (Some(f), Some(h)) = (file.as_mut(), hunk.take()) {
            f.hunks.push(h);
        }
    };

    for line in text.split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            finish_hunk(&mut file, &mut hunk);
            if let Some(f) = file.take() {
                push_file(&mut diff, f);
            }
            let (a, b) = split_git_header(rest);
            file = Some(DiffFile { old_path: a, new_path: b, status: "modified".into(), ..Default::default() });
            lines_in_file = 0;
            continue;
        }
        let Some(f) = file.as_mut() else { continue };
        if hunk.is_none() || line.starts_with("@@") {
            // Kopfzeilen der Datei
            if let Some(m) = line.strip_prefix("new file mode ") {
                f.status = "added".into();
                f.new_mode = m.trim().into();
                continue;
            }
            if let Some(m) = line.strip_prefix("deleted file mode ") {
                f.status = "deleted".into();
                f.old_mode = m.trim().into();
                continue;
            }
            if let Some(m) = line.strip_prefix("old mode ") {
                f.old_mode = m.trim().into();
                continue;
            }
            if let Some(m) = line.strip_prefix("new mode ") {
                f.new_mode = m.trim().into();
                continue;
            }
            if let Some(p) = line.strip_prefix("rename from ") {
                f.status = "renamed".into();
                f.old_path = p.into();
                continue;
            }
            if let Some(p) = line.strip_prefix("rename to ") {
                f.status = "renamed".into();
                f.new_path = p.into();
                continue;
            }
            if let Some(p) = line.strip_prefix("copy from ") {
                f.status = "copied".into();
                f.old_path = p.into();
                continue;
            }
            if let Some(p) = line.strip_prefix("copy to ") {
                f.status = "copied".into();
                f.new_path = p.into();
                continue;
            }
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                f.binary = true;
                continue;
            }
            if let Some(p) = line.strip_prefix("--- ") {
                if p != "/dev/null" {
                    f.old_path = strip_prefix_path(p, "a/");
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("+++ ") {
                if p != "/dev/null" {
                    f.new_path = strip_prefix_path(p, "b/");
                }
                continue;
            }
            if line.starts_with("@@") {
                finish_hunk(&mut file, &mut hunk);
                let h = parse_hunk_header(line);
                old_no = h.old_start;
                new_no = h.new_start;
                hunk = Some(h);
                continue;
            }
            continue;
        }
        // Zeilen eines Abschnitts
        let h = hunk.as_mut().expect("hunk");
        let f = file.as_mut().expect("file");
        let (kind, content) = match line.as_bytes().first() {
            Some(b'+') => (DiffLineKind::Add, &line[1..]),
            Some(b'-') => (DiffLineKind::Del, &line[1..]),
            Some(b' ') => (DiffLineKind::Context, &line[1..]),
            Some(b'\\') => (DiffLineKind::Meta, line),
            // Leere Zeile am Ende der Ausgabe oder Zeile ohne Präfix
            _ => continue,
        };
        match kind {
            DiffLineKind::Add => f.additions += 1,
            DiffLineKind::Del => f.deletions += 1,
            _ => {}
        }
        lines_in_file += 1;
        if lines_in_file > MAX_LINES_PER_FILE {
            f.too_large = true;
            continue;
        }
        let (old_line, new_line) = match kind {
            DiffLineKind::Add => {
                new_no += 1;
                (None, Some(new_no - 1))
            }
            DiffLineKind::Del => {
                old_no += 1;
                (Some(old_no - 1), None)
            }
            DiffLineKind::Context => {
                old_no += 1;
                new_no += 1;
                (Some(old_no - 1), Some(new_no - 1))
            }
            DiffLineKind::Meta => (None, None),
        };
        h.lines.push(DiffLine { kind, old_line, new_line, content: content.to_string(), html: None });
    }
    finish_hunk(&mut file, &mut hunk);
    if let Some(f) = file.take() {
        push_file(&mut diff, f);
    }
    diff
}

fn push_file(diff: &mut Diff, mut f: DiffFile) {
    diff.additions += f.additions;
    diff.deletions += f.deletions;
    if f.too_large {
        f.hunks.clear();
    }
    if diff.files.len() >= MAX_FILES {
        diff.truncated = true;
        return;
    }
    diff.files.push(f);
}

/// `a/x b/x` → (x, x). Bei Leerzeichen im Pfad ist die Zeile mehrdeutig – ohne Umbenennung
/// sind beide Pfade gleich, also die Mitte nehmen; Umbenennungen korrigieren `rename from/to`.
fn split_git_header(rest: &str) -> (String, String) {
    let rest = rest.trim();
    if rest.starts_with('"') {
        // Gequotete Pfade (Sonderzeichen): grob zerlegen, ---/+++ korrigieren
        let parts: Vec<&str> = rest.split("\" \"").collect();
        if parts.len() == 2 {
            return (
                strip_prefix_path(parts[0].trim_start_matches('"'), "a/"),
                strip_prefix_path(parts[1].trim_end_matches('"'), "b/"),
            );
        }
    }
    let len = rest.len();
    if len >= 5 && (len - 1) % 2 == 0 {
        let half = (len - 1) / 2;
        if rest.is_char_boundary(half) && rest.is_char_boundary(half + 1) {
            let (a, b) = (&rest[..half], &rest[half + 1..]);
            if let (Some(a2), Some(b2)) = (a.strip_prefix("a/"), b.strip_prefix("b/")) {
                if a2 == b2 {
                    return (a2.to_string(), b2.to_string());
                }
            }
        }
    }
    match rest.split_once(" b/") {
        Some((a, b)) => (a.trim_start_matches("a/").to_string(), b.to_string()),
        None => (rest.to_string(), rest.to_string()),
    }
}

fn strip_prefix_path(p: &str, prefix: &str) -> String {
    let p = p.trim_end_matches('\t');
    let p = if p.starts_with('"') && p.ends_with('"') && p.len() >= 2 { &p[1..p.len() - 1] } else { p };
    p.strip_prefix(prefix).unwrap_or(p).to_string()
}

/// `@@ -1,4 +1,6 @@ fn main()`
fn parse_hunk_header(line: &str) -> DiffHunk {
    let mut h = DiffHunk { header: line.to_string(), ..Default::default() };
    let inner = line.trim_start_matches('@').trim_start();
    let mut parts = inner.split_whitespace();
    let range = |s: Option<&str>, sign: char| -> (i64, i64) {
        let s = s.unwrap_or("").trim_start_matches(sign);
        let mut it = s.split(',');
        let start = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let count = it.next().and_then(|x| x.parse().ok()).unwrap_or(1);
        (start, count)
    };
    (h.old_start, h.old_lines) = range(parts.next(), '-');
    (h.new_start, h.new_lines) = range(parts.next(), '+');
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/src/main.rs b/src/main.rs
index 83db48f..bf269f4 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@ fn x
 fn main() {
-    println!(\"a\");
+    println!(\"b\");
+    println!(\"c\");
 }
diff --git a/my file.txt b/my file.txt
new file mode 100644
index 0000000..e69de29
--- /dev/null
+++ b/my file.txt
@@ -0,0 +1 @@
+hallo
\\ No newline at end of file
diff --git a/old.txt b/new.txt
similarity index 100%
rename from old.txt
rename to new.txt
diff --git a/logo.png b/logo.png
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn parses_files_hunks_and_lines() {
        let d = parse(SAMPLE);
        assert_eq!(d.files.len(), 4);
        let f = &d.files[0];
        assert_eq!(f.new_path, "src/main.rs");
        assert_eq!((f.additions, f.deletions), (2, 1));
        assert_eq!(f.hunks[0].lines.len(), 5);
        assert_eq!(f.hunks[0].lines[2].new_line, Some(2));
        assert_eq!(f.hunks[0].lines[1].old_line, Some(2));
        assert_eq!(d.files[1].status, "added");
        assert_eq!(d.files[1].new_path, "my file.txt");
        assert_eq!(d.files[1].hunks[0].lines[1].kind, DiffLineKind::Meta);
        assert_eq!(d.files[2].status, "renamed");
        assert_eq!((d.files[2].old_path.as_str(), d.files[2].new_path.as_str()), ("old.txt", "new.txt"));
        assert!(d.files[3].binary);
        assert_eq!((d.additions, d.deletions), (3, 1));
    }
}
