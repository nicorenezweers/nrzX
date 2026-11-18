//! Bereich workspace: Vorschau-Token und Proxy gegen einen kleinen lokalen HTTP-Server
//! (Weiterleitungen, Cookies, Host, WebSocket-artiges Upgrade).

mod common;

use common::TestApp;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response};
use nrzx_shared::workspace::{PreviewPort, PreviewToken};
use rocket::http::{ContentType, Status};
use rocket::local::asynchronous::Client;
use serde_json::json;
use std::convert::Infallible;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Freien Port finden.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Ziel der Vorschau: gibt Host, Cookies und Pfad zurück, leitet `/weiter` absolut auf sich selbst
/// um und beantwortet `Upgrade: echo` mit 101 und einem Echo.
async fn target(mut req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let port = req.headers().get("host").and_then(|h| h.to_str().ok()).unwrap_or("").rsplit(':').next().unwrap_or("").to_string();
    if req.headers().get("upgrade").is_some() {
        let on = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            if let Ok(mut io) = on.await {
                let mut buf = [0u8; 64];
                if let Ok(n) = io.read(&mut buf).await {
                    let _ = io.write_all(&buf[..n]).await;
                }
            }
        });
        return Ok(Response::builder().status(101).header("connection", "upgrade").header("upgrade", "echo").body(Body::empty()).unwrap());
    }
    if req.uri().path() == "/weiter" {
        return Ok(Response::builder()
            .status(302)
            .header("location", format!("http://localhost:{port}/ziel?x=1"))
            .header("x-frame-options", "DENY")
            .body(Body::empty())
            .unwrap());
    }
    let h = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
    let text = format!("path={} host={} cookie={} origin={}", req.uri(), h("host"), h("cookie"), h("origin"));
    Ok(Response::builder().header("set-cookie", "sitzung=1; Domain=localhost; Path=/").body(Body::from(text)).unwrap())
}

#[rocket::async_test]
async fn preview_token_and_proxy() {
    // App mit eingeschaltetem Vorschau-Port (Proxy starten wir selbst)
    let dir = tempfile::tempdir().unwrap();
    let preview_port = free_port();
    let figment = nrzx::config::figment()
        .merge(("data_dir", dir.path().to_string_lossy().to_string()))
        .merge(("background", false))
        .merge(("preview_port", preview_port))
        .merge(("log_level", "off"));
    let client = Client::untracked(nrzx::build_with(figment).await.unwrap()).await.unwrap();
    let state = client.rocket().state::<nrzx::AppState>().unwrap().clone();
    let t = TestApp { client, state, dir };
    let anna = t.user("anna", false).await;
    let th = t.thread(anna.id, None, "Vorschau").await;

    let proxy = std::net::TcpListener::bind(("127.0.0.1", preview_port)).unwrap();
    tokio::spawn(nrzx::workspace::preview::serve(proxy));

    // Zielserver
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let make = make_service_fn(|_| async { Ok::<_, Infallible>(service_fn(target)) });
    tokio::spawn(hyper::Server::from_tcp(listener).unwrap().serve(make));

    // Ports von nrzX selbst gibt es nicht als Vorschau
    let res = t
        .client
        .post("/api/v1/previews/token")
        .private_cookie(anna.cookie.clone())
        .header(ContentType::JSON)
        .header(rocket::http::Header::new("Host", "code.local:9300"))
        .body(json!({ "threadId": th, "port": 9300 }).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::BadRequest);

    // Token holen
    let res = t
        .client
        .post("/api/v1/previews/token")
        .private_cookie(anna.cookie.clone())
        .header(ContentType::JSON)
        .header(rocket::http::Header::new("Host", "code.local:9300"))
        .body(json!({ "threadId": th, "port": port, "path": "/start" }).to_string())
        .dispatch()
        .await;
    assert_eq!(res.status(), Status::Ok);
    let tok: PreviewToken = serde_json::from_str(&res.into_string().await.unwrap()).unwrap();
    assert_eq!(tok.origin, format!("http://code.local:{preview_port}"));
    assert!(tok.url.starts_with(&format!("http://code.local:{preview_port}/__nrzx/open?token=")));
    let open = tok.url.replace("code.local", "127.0.0.1");
    let base = format!("http://127.0.0.1:{preview_port}");

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    // Ohne Cookie: kein Zugang
    let r = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403);
    // Token einlösen: Cookie und Weiterleitung auf den gewünschten Pfad
    let r = http.get(&open).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 302);
    assert_eq!(r.headers()["location"], "/start");
    let set = r.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(set.starts_with("nrzx_preview=") && set.contains("HttpOnly"));
    let preview_cookie = set.split(';').next().unwrap().to_string();
    // Ein zweites Mal gilt das Token nicht
    let r = http.get(&open).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403);

    // Weitergereicht: Host des Ziels, ohne Cookies von nrzX
    let cookies = format!("nrzx_session=GEHEIM; {preview_cookie}; eigenes=1");
    let r = http.get(format!("{base}/start?a=b")).header("cookie", &cookies).header("origin", &base).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let set = r.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(!set.to_lowercase().contains("domain"), "{set}");
    let body = r.text().await.unwrap();
    assert!(body.contains("path=/start?a=b"), "{body}");
    assert!(body.contains(&format!("host=localhost:{port}")), "{body}");
    assert!(body.contains("cookie=eigenes=1"), "{body}");
    assert!(!body.contains("GEHEIM") && !body.contains("nrzx_preview"), "{body}");
    assert!(body.contains(&format!("origin=http://localhost:{port}")), "{body}");

    // Weiterleitungen auf den Zielserver werden relativ
    let r = http.get(format!("{base}/weiter")).header("cookie", &preview_cookie).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 302);
    assert_eq!(r.headers()["location"], "/ziel?x=1");
    assert!(r.headers().get("x-frame-options").is_none());

    // Upgrade (wie WebSocket): 101 und Daten in beide Richtungen
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", preview_port)).await.unwrap();
    let req = format!("GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{preview_port}\r\nConnection: Upgrade\r\nUpgrade: echo\r\nCookie: {preview_cookie}\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 1024];
    let mut head = Vec::new();
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut buf).await.unwrap();
        assert!(n > 0, "Verbindung zu");
        head.extend_from_slice(&buf[..n]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    s.write_all(b"ping").await.unwrap();
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut buf)).await.unwrap().unwrap();
    assert_eq!(&buf[..n], b"ping");

    // Ports auflisten: Antwort für eigene Threads, fremde Threads 404
    let res = t.client.get(format!("/api/v1/previews?threadId={th}")).private_cookie(anna.cookie.clone()).dispatch().await;
    assert_eq!(res.status(), Status::Ok);
    let ports: Vec<PreviewPort> = serde_json::from_str(&res.into_string().await.unwrap()).unwrap();
    assert!(ports.iter().all(|p| p.port != 9300 && p.port != preview_port));
}
