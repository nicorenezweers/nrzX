//! Eingebettete Dateien unter `/static/…`: Schrift (Inter), Icons und die Syntaxfarben
//! `/static/highlight.css` (aus syntect erzeugt).

use rocket::http::{ContentType, Header};
use rocket::response::{self, Responder, Response};
use rocket::{get, routes, Request, Route};
use std::io::Cursor;
use std::sync::LazyLock;

pub fn routes() -> Vec<Route> {
    routes![file, highlight_css, favicon]
}

/// Inhalt aus `static/` (beim Übersetzen eingebettet).
const FILES: &[(&str, &[u8])] = &[
    ("fonts/inter-latin.woff2", include_bytes!("../../static/fonts/inter-latin.woff2")),
    ("fonts/inter-latin-ext.woff2", include_bytes!("../../static/fonts/inter-latin-ext.woff2")),
    ("icon.png", include_bytes!("../../static/icon.png")),
    ("favicon-32.png", include_bytes!("../../static/favicon-32.png")),
];

static HIGHLIGHT_CSS: LazyLock<String> = LazyLock::new(crate::highlight::css);

pub struct Static {
    body: Vec<u8>,
    content_type: ContentType,
    cache: &'static str,
}

impl<'r> Responder<'r, 'static> for Static {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        Response::build()
            .header(self.content_type)
            .header(Header::new("Cache-Control", self.cache))
            .sized_body(self.body.len(), Cursor::new(self.body))
            .ok()
    }
}

#[get("/static/<path..>", rank = 2)]
fn file(path: std::path::PathBuf) -> Option<Static> {
    let p = path.to_str()?.replace('\\', "/");
    let (_, body) = FILES.iter().find(|(name, _)| *name == p)?;
    let ext = p.rsplit('.').next().unwrap_or("");
    Some(Static {
        body: body.to_vec(),
        content_type: ContentType::from_extension(ext).unwrap_or(ContentType::Binary),
        cache: "public, max-age=604800",
    })
}

#[get("/static/highlight.css", rank = 1)]
fn highlight_css() -> Static {
    Static { body: HIGHLIGHT_CSS.as_bytes().to_vec(), content_type: ContentType::CSS, cache: "public, max-age=86400" }
}

#[get("/favicon.ico")]
fn favicon() -> Static {
    Static { body: FILES[3].1.to_vec(), content_type: ContentType::PNG, cache: "public, max-age=604800" }
}
