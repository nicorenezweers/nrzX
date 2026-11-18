//! Sicherheits-Header für jede Antwort. Die Content-Security-Policy erlaubt nur eigene
//! Inhalte: Oberfläche, CSS und Schrift sind eingebettet (siehe routes::spa und
//! routes::static_files), Skripte nur als Dateien – keine Inline-Skripte. WebAssembly braucht
//! 'wasm-unsafe-eval', Dioxus' DOM-Brücke 'unsafe-eval'. Inline-Stile bleiben erlaubt
//! (Label-Farben). Bilder in Markdown dürfen von überall kommen (README-Abzeichen).
//! Rahmen (`frame-src`): die Vorschau läuft auf einem eigenen Port (Bereich workspace).
//! Das Mikrofon darf nur die eigene Seite benutzen (Sprachsteuerung „Dieter“).

use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::Header;
use rocket::{Request, Response};

pub struct SecurityHeaders;

const CSP: &str = "default-src 'self'; \
    script-src 'self' 'wasm-unsafe-eval' 'unsafe-eval'; \
    style-src 'self' 'unsafe-inline'; \
    font-src 'self'; \
    img-src 'self' data: blob: https: http:; \
        connect-src 'self'; \
    object-src 'none'; \
    frame-src 'self' http: https:; \
    frame-ancestors 'none'; \
    base-uri 'self'; \
    form-action 'self'";

#[rocket::async_trait]
impl Fairing for SecurityHeaders {
    fn info(&self) -> Info {
        Info {
            name: "Sicherheits-Header",
            kind: Kind::Response,
        }
    }

    async fn on_response<'r>(&self, _req: &'r Request<'_>, res: &mut Response<'r>) {
        for (name, value) in [
            ("Content-Security-Policy", CSP),
            ("X-Content-Type-Options", "nosniff"),
            ("X-Frame-Options", "DENY"),
            ("Referrer-Policy", "same-origin"),
            // Mikrofon nur für die eigene Oberfläche (Sprachsteuerung „Dieter“), nie für Rahmen
            ("Permissions-Policy", "camera=(), microphone=(self), geolocation=()"),
            ("Cross-Origin-Opener-Policy", "same-origin"),
        ] {
            if !res.headers().contains(name) {
                res.set_header(Header::new(name, value));
            }
        }
    }
}
