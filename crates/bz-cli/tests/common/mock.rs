//! A tiny in-test HTTP/1.1 server standing in for bozkurtd (std only, one thread per
//! connection, `Connection: close`). Shared by the bz-cli and bz-mcp integration tests.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

/// (method, path-without-query) → (status, body)
pub type Routes = HashMap<(String, String), (u16, Value)>;

pub struct Mock {
    pub port: u16,
    /// Every request received: (method, path incl. query, body).
    pub log: Arc<Mutex<Vec<(String, String, String)>>>,
    pub home: tempfile::TempDir,
}

impl Mock {
    pub fn start(routes: Routes) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let routes = Arc::new(routes);
        let l2 = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let routes = routes.clone();
                let log = l2.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &routes, &log);
                });
            }
        });
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            format!("api_port = {port}\n"),
        )
        .unwrap();
        Mock { port, log, home }
    }

    pub fn requests(&self) -> Vec<(String, String, String)> {
        self.log.lock().unwrap().clone()
    }

    pub fn saw(&self, method: &str, path: &str) -> bool {
        self.requests()
            .iter()
            .any(|(m, p, _)| m == method && p == path)
    }
}

fn handle(
    stream: TcpStream,
    routes: &Routes,
    log: &Mutex<Vec<(String, String, String)>>,
) -> std::io::Result<()> {
    let mut r = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        r.read_line(&mut h)?;
        if h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).to_string();
    log.lock()
        .unwrap()
        .push((method.clone(), target.clone(), body));
    let path = target.split('?').next().unwrap_or("").to_string();
    let (status, value) = routes.get(&(method, path)).cloned().unwrap_or((
        404,
        json!({"error":{"code":"not_found","message":"no such route"}}),
    ));
    let text = if status == 204 {
        String::new()
    } else {
        value.to_string()
    };
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Status",
    };
    let mut s = stream;
    write!(
        s,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )?;
    s.flush()
}

pub fn site() -> Value {
    json!({
        "id": "s1", "domain": "shop.test", "aliases": [], "path": "/nonexistent/code/shop", "kind": "php",
        "root": "public", "php": "8.3", "https": true, "wildcard": false, "framework": "laravel",
        "env": {}, "created_at": "2026-10-09T12:00:00Z"
    })
}

pub fn routes() -> Routes {
    let mut r = Routes::new();
    let mut add = |m: &str, p: &str, s: u16, v: Value| {
        r.insert((m.to_string(), format!("/api/v1{p}")), (s, v));
    };
    add(
        "GET",
        "/health",
        200,
        json!({"ok": true, "version": "0.1.0"}),
    );
    add("GET", "/sites", 200, json!([site()]));
    add("GET", "/sites/s1", 200, site());
    add("DELETE", "/sites/s1", 204, Value::Null);
    add(
        "GET",
        "/overview",
        200,
        json!({
            "version": "0.1.0", "os": "linux", "arch": "x86_64", "home": "/h", "tld": "test",
            "runtimes": [{"kind": "php", "active": "8.3.14", "installed": ["8.3.14"]}],
            "services": [{"name": "caddy", "status": "running", "port": 80, "version": "2.10.0", "autostart": true, "uptime_secs": 7860}],
            "sites_count": 1, "ca_trusted": true, "dns_configured": false, "http_port": 80, "https_port": 443
        }),
    );
    add(
        "GET",
        "/runtimes/php/versions",
        200,
        json!([
            {"version": "8.4.1", "channel": "stable", "installed": false, "active": false},
            {"version": "8.3.14", "channel": "stable", "installed": true, "active": true}
        ]),
    );
    add(
        "POST",
        "/runtimes/php/install",
        202,
        json!({"job_id": "j1"}),
    );
    add(
        "GET",
        "/jobs/j1",
        200,
        json!({
            "id": "j1", "title": "Install php 8.3", "status": "done", "progress": 1.0,
            "message": "php 8.3.14 installed", "started_at": "2026-10-09T12:00:00Z", "finished_at": "2026-10-09T12:01:00Z"
        }),
    );
    add(
        "GET",
        "/doctor",
        200,
        json!([
            {"id": "caddy", "title": "Caddy", "status": "ok", "detail": "running"},
            {"id": "dns", "title": "DNS", "status": "fail", "detail": "not configured", "fix": "bz dns setup"}
        ]),
    );
    r
}
