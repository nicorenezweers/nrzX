//! Markdown (GitHub-Flavored) → sicheres HTML.
//!
//! * pulldown-cmark: Tabellen, Aufgabenlisten, Fussnoten, Durchstreichen, Hinweise (`> [!NOTE]`)
//! * Codeblöcke mit Syntaxhervorhebung (`highlight`)
//! * Verweise: `#12` und `owner/repo#12` (Issue/PR), `@name` (Profil), Commit-SHAs, nackte URLs
//! * relative Links und Bilder zeigen ins Repository (`/src/…` bzw. `/raw/…`)
//! * Überschriften bekommen Anker (`id`), wie auf GitHub
//! * zum Schluss ammonia: nur erlaubte Tags/Attribute, Klassen nur `hl-…`/`language-…`/eigene

use crate::highlight::{escape, highlight_block};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Umfeld für Verweise und relative Links.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    /// `owner`
    pub owner: String,
    /// `name`
    pub repo: String,
    /// Branch/Tag für relative Links (leer = keine Umschreibung)
    pub git_ref: String,
    /// Verzeichnis der Markdown-Datei im Repository (für `![](bild.png)` neben dem README)
    pub dir: String,
    /// Basis für Wiki-Links (`[[Seite]]` und relative Links im Wiki)
    pub wiki: bool,
}

impl Ctx {
    pub fn repo(owner: &str, repo: &str) -> Self {
        Self { owner: owner.into(), repo: repo.into(), ..Default::default() }
    }

    pub fn file(owner: &str, repo: &str, git_ref: &str, dir: &str) -> Self {
        Self { owner: owner.into(), repo: repo.into(), git_ref: git_ref.into(), dir: dir.trim_matches('/').into(), wiki: false }
    }

    fn has_repo(&self) -> bool {
        !self.owner.is_empty() && !self.repo.is_empty()
    }
}

/// Markdown rendern (ohne Repository-Umfeld: nur `@name` und URLs werden verlinkt).
pub fn render_plain(text: &str) -> String {
    render(text, &Ctx::default())
}

pub fn render(text: &str, ctx: &Ctx) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_GFM);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let parser = Parser::new_ext(text, opts);
    let mut events: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None; // (Sprache, Inhalt)
    let mut link_depth = 0usize;
    let mut heading: Option<(HeadingLevel, Vec<Event>, String)> = None;
    let mut slugs: HashMap<String, usize> = HashMap::new();

    for ev in parser {
        // Codeblöcke einsammeln und hervorheben
        if let Some((lang, buf)) = code.as_mut() {
            match ev {
                Event::Text(t) => {
                    buf.push_str(&t);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => {
                    let lang_class = if lang.is_empty() { String::new() } else { format!(" class=\"language-{}\"", escape(lang)) };
                    let html = format!("<pre class=\"hl-code\"><code{lang_class}>{}</code></pre>\n", highlight_block(lang, buf));
                    code = None;
                    events.push(Event::Html(CowStr::from(html)));
                    continue;
                }
                _ => continue,
            }
        }
        let ev = match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split([' ', ',', '{']).next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
                continue;
            }
            Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
                link_depth += 1;
                Event::Start(Tag::Link { link_type, dest_url: CowStr::from(rewrite_url(&dest_url, ctx, false)), title, id })
            }
            Event::End(TagEnd::Link) => {
                link_depth = link_depth.saturating_sub(1);
                Event::End(TagEnd::Link)
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
                Event::Start(Tag::Image { link_type, dest_url: CowStr::from(rewrite_url(&dest_url, ctx, true)), title, id })
            }
            Event::Html(h) => Event::Html(CowStr::from(rewrite_html_urls(&h, ctx))),
            Event::InlineHtml(h) => Event::InlineHtml(CowStr::from(rewrite_html_urls(&h, ctx))),
            Event::Text(t) if link_depth == 0 => Event::Html(CowStr::from(linkify(&t, ctx))),
            other => other,
        };
        // Überschriften: Text sammeln, Anker erzeugen
        match ev {
            Event::Start(Tag::Heading { level, id, .. }) => {
                heading = Some((level, Vec::new(), id.map(|i| i.to_string()).unwrap_or_default()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, inner, given_id)) = heading.take() {
                    let mut text = String::new();
                    let mut inner_html = String::new();
                    for e in &inner {
                        if let Event::Text(t) | Event::Code(t) = e {
                            text.push_str(t);
                        }
                        if let Event::Html(h) = e {
                            text.push_str(&strip_tags(h));
                        }
                    }
                    pulldown_cmark::html::push_html(&mut inner_html, inner.into_iter());
                    let mut slug = if given_id.is_empty() { slugify(&text) } else { slugify(&given_id) };
                    let n = slugs.entry(slug.clone()).or_insert(0);
                    if *n > 0 {
                        slug = format!("{slug}-{n}");
                    }
                    *n += 1;
                    let lvl = level as usize;
                    events.push(Event::Html(CowStr::from(format!(
                        "<h{lvl} id=\"{slug}\"><a class=\"anchor\" href=\"#{slug}\" aria-hidden=\"true\">#</a>{inner_html}</h{lvl}>\n"
                    ))));
                }
            }
            other => match heading.as_mut() {
                Some((_, inner, _)) => inner.push(other),
                None => events.push(other),
            },
        }
    }

    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    sanitize(&html)
}

fn strip_tags(s: &str) -> String {
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("regex"));
    TAG.replace_all(s, "").to_string()
}

/// Anker wie GitHub: klein, Leerzeichen → `-`, Satzzeichen weg.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            out.push('-');
        }
    }
    if out.is_empty() {
        "section".into()
    } else {
        out
    }
}

/// Relative Adresse ins Repository umschreiben: Links → `/o/r/src/branch/<ref>/<pfad>`,
/// Bilder → `/o/r/raw/branch/<ref>/<pfad>`. Absolute URLs, Anker und `mailto:` bleiben.
fn rewrite_url(url: &str, ctx: &Ctx, image: bool) -> String {
    let u = url.trim();
    if u.is_empty() || u.starts_with('#') || u.starts_with('/') || u.contains("://") || u.starts_with("mailto:") || u.starts_with("data:") {
        return u.to_string();
    }
    if ctx.wiki && ctx.has_repo() && !image {
        let page = u.trim_end_matches(".md");
        return format!("/{}/{}/wiki/{}", ctx.owner, ctx.repo, page);
    }
    if ctx.git_ref.is_empty() || !ctx.has_repo() {
        return u.to_string();
    }
    let mut parts: Vec<&str> = if ctx.dir.is_empty() { Vec::new() } else { ctx.dir.split('/').collect() };
    let (path, frag) = match u.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (u, None),
    };
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let kind = if image { "raw" } else { "src" };
    let mut out = format!("/{}/{}/{kind}/branch/{}/{}", ctx.owner, ctx.repo, ctx.git_ref, parts.join("/"));
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// `src="…"`/`href="…"` in HTML-Schnipseln (README mit `<img src="docs/x.png">`) umschreiben.
fn rewrite_html_urls(html: &str, ctx: &Ctx) -> String {
    static ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\b(src|href)\s*=\s*"([^"]*)""#).expect("regex"));
    if ctx.git_ref.is_empty() {
        return html.to_string();
    }
    ATTR.replace_all(html, |c: &regex::Captures| {
        let image = c[1].eq_ignore_ascii_case("src");
        format!("{}=\"{}\"", &c[1], rewrite_url(&c[2], ctx, image))
    })
    .to_string()
}

static REFS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?P<url>https?://[^\s<>()]+[^\s<>().,;:!?'\x22])",
        r"|(?P<xref>\b(?P<xo>[A-Za-z0-9][A-Za-z0-9_.-]*)/(?P<xr>[A-Za-z0-9_.-]+)#(?P<xn>\d+)\b)",
        r"|(?P<issue>(?:^|\B)#(?P<n>\d+)\b)",
        r"|(?P<mention>(?:^|\B)@(?P<user>[A-Za-z0-9][A-Za-z0-9_.-]*[A-Za-z0-9_]|[A-Za-z0-9]))",
        r"|(?P<sha>\b[0-9a-f]{7,40}\b)",
    ))
    .expect("regex")
});

/// Text maskieren und Verweise verlinken.
fn linkify(text: &str, ctx: &Ctx) -> String {
    let mut out = String::new();
    let mut last = 0;
    for c in REFS.captures_iter(text) {
        let m = c.get(0).expect("match");
        out.push_str(&escape(&text[last..m.start()]));
        last = m.end();
        let raw = m.as_str();
        if let Some(u) = c.name("url") {
            out.push_str(&format!("<a href=\"{0}\">{0}</a>", escape(u.as_str())));
        } else if c.name("xref").is_some() {
            out.push_str(&format!(
                "<a class=\"ref-issue\" href=\"/{}/{}/issues/{}\">{}</a>",
                escape(&c["xo"]),
                escape(&c["xr"]),
                &c["xn"],
                escape(raw)
            ));
        } else if c.name("issue").is_some() && ctx.has_repo() {
            out.push_str(&format!(
                "<a class=\"ref-issue\" href=\"/{}/{}/issues/{}\">#{}</a>",
                escape(&ctx.owner),
                escape(&ctx.repo),
                &c["n"],
                &c["n"]
            ));
        } else if c.name("mention").is_some() {
            let user = &c["user"];
            out.push_str(&format!("<a class=\"mention\" href=\"/{0}\">@{0}</a>", escape(user)));
        } else if c.name("sha").is_some() && ctx.has_repo() && raw.chars().any(|ch| ch.is_ascii_digit()) && raw.chars().any(|ch| ch.is_ascii_alphabetic()) {
            out.push_str(&format!(
                "<a class=\"ref-commit\" href=\"/{}/{}/commit/{}\"><code>{}</code></a>",
                escape(&ctx.owner),
                escape(&ctx.repo),
                raw,
                &raw[..raw.len().min(10)]
            ));
        } else {
            out.push_str(&escape(raw));
        }
    }
    out.push_str(&escape(&text[last..]));
    out
}

/// Erlaubte Klassen: Syntaxfarben, Sprache des Codeblocks und die eigenen Markierungen.
fn allowed_class(class: &str) -> bool {
    class.starts_with("hl-")
        || class.starts_with("language-")
        || class.starts_with("markdown-alert")
        || matches!(class, "anchor" | "mention" | "ref-issue" | "ref-commit" | "task-list-item" | "footnote-definition" | "footnote-reference" | "footnote-definition-label")
}

static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut b = ammonia::Builder::default();
    let tags: HashSet<&str> = ["input", "details", "summary", "picture", "source", "kbd", "sup", "sub", "del", "ins", "mark", "abbr", "dl", "dt", "dd", "video"]
        .into_iter()
        .collect();
    b.add_tags(tags);
    b.add_generic_attributes(["class", "align", "id"]);
    b.add_tag_attributes("img", ["width", "height", "align", "loading"]);
    b.add_tag_attributes("input", ["type", "checked", "disabled"]);
    b.add_tag_attributes("source", ["srcset", "media", "type", "src"]);
    b.add_tag_attributes("video", ["src", "controls", "width", "height", "poster"]);
    b.add_tag_attributes("td", ["align", "colspan", "rowspan"]);
    b.add_tag_attributes("th", ["align", "colspan", "rowspan"]);
    b.add_tag_attributes("details", ["open"]);
    b.add_tag_attributes("ol", ["start"]);
    b.link_rel(Some("nofollow noopener noreferrer"));
    b.attribute_filter(|element, attribute, value| match attribute {
        "class" => {
            let kept: Vec<&str> = value.split_whitespace().filter(|c| allowed_class(c)).collect();
            if kept.is_empty() {
                None
            } else {
                Some(kept.join(" ").into())
            }
        }
        // Eigene IDs nur an Überschriften und Fussnoten
        "id" if !matches!(element, "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "div" | "sup") => None,
        "type" if element == "input" => (value == "checkbox").then(|| value.into()),
        _ => Some(value.into()),
    });
    b
});

pub fn sanitize(html: &str) -> String {
    SANITIZER.clean(html).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_gfm_and_strips_scripts() {
        let html = render_plain("# Hallo Welt\n\n- [x] fertig\n\n<script>alert(1)</script>\n\n| a |\n|---|\n| b |");
        assert!(html.contains("<h1 id=\"hallo-welt\">"), "{html}");
        assert!(html.contains("checkbox"));
        assert!(!html.contains("<script"));
        assert!(html.contains("<table>"));
    }

    #[test]
    fn links_refs_and_rewrites_relative() {
        let ctx = Ctx::file("nico", "app", "main", "docs");
        let html = render("Siehe #12 und @anna, Bild: ![x](img/a.png) [Doku](../README.md)", &ctx);
        assert!(html.contains("/nico/app/issues/12"), "{html}");
        assert!(html.contains("href=\"/anna\""));
        assert!(html.contains("/nico/app/raw/branch/main/docs/img/a.png"));
        assert!(html.contains("/nico/app/src/branch/main/README.md"));
    }

    #[test]
    fn highlights_code_blocks() {
        let html = render_plain("```rust\nfn main() {}\n```");
        assert!(html.contains("hl-"), "{html}");
        assert!(html.contains("language-rust"));
    }
}
