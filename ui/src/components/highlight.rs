//! Kleine Syntax-Hervorhebung für Code-Blöcke im Chat (Bearbeitungen der Agenten): je Zeile
//! Schlüsselwörter, Texte, Zahlen, Kommentare, Funktionsaufrufe und Typen – genug für kurze
//! Ausschnitte, ohne Grammatiken. Mehrzeilige Kommentare und Texte werden nicht verfolgt.

/// Sprachfamilie (aus der Dateiendung).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang {
    Rust,
    Js,
    Py,
    Go,
    CLike,
    Shell,
    Sql,
    Css,
    Markup,
    Data,
    Plain,
}

pub fn lang_of(path: &str) -> Lang {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let name = path.rsplit('/').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" => Lang::Rust,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" => Lang::Js,
        "py" | "pyi" | "rb" => Lang::Py,
        "go" => Lang::Go,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "cs" | "java" | "kt" | "kts" | "swift" | "scala" | "dart" | "m" | "mm" | "php" | "zig" => Lang::CLike,
        "sh" | "bash" | "zsh" | "fish" | "ps1" => Lang::Shell,
        "sql" => Lang::Sql,
        "css" | "scss" | "sass" | "less" => Lang::Css,
        "html" | "htm" | "xml" | "svg" | "xhtml" => Lang::Markup,
        "json" | "jsonc" | "toml" | "yaml" | "yml" | "ini" | "env" | "lock" => Lang::Data,
        _ if name == "dockerfile" || name == "makefile" => Lang::Shell,
        _ => Lang::Plain,
    }
}

const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
    "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "Some", "None",
    "Ok", "Err",
];
const JS: &[&str] = &[
    "abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "constructor", "continue", "debugger", "declare", "default", "delete", "do", "else",
    "enum", "export", "extends", "false", "finally", "for", "from", "function", "get", "if", "implements", "import", "in", "instanceof", "interface", "let", "new",
    "null", "of", "private", "protected", "public", "readonly", "return", "set", "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof",
    "undefined", "var", "void", "while", "with", "yield",
];
const PY: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "False", "finally", "for", "from", "global", "if",
    "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "self", "True", "try", "while", "with", "yield", "end", "do", "then",
    "unless", "module", "require", "puts", "nil",
];
const GO: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package",
    "range", "return", "select", "struct", "switch", "type", "var", "nil", "true", "false",
];
const CLIKE: &[&str] = &[
    "auto", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else", "enum", "extends", "extern", "false", "final", "for", "fun",
    "func", "guard", "if", "implements", "import", "in", "interface", "internal", "is", "let", "namespace", "new", "null", "nullptr", "object", "override", "package",
    "private", "protected", "public", "return", "self", "sizeof", "static", "struct", "super", "switch", "this", "throw", "throws", "true", "try", "typedef", "union",
    "using", "val", "var", "virtual", "void", "volatile", "when", "while", "where", "int", "long", "char", "float", "double", "bool", "boolean", "unsigned", "signed",
    "short", "async", "await", "function", "echo", "fn", "defer", "pub",
];
const SHELL: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "function", "in", "return", "export", "local", "readonly", "set", "unset",
    "echo", "exit", "source", "cd", "FROM", "RUN", "COPY", "CMD", "ENV", "WORKDIR", "EXPOSE", "ENTRYPOINT", "ARG", "ADD",
];
const SQL: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table", "index", "drop", "alter", "add", "column", "join", "left",
    "right", "inner", "outer", "on", "and", "or", "not", "null", "primary", "key", "references", "default", "as", "order", "by", "group", "having", "limit", "offset",
    "distinct", "union", "case", "when", "then", "else", "end", "exists", "in", "is", "integer", "text", "real", "blob", "unique", "if",
];
const DATA: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];

fn keywords(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => RUST,
        Lang::Js => JS,
        Lang::Py => PY,
        Lang::Go => GO,
        Lang::CLike => CLIKE,
        Lang::Shell => SHELL,
        Lang::Sql => SQL,
        Lang::Data | Lang::Css => DATA,
        Lang::Markup | Lang::Plain => &[],
    }
}

/// Zeilenkommentar der Sprache.
fn line_comment(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust | Lang::Js | Lang::Go | Lang::CLike => &["//"],
        Lang::Py | Lang::Shell => &["#"],
        Lang::Data => &["#", "//"],
        Lang::Sql => &["--"],
        Lang::Css | Lang::Markup | Lang::Plain => &[],
    }
}

/// Teile einer Zeile mit CSS-Klasse (`""` = normal).
pub fn highlight(line: &str, lang: Lang) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let push = |out: &mut Vec<(&'static str, String)>, class: &'static str, text: String| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.0 == class {
                last.1.push_str(&text);
                return;
            }
        }
        out.push((class, text));
    };
    if lang == Lang::Plain {
        return vec![("", line.to_string())];
    }
    let trimmed = line.trim_start();
    // Zeilen in Block-Kommentaren (Heuristik: „/*“, „*“, „*/“ am Anfang; HTML „<!--“)
    if trimmed.starts_with("/*") || trimmed.starts_with("*/") || (trimmed.starts_with('*') && !trimmed.starts_with("**") && lang != Lang::Markup && lang != Lang::Data)
        || trimmed.starts_with("<!--")
        || (lang == Lang::Py && (trimmed.starts_with("\"\"\"") || trimmed.starts_with("'''")))
    {
        return vec![("tok-com", line.to_string())];
    }
    let kws = keywords(lang);
    let comments = line_comment(lang);
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rest: String = chars[i..].iter().take(3).collect();
        // Kommentar bis zum Zeilenende
        if comments.iter().any(|p| rest.starts_with(p)) && (lang != Lang::Shell || i == 0 || chars[i - 1].is_whitespace()) {
            push(&mut out, "tok-com", chars[i..].iter().collect());
            break;
        }
        // Text in Anführungszeichen
        if c == '"' || c == '\'' || c == '`' {
            // Rust: 'a (Lebensdauer) nicht als Text
            if c == '\'' && lang == Lang::Rust && !chars[i + 1..].iter().take(3).any(|x| *x == '\'') {
                push(&mut out, "tok-kw", c.to_string());
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                if chars[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            let end = (j + 1).min(chars.len());
            push(&mut out, "tok-str", chars[i..end].iter().collect());
            i = end;
            continue;
        }
        // Zahl
        if c.is_ascii_digit() && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_') {
                j += 1;
            }
            push(&mut out, "tok-num", chars[i..j].iter().collect());
            i = j;
            continue;
        }
        // Wort
        if c.is_alphabetic() || c == '_' || c == '$' || c == '@' {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '$' || (lang == Lang::Css && chars[j] == '-')) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let next = chars[j..].iter().find(|x| !x.is_whitespace()).copied();
            let is_kw = if lang == Lang::Sql { kws.contains(&word.to_lowercase().as_str()) } else { kws.contains(&word.as_str()) };
            let class = if is_kw {
                "tok-kw"
            } else if lang == Lang::Markup && i > 0 && (chars[i - 1] == '<' || chars[i - 1] == '/') {
                "tok-kw"
            } else if next == Some('(') || (lang == Lang::Rust && next == Some('!')) {
                "tok-fn"
            } else if word.chars().next().is_some_and(|f| f.is_uppercase()) && word.chars().any(|x| x.is_lowercase()) {
                "tok-ty"
            } else if lang == Lang::Data && matches!(next, Some(':') | Some('=')) {
                "tok-fn"
            } else {
                ""
            };
            push(&mut out, class, word);
            i = j;
            continue;
        }
        push(&mut out, "", c.to_string());
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(line: &str, lang: Lang) -> Vec<(&'static str, String)> {
        highlight(line, lang).into_iter().filter(|(c, _)| !c.is_empty()).collect()
    }

    #[test]
    fn tokens_for_common_languages() {
        let t = classes("export function total(items: Item[], discount = 0) { // Summe", Lang::Js);
        assert!(t.contains(&("tok-kw", "export".into())));
        assert!(t.contains(&("tok-fn", "total".into())));
        assert!(t.contains(&("tok-ty", "Item".into())));
        assert!(t.contains(&("tok-num", "0".into())));
        assert_eq!(t.last().unwrap(), &("tok-com", "// Summe".into()));
        let t = classes("let s = \"a \\\" b\"; let n = 42;", Lang::Rust);
        assert!(t.contains(&("tok-str", "\"a \\\" b\"".into())));
        assert!(t.contains(&("tok-num", "42".into())));
        let t = classes("SELECT id FROM users WHERE x = 'a'", Lang::Sql);
        assert!(t.contains(&("tok-kw", "SELECT".into())));
        assert!(t.contains(&("tok-str", "'a'".into())));
        assert_eq!(lang_of("src/cart.ts"), Lang::Js);
        assert_eq!(lang_of("README"), Lang::Plain);
        // Unverändert zusammengesetzt
        let line = "fn main() { println!(\"hi\"); }";
        assert_eq!(highlight(line, Lang::Rust).into_iter().map(|(_, t)| t).collect::<String>(), line);
    }
}
