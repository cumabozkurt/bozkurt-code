//! End-to-end tests of the HTTP API over a real listener (temp home, random ports,
//! no-op networking so nothing outside the temp dir is touched).

use std::time::Duration;

use bz_daemon::{Daemon, DaemonOptions, NetChoice};
use futures_util::StreamExt;
use serde_json::{json, Value};

struct T {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    d: Option<Daemon>,
    base: String,
    /// Sends the local API token.
    http: reqwest::Client,
    /// No token, no default headers.
    raw: reqwest::Client,
    token: String,
}

impl T {
    async fn new() -> T {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut o = DaemonOptions::new(root.join("home"));
        o.api_port = Some(0);
        o.net = NetChoice::Noop;
        o.autostart = false;
        o.mdns = false;
        let d = Daemon::start(o).await.unwrap();
        let base = format!("http://{}/api/v1", d.local_addr);
        let token = d.api_token().to_string();
        assert_eq!(
            bz_core::apitoken::read(&root.join("home")).as_deref(),
            Some(token.as_str())
        );
        let mut hm = reqwest::header::HeaderMap::new();
        hm.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        T {
            _dir: dir,
            root,
            d: Some(d),
            base,
            http: reqwest::Client::builder()
                .no_proxy()
                .default_headers(hm)
                .build()
                .unwrap(),
            raw: reqwest::Client::builder().no_proxy().build().unwrap(),
            token,
        }
    }
    fn url(&self, p: &str) -> String {
        format!("{}{p}", self.base)
    }
    async fn get(&self, p: &str) -> (u16, Value) {
        let r = self.http.get(self.url(p)).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn send(&self, m: reqwest::Method, p: &str, body: Option<Value>) -> (u16, Value) {
        let mut rq = self.http.request(m, self.url(p));
        if let Some(b) = body {
            rq = rq.json(&b);
        }
        let r = rq.send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn post(&self, p: &str, body: Value) -> (u16, Value) {
        self.send(reqwest::Method::POST, p, Some(body)).await
    }
    async fn stop(mut self) {
        if let Some(d) = self.d.take() {
            d.shutdown().await;
        }
    }
}

fn laravel_dir(root: &std::path::Path, name: &str) -> String {
    let p = root.join(name);
    std::fs::create_dir_all(p.join("public")).unwrap();
    std::fs::write(p.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::write(
        p.join("composer.json"),
        r#"{"require":{"php":"^8.2","laravel/framework":"^11.0"}}"#,
    )
    .unwrap();
    std::fs::write(p.join("public/index.php"), "<?php echo 'hi';").unwrap();
    p.to_string_lossy().into_owned()
}

async fn wait_job(t: &T, id: &str) -> Value {
    for _ in 0..100 {
        let (_, j) = t.get(&format!("/jobs/{id}")).await;
        if j["status"] != "running" {
            return j;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job {id} did not finish");
}

#[tokio::test]
async fn health_overview_and_errors() {
    let t = T::new().await;
    let (s, h) = t.get("/health").await;
    assert_eq!(s, 200);
    assert_eq!(h["ok"], true);
    assert_eq!(h["version"], bz_core::engine::VERSION);

    let (s, o) = t.get("/overview").await;
    assert_eq!(s, 200);
    assert_eq!(o["tld"], "test");
    assert_eq!(o["sites_count"], 0);
    assert!(o["services"].as_array().unwrap().len() >= 6);

    // error envelope + status mapping
    let (s, e) = t.get("/sites/nope").await;
    assert_eq!(s, 404);
    assert_eq!(e["error"]["code"], "not_found");
    assert!(e["error"]["message"].as_str().unwrap().contains("nope"));
    let (s, e) = t.get("/no/such/route").await;
    assert_eq!(s, 404);
    assert_eq!(e["error"]["code"], "not_found");
    let (s, e) = t
        .post("/detect", json!({"path": "/definitely/not/here"}))
        .await;
    assert_eq!(s, 404);
    assert_eq!(e["error"]["code"], "not_found");
    let r = t
        .http
        .post(t.url("/sites"))
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    let e: Value = r.json().await.unwrap();
    assert_eq!(e["error"]["code"], "invalid_input");
    let (s, e) = t.get("/runtimes/ruby/versions").await;
    assert_eq!(s, 400);
    assert_eq!(e["error"]["code"], "invalid_input");
    let (s, e) = t.send(reqwest::Method::PUT, "/health", None).await;
    assert_eq!(s, 405);
    assert_eq!(e["error"]["code"], "method_not_allowed");
    let (s, e) = t
        .send(
            reqwest::Method::PATCH,
            "/config",
            Some(json!({"tld": "dev"})),
        )
        .await;
    assert_eq!(s, 400, "{e}");
    // D1: invalid / duplicate ports → 400, config.toml untouched
    let before = std::fs::read(t.root.join("home/config.toml")).ok();
    for body in [
        json!({"https_port": 0}),
        json!({"http_port": 443, "https_port": 443}),
    ] {
        let (s, e) = t
            .send(reqwest::Method::PATCH, "/config", Some(body.clone()))
            .await;
        assert_eq!(s, 400, "{body} {e}");
        assert_eq!(e["error"]["code"], "invalid_input");
    }
    assert_eq!(std::fs::read(t.root.join("home/config.toml")).ok(), before);

    // misc read endpoints
    assert_eq!(t.get("/runtimes").await.1.as_array().unwrap().len(), 5);
    assert_eq!(t.get("/services").await.0, 200);
    assert_eq!(t.get("/dns").await.1["tld"], "test");
    assert!(t.get("/tls").await.1["ca_path"]
        .as_str()
        .unwrap()
        .ends_with("rootCA.pem"));
    let (s, p) = t.get("/ports/1").await;
    assert_eq!(s, 200);
    assert_eq!(p["port"], 1);
    assert!(p.get("in_use").is_some());
    let (s, doc) = t.get("/doctor").await;
    assert_eq!(s, 200);
    assert!(doc
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == "dns" && c["fix"] == "dns.configure"));
    assert_eq!(t.get("/config").await.1["api_port"], 7878);
    assert_eq!(t.get("/services/redis/logs?lines=5").await.0, 200);
    assert_eq!(t.get("/import/scan").await.0, 200);
    let (s, e) = t.get("/services/redis/databases").await;
    assert_eq!(s, 400, "{e}");
    let (s, _) = t.get("/services/mariadb/databases").await;
    assert_eq!(s, 404); // not installed
    t.stop().await;
}

#[tokio::test]
async fn site_crud_jobs_and_events() {
    let t = T::new().await;
    let path = laravel_dir(&t.root, "shop");

    // websocket subscribed before the change
    let ws_url = format!(
        "ws://{}/api/v1/events?token={}",
        t.d.as_ref().unwrap().local_addr,
        t.token
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    let (s, det) = t.post("/detect", json!({"path": path})).await;
    assert_eq!(s, 200);
    assert_eq!(det["framework"], "laravel");
    assert_eq!(det["kind"], "php");
    assert_eq!(det["root"], "public");
    assert_eq!(det["suggested_domain"], "shop.test");

    let (s, site) = t.post("/sites", json!({"path": path})).await;
    assert_eq!(s, 201, "{site}");
    assert_eq!(site["domain"], "shop.test");
    assert_eq!(site["framework"], "laravel");
    let id = site["id"].as_str().unwrap().to_string();

    // the websocket sees site.changed
    let ev = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m = ws.next().await.unwrap().unwrap();
            if let tokio_tungstenite::tungstenite::Message::Text(txt) = m {
                let v: Value = serde_json::from_str(&txt).unwrap();
                if v["type"] == "site.changed" {
                    return v;
                }
            }
        }
    })
    .await
    .expect("event");
    assert_eq!(ev["site"]["id"], id.as_str());

    // duplicate domain → conflict
    let (s, e) = t.post("/sites", json!({"path": path})).await;
    assert!(s == 409 || s == 400, "{s} {e}");

    let (s, list) = t.get("/sites").await;
    assert_eq!(s, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);

    let (s, p) = t
        .send(
            reqwest::Method::PATCH,
            &format!("/sites/{id}"),
            Some(json!({"aliases": ["admin.shop"], "env": {"APP_ENV": "local"}})),
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["aliases"], json!(["admin.shop.test"]));
    assert_eq!(p["env"]["APP_ENV"], "local");

    let (s, p) = t.post(&format!("/sites/{id}/unsecure"), json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(p["https"], false);
    let (s, e) = t.post(&format!("/sites/{id}/start"), json!({})).await;
    assert_eq!(s, 400, "{e}"); // not an app site
    let (s, _) = t.get(&format!("/sites/{id}/logs?lines=10")).await;
    assert_eq!(s, 200);

    // jobs: tls trust via the no-op provider
    let (s, j) = t.send(reqwest::Method::POST, "/tls/trust", None).await;
    assert_eq!(s, 202);
    let jid = j["job_id"].as_str().unwrap().to_string();
    let done = wait_job(&t, &jid).await;
    assert_eq!(done["status"], "done", "{done}");
    let (_, jobs) = t.get("/jobs").await;
    assert!(jobs
        .as_array()
        .unwrap()
        .iter()
        .any(|j| j["id"] == jid.as_str()));
    let (s, _) = t.get("/jobs/does-not-exist").await;
    assert_eq!(s, 404);
    let (s, j) = t.post("/doctor/fix/services.dns.start", json!({})).await;
    assert_eq!(s, 202);
    assert_eq!(
        wait_job(&t, j["job_id"].as_str().unwrap()).await["status"],
        "done"
    );
    assert_eq!(t.get("/dns").await.1["running"], true);
    let (s, _) = t.post("/doctor/fix/bogus", json!({})).await;
    assert_eq!(s, 404);

    // DNS entries alone are rejected (never imported, P1-20)
    let (s, j) = t
        .post(
            "/import",
            json!({"vhosts": [], "dns": [{"source": "valet", "tld": "test", "file": "/etc/resolver/test"}]}),
        )
        .await;
    assert_eq!(s, 400, "{j}");
    assert_eq!(j["error"]["code"], "invalid_input", "{j}");

    // import job with one vhost
    let other = laravel_dir(&t.root, "legacy");
    let (s, j) = t
        .post(
            "/import",
            json!({"vhosts": [{"source": "mamp", "domain": "legacy.local", "path": other}], "dns": []}),
        )
        .await;
    assert_eq!(s, 202);
    let done = wait_job(&t, j["job_id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "done", "{done}");
    assert!(t
        .get("/sites")
        .await
        .1
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["domain"] == "legacy.test"));

    let r = t
        .http
        .delete(t.url(&format!("/sites/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 204);
    assert_eq!(t.get(&format!("/sites/{id}")).await.0, 404);
    t.stop().await;
}

#[tokio::test]
async fn cors_preflight() {
    let t = T::new().await;
    let r = t
        .http
        .request(reqwest::Method::OPTIONS, t.url("/sites"))
        .header("Origin", "http://localhost:1420")
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Headers", "content-type")
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert_eq!(
        r.headers()["access-control-allow-origin"],
        "http://localhost:1420"
    );
    let r = t
        .http
        .get(t.url("/health"))
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert!(r.headers().get("access-control-allow-origin").is_none());
    t.stop().await;
}

#[tokio::test]
async fn remote_pairing_flow() {
    let t = T::new().await;
    // port 0 is rejected (D1); enable on a free port instead
    let (s, e) = t
        .send(
            reqwest::Method::PATCH,
            "/config",
            Some(json!({"remote": {"port": 0}})),
        )
        .await;
    assert_eq!(s, 400, "{e}");
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (s, _) = t
        .send(
            reqwest::Method::PATCH,
            "/config",
            Some(json!({"remote": {"port": free}})),
        )
        .await;
    assert_eq!(s, 200);
    // a port that cannot be bound → 409 and `enabled` stays false on disk
    let busy = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let (s, _) = t
        .send(
            reqwest::Method::PATCH,
            "/config",
            Some(json!({"remote": {"port": busy.local_addr().unwrap().port()}})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, e) = t.post("/remote/enable", json!({})).await;
    assert_eq!(s, 409, "{e}");
    assert_eq!(t.get("/config").await.1["remote"]["enabled"], false);
    drop(busy);
    let (s, _) = t
        .send(
            reqwest::Method::PATCH,
            "/config",
            Some(json!({"remote": {"port": free}})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, info) = t.post("/remote/enable", json!({})).await;
    assert_eq!(s, 200, "{info}");
    assert_eq!(info["enabled"], true);
    let raddr =
        t.d.as_ref()
            .unwrap()
            .remote_addr()
            .await
            .expect("remote running");
    let rbase = format!("http://127.0.0.1:{}/api/v1", raddr.port());
    let h = &t.raw;

    // no token → 401
    let r = h.get(format!("{rbase}/health")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    let e: Value = r.json().await.unwrap();
    assert_eq!(e["error"]["code"], "unauthorized");

    // pairing code: local only
    let r = h
        .post(format!("{rbase}/remote/pairing-code"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401); // not even authorized remotely
    let (s, e) = t
        .post(
            "/remote/pair",
            json!({"code": "000000", "device_name": "x"}),
        )
        .await;
    assert_eq!(s, 403, "{e}");
    let (s, code) = t.post("/remote/pairing-code", json!({})).await;
    assert_eq!(s, 200);
    let c = code["code"].as_str().unwrap().to_string();
    assert_eq!(c.len(), 8); // P1-2: 8 digits
    assert!(code["expires_at"].is_string());

    // wrong code → 401 bad_code
    let wrong = if c == "11111111" {
        "22222222"
    } else {
        "11111111"
    };
    let r = h
        .post(format!("{rbase}/remote/pair"))
        .json(&json!({"code": wrong, "device_name": "Pixel"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["code"],
        "bad_code"
    );

    // right code → token
    let r = h
        .post(format!("{rbase}/remote/pair"))
        .json(&json!({"code": c, "device_name": "Pixel"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let tok = r.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // reuse rejected
    let r = h
        .post(format!("{rbase}/remote/pair"))
        .json(&json!({"code": c, "device_name": "Pixel 2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);

    // authorized request
    let r = h
        .get(format!("{rbase}/overview"))
        .bearer_auth(&tok)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    // wrong token
    let r = h
        .get(format!("{rbase}/overview"))
        .bearer_auth("deadbeef")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
    // D1: the remote listener cannot move the API or its own port
    for body in [json!({"api_port": 7000}), json!({"remote": {"port": 7001}})] {
        let r = h
            .patch(format!("{rbase}/config"))
            .bearer_auth(&tok)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 403, "{body}");
        assert_eq!(
            r.json::<Value>().await.unwrap()["error"]["code"],
            "forbidden"
        );
    }
    // ...but other keys (and an unchanged api_port) are fine
    let r = h
        .patch(format!("{rbase}/config"))
        .bearer_auth(&tok)
        .json(&json!({"api_port": 7878, "dns_port": 5391}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    // pairing-code refused remotely even with a token
    let r = h
        .post(format!("{rbase}/remote/pairing-code"))
        .bearer_auth(&tok)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    // websocket with ?token=
    let (ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{}/api/v1/events?token={tok}",
        raddr.port()
    ))
    .await
    .expect("ws with token");
    drop(ws);
    assert!(tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{}/api/v1/events?token=bad",
        raddr.port()
    ))
    .await
    .is_err());

    // device listed; token stored hashed
    let (_, info) = t.get("/remote").await;
    let devs = info["paired_devices"].as_array().unwrap();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0]["name"], "Pixel");
    let stored = std::fs::read_to_string(t.root.join("home/remote.json")).unwrap();
    assert!(!stored.contains(&tok));

    // revoke → 401
    let id = devs[0]["id"].as_str().unwrap();
    let r = t
        .http
        .delete(t.url(&format!("/remote/devices/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 204);
    let r = h
        .get(format!("{rbase}/health"))
        .bearer_auth(&tok)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);

    // rate limiting (5 attempts/min per address; 3 used above)
    let mut last = 0;
    for _ in 0..4 {
        last = h
            .post(format!("{rbase}/remote/pair"))
            .json(&json!({"code": "000001", "device_name": "x"}))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16();
    }
    assert_eq!(last, 429);

    let (s, info) = t.post("/remote/disable", json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(info["enabled"], false);
    assert!(t.d.as_ref().unwrap().remote_addr().await.is_none());
    t.stop().await;
}

// ---- P0-1: local API protection --------------------------------------------------------

async fn status_of(r: reqwest::RequestBuilder) -> (u16, Value) {
    let r = r.send().await.unwrap();
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn local_api_requires_token_for_non_browser_clients() {
    let t = T::new().await;
    // no token → 401 (health stays open for liveness probes)
    let (s, e) = status_of(t.raw.get(t.url("/overview"))).await;
    assert_eq!(s, 401, "{e}");
    assert_eq!(e["error"]["code"], "unauthorized");
    assert_eq!(status_of(t.raw.get(t.url("/health"))).await.0, 200);
    // wrong token → 401; right token via Bearer or X-Bozkurt-Token → 200
    assert_eq!(
        status_of(t.raw.get(t.url("/overview")).bearer_auth("0".repeat(64)))
            .await
            .0,
        401
    );
    assert_eq!(
        status_of(
            t.raw
                .get(t.url("/overview"))
                .header("X-Bozkurt-Token", &t.token)
        )
        .await
        .0,
        200
    );
    assert_eq!(status_of(t.http.get(t.url("/overview"))).await.0, 200);
    // websocket without token refused, with ?token= accepted
    let addr = t.d.as_ref().unwrap().local_addr;
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events"))
            .await
            .is_err()
    );
    let (ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/events?token={}", t.token))
            .await
            .expect("ws with token");
    drop(ws);
    t.stop().await;
}

#[tokio::test]
async fn local_api_rejects_cross_site_requests() {
    let t = T::new().await;
    let path = t.root.to_string_lossy().into_owned();
    let body = format!(r#"{{"path":"{path}"}}"#);
    // the CSRF shape from the audit: text/plain "simple" request from a foreign origin
    let (s, e) = status_of(
        t.raw
            .post(t.url("/detect"))
            .header("Origin", "https://evil.example")
            .header("Content-Type", "text/plain")
            .body(body.clone()),
    )
    .await;
    assert_eq!(s, 403, "{e}");
    // same with the token (a foreign origin is never allowed)
    let (s, _) = status_of(
        t.http
            .post(t.url("/detect"))
            .header("Origin", "https://evil.example")
            .header("Content-Type", "application/json")
            .body(body.clone()),
    )
    .await;
    assert_eq!(s, 403);
    // non-JSON content type → 415 even with a valid token
    let (s, e) = status_of(
        t.http
            .post(t.url("/detect"))
            .header("Content-Type", "text/plain")
            .body(body.clone()),
    )
    .await;
    assert_eq!(s, 415, "{e}");
    assert_eq!(e["error"]["code"], "unsupported_media_type");
    let (s, _) = status_of(
        t.http
            .post(t.url("/detect"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body("path=/tmp"),
    )
    .await;
    assert_eq!(s, 415);
    // a body with no content type at all → 415
    let (s, _) = status_of(t.http.post(t.url("/detect")).body(body.clone())).await;
    assert_eq!(s, 415);
    // DNS rebinding: foreign Host → 421
    let (s, e) = status_of(
        t.http
            .get(t.url("/overview"))
            .header("Host", "rebind.evil.example"),
    )
    .await;
    assert_eq!(s, 421, "{e}");
    let port = t.d.as_ref().unwrap().local_addr.port();
    let (s, _) = status_of(
        t.http
            .get(t.url("/overview"))
            .header("Host", format!("localhost:{port}")),
    )
    .await;
    assert_eq!(s, 200);
    // the Tauri webview origin works without a token, with JSON
    let (s, e) = status_of(
        t.raw
            .post(t.url("/detect"))
            .header("Origin", "tauri://localhost")
            .header("Content-Type", "application/json")
            .body(body),
    )
    .await;
    assert_eq!(s, 200, "{e}");
    // cross-site websocket refused even with a token
    let addr = t.d.as_ref().unwrap().local_addr;
    let mut req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{addr}/api/v1/events?token={}", t.token),
    )
    .unwrap();
    req.headers_mut()
        .insert("Origin", "https://evil.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
    t.stop().await;
}

#[tokio::test]
async fn shutdown_endpoint_is_local_and_signals() {
    let t = T::new().await;
    let (s, _) = status_of(t.raw.post(t.url("/daemon/shutdown"))).await;
    assert_eq!(s, 401);
    let (s, v) = status_of(t.http.post(t.url("/daemon/shutdown"))).await;
    assert_eq!(s, 202, "{v}");
    tokio::time::timeout(
        Duration::from_secs(5),
        t.d.as_ref().unwrap().shutdown_requested(),
    )
    .await
    .expect("shutdown requested");
    t.stop().await;
}
