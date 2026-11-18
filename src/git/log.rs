//! Commits lesen: `git log`/`git show` mit eigenem Format.

use crate::error::AppResult;
use crate::AppState;
use nrzx_shared::{Commit, Signature};
use std::path::Path;

/// Format für [`commits`]: Felder getrennt mit 0x1f, Commits mit 0x1e.
pub const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%B%x1e";

/// `git log` mit beliebigen weiteren Argumenten (z. B. `["-n", "30", "main", "--", "src"]`).
/// Das Format setzt diese Funktion selbst.
pub async fn commits(state: &AppState, dir: &Path, args: &[&str]) -> AppResult<Vec<Commit>> {
    let mut all = vec!["log", FORMAT];
    all.extend_from_slice(args);
    let out = crate::git::run_with(state, dir, &all, None, &[]).await?;
    if !out.ok() {
        // Unbekannte Revision, leeres Repository …
        return Ok(Vec::new());
    }
    Ok(parse(&out.stdout_str()))
}

/// Einen Commit lesen.
pub async fn commit(state: &AppState, dir: &Path, rev: &str) -> AppResult<Option<Commit>> {
    if rev.starts_with('-') {
        return Ok(None);
    }
    Ok(commits(state, dir, &["-n", "1", rev, "--"]).await?.into_iter().next())
}

pub fn parse(text: &str) -> Vec<Commit> {
    text.split('\u{1e}')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            if rec.trim().is_empty() {
                return None;
            }
            let f: Vec<&str> = rec.splitn(9, '\u{1f}').collect();
            if f.len() < 9 {
                return None;
            }
            let message = f[8].trim_end().to_string();
            let summary = message.lines().next().unwrap_or("").to_string();
            Some(Commit {
                sha: f[0].to_string(),
                short_sha: f[0].chars().take(10).collect(),
                summary,
                message,
                author: Signature { name: f[2].into(), email: f[3].into(), when: f[4].parse().unwrap_or(0), user: None },
                committer: Signature { name: f[5].into(), email: f[6].into(), when: f[7].parse().unwrap_or(0), user: None },
                parents: f[1].split_whitespace().map(str::to_string).collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_log_records() {
        let text = "abc\u{1f}p1 p2\u{1f}Ann\u{1f}ann@x\u{1f}10\u{1f}Bob\u{1f}bob@x\u{1f}11\u{1f}Titel\n\nText\n\u{1e}\n";
        let c = super::parse(text);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].summary, "Titel");
        assert_eq!(c[0].message, "Titel\n\nText");
        assert_eq!(c[0].parents, vec!["p1", "p2"]);
        assert_eq!(c[0].author.when, 10);
    }
}
