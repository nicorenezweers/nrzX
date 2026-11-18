//! Syntaxhervorhebung mit syntect (reines Rust, fancy-regex). Ausgabe sind `<span>` mit
//! Klassen `hl-…`; die Farben liefert `/static/highlight.css` (dunkles und helles Thema),
//! erzeugt aus den syntect-Themen beim Start.

use regex::Regex;
use std::sync::LazyLock;
use syntect::highlighting::ThemeSet;
use syntect::html::{css_for_theme_with_class_style, line_tokens_to_classed_spans, ClassStyle};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

pub const CLASS_STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "hl-" };

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// Dateiendungen ohne eigene syntect-Syntax → ähnliche Syntax.
const ALIASES: &[(&str, &str)] = &[
    ("ts", "js"),
    ("tsx", "js"),
    ("jsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("vue", "html"),
    ("svelte", "html"),
    ("toml", "ini"),
    ("cfg", "ini"),
    ("conf", "ini"),
    ("lock", "ini"),
    ("kts", "java"),
    ("kt", "java"),
    ("swift", "rs"),
    ("zig", "rs"),
    ("dockerfile", "sh"),
    ("containerfile", "sh"),
    ("zsh", "sh"),
    ("fish", "sh"),
    ("ps1", "sh"),
    ("nix", "sh"),
    ("scss", "css"),
    ("sass", "css"),
    ("less", "css"),
    ("jsonc", "json"),
    ("json5", "json"),
    ("hcl", "ini"),
    ("tf", "ini"),
    ("gradle", "groovy"),
    ("cmake", "sh"),
    ("vb", "cs"),
    ("fs", "ml"),
    ("ex", "rb"),
    ("exs", "rb"),
    ("dart", "java"),
    ("proto", "go"),
    ("sol", "js"),
];

/// Syntax zu Dateiname (Endung, besondere Namen) oder erster Zeile (Shebang).
pub fn syntax_for(filename: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let ss = &*SYNTAXES;
    let base = filename.rsplit('/').next().unwrap_or(filename);
    let lower = base.to_lowercase();
    let ext = match lower.as_str() {
        "dockerfile" | "containerfile" => "sh".to_string(),
        "makefile" | "gnumakefile" => "makefile".to_string(),
        "cargo.lock" | ".gitconfig" | ".editorconfig" => "ini".to_string(),
        "cmakelists.txt" => "sh".to_string(),
        _ => lower.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default(),
    };
    let ext = ALIASES.iter().find(|(a, _)| *a == ext).map(|(_, b)| b.to_string()).unwrap_or(ext);
    if !ext.is_empty() {
        if let Some(s) = ss.find_syntax_by_extension(&ext) {
            return Some(s);
        }
    }
    ss.find_syntax_by_first_line(first_line)
}

/// Syntax zu einer Sprachangabe in Markdown (```rust, ```py …).
pub fn syntax_for_lang(lang: &str) -> Option<&'static SyntaxReference> {
    let ss = &*SYNTAXES;
    let l = lang.trim().to_lowercase();
    if l.is_empty() {
        return None;
    }
    let l = match l.as_str() {
        "rust" => "rs",
        "python" => "py",
        "javascript" | "typescript" | "ts" | "tsx" | "jsx" => "js",
        "shell" | "bash" | "zsh" | "console" | "sh" | "dockerfile" => "sh",
        "yml" => "yaml",
        "c++" | "cpp" => "cpp",
        "c#" | "csharp" => "cs",
        "golang" => "go",
        "ruby" => "rb",
        "toml" | "ini" => "ini",
        "markdown" => "md",
        "kotlin" | "kt" => "java",
        "diff" | "patch" => "diff",
        other => other,
    };
    ss.find_syntax_by_token(l).or_else(|| ss.find_syntax_by_extension(l))
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn open_spans(stack: &ScopeStack) -> String {
    let mut out = String::new();
    for scope in stack.as_slice() {
        let classes: Vec<String> = scope.build_string().split('.').map(|a| format!("hl-{a}")).collect();
        out.push_str(&format!("<span class=\"{}\">", classes.join(" ")));
    }
    out
}

/// Text Zeile für Zeile hervorheben; jede Zeile ist für sich gültiges HTML (offene Spans werden
/// am Zeilenende geschlossen und in der nächsten Zeile wieder geöffnet). Ohne passende Syntax
/// nur maskiert.
pub fn highlight_lines(syntax: Option<&SyntaxReference>, text: &str) -> Vec<String> {
    let Some(syntax) = syntax else {
        return text.lines().map(escape).collect();
    };
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        let prefix = open_spans(&stack);
        let html = match state.parse_line(line, &SYNTAXES) {
            Ok(ops) => match line_tokens_to_classed_spans(line, &ops, CLASS_STYLE, &mut stack) {
                Ok((h, _)) => h,
                Err(_) => escape(line),
            },
            Err(_) => escape(line),
        };
        let mut l = prefix + &html;
        for _ in 0..stack.len() {
            l.push_str("</span>");
        }
        out.push(l.trim_end_matches(['\n', '\r']).to_string().replace("\n</span>", "</span>"));
    }
    out
}

/// Datei hervorheben (Syntax aus dem Namen).
pub fn highlight_file(filename: &str, text: &str) -> Vec<String> {
    let first = text.lines().next().unwrap_or("");
    highlight_lines(syntax_for(filename, first), text)
}

/// Codeblock (Markdown) als HTML-Zeilen, mit `\n` verbunden.
pub fn highlight_block(lang: &str, code: &str) -> String {
    highlight_lines(syntax_for_lang(lang), code).join("\n")
}

/// Name der Sprache einer Datei (für Statistiken), z. B. "Rust", "Python".
pub fn language_name(filename: &str) -> Option<String> {
    let s = syntax_for(filename, "")?;
    match s.name.as_str() {
        "Plain Text" => None,
        n => Some(n.to_string()),
    }
}

static SELECTOR_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)(^|,\s*)\.hl-").expect("regex"));

/// CSS für die `hl-`-Klassen: dunkel als Standard, hell unter `html.light`.
pub fn css() -> String {
    let themes = ThemeSet::load_defaults();
    let dark = themes
        .themes
        .get("base16-ocean.dark")
        .and_then(|t| css_for_theme_with_class_style(t, CLASS_STYLE).ok())
        .unwrap_or_default();
    let light = themes
        .themes
        .get("InspiredGitHub")
        .and_then(|t| css_for_theme_with_class_style(t, CLASS_STYLE).ok())
        .unwrap_or_default();
    let light = SELECTOR_START.replace_all(&light, "${1}html.light .hl-");
    // Hintergründe der Themen nicht übernehmen – die Oberfläche hat eigene Flächen
    let strip_bg = |css: &str| css.lines().filter(|l| !l.trim_start().starts_with("background-color")).collect::<Vec<_>>().join("\n");
    format!("/* nrzGit – Syntaxfarben (syntect) */\n{}\n{}\n", strip_bg(&dark), strip_bg(&light))
}

#[cfg(test)]
mod tests {
    #[test]
    fn lines_are_balanced() {
        let lines = super::highlight_file("x.rs", "/* a\nb */\nfn main() {}\n");
        assert_eq!(lines.len(), 3);
        for l in &lines {
            assert_eq!(l.matches("<span").count(), l.matches("</span>").count(), "{l}");
        }
        assert!(super::highlight_file("x.unknown", "<b>").contains(&"&lt;b&gt;".to_string()));
    }
}
