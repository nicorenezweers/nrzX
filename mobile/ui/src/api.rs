//! Aufrufe an den Server von nrzXMobile. Fehler kommen als Text (`{"error": …}` des Servers
//! bzw. von nrzX am PC). Die Sitzung steckt im HttpOnly-Cookie (die Oberfläche sieht sie nie);
//! ändernde Aufrufe tragen `X-Nrzm: 1` (Schutz vor fremden Seiten). Läuft die Sitzung ab
//! (401), geht es zurück zur Anmeldung.

use gloo_net::http::{Request, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;

async fn check(r: Result<Response, gloo_net::Error>) -> Result<Response, String> {
    let resp = r.map_err(|e| format!("Keine Verbindung: {e}"))?;
    if resp.ok() {
        return Ok(resp);
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            // Server: {"error": "…"}; nrzX: {"error": {"code", "message"}}
            v["error"].as_str().or(v["error"]["message"].as_str()).or(v["message"].as_str()).map(str::to_string)
        })
        .unwrap_or_else(|| format!("Fehler {status}"));
    if status == 401 {
        crate::signed_out();
    }
    Err(msg)
}

async fn finish<T: DeserializeOwned>(r: Result<Response, gloo_net::Error>) -> Result<T, String> {
    check(r).await?.json::<T>().await.map_err(|e| e.to_string())
}

fn write(b: RequestBuilder) -> RequestBuilder {
    b.header("X-Nrzm", "1")
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    finish(Request::get(path).send().await).await
}

pub async fn post<T: DeserializeOwned, B: Serialize>(path: &str, body: &B) -> Result<T, String> {
    let req = write(Request::post(path)).json(body).map_err(|e| e.to_string())?;
    finish(req.send().await).await
}

/// POST ohne Antwort-Rumpf (204).
pub async fn post_empty<B: Serialize>(path: &str, body: &B) -> Result<(), String> {
    let req = write(Request::post(path)).json(body).map_err(|e| e.to_string())?;
    check(req.send().await).await.map(|_| ())
}

pub async fn delete(path: &str) -> Result<(), String> {
    check(write(Request::delete(path)).send().await).await.map(|_| ())
}
