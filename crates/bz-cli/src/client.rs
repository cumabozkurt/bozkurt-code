//! Typed blocking HTTP client for the bozkurtd API v1 (`docs/API.md`).
//!
//! Shared by `bz` and `bozkurt-mcp`. It only ever talks to the local listener
//! `http://127.0.0.1:<api_port>/api/v1` and authenticates with the per-install token in
//! `$BOZKURT_HOME/api-token` (read on every request, so a daemon started after the
//! client was built is picked up).

use std::time::Duration;

use bz_core::models::{
    CreateSiteRequest, Detection, DnsStatus, DoctorCheck, Health, Job, JobAccepted, LogLines,
    Overview, PortOwner, Runtime, RuntimeKind, Service, ServicePatch, Site, SitePatch, TlsStatus,
    Version,
};
use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::error::CliError;

/// Result of an API call.
pub type ApiResult<T> = Result<T, CliError>;

/// Blocking client bound to one daemon base URL.
#[derive(Clone)]
pub struct Client {
    http: Http,
    /// e.g. `http://127.0.0.1:7878/api/v1`
    base: String,
    port: u16,
    verbose: u8,
    /// Home whose `api-token` is sent as `Authorization: Bearer`.
    token_home: Option<std::path::PathBuf>,
}

impl Client {
    /// Client for `127.0.0.1:<port>`.
    pub fn local(port: u16) -> Client {
        Client::with_base(format!("http://127.0.0.1:{port}/api/v1"), port)
    }

    /// Client for an explicit base URL (tests point this at a mock server).
    pub fn with_base(base: impl Into<String>, port: u16) -> Client {
        let http = Http::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(300))
            .no_proxy()
            .build()
            .expect("http client");
        Client {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            port,
            verbose: 0,
            token_home: None,
        }
    }

    /// Send the token from `<home>/api-token` with every request.
    pub fn with_token_home(mut self, home: impl Into<std::path::PathBuf>) -> Client {
        self.token_home = Some(home.into());
        self
    }

    /// The local API token, if the daemon has created one.
    pub fn token(&self) -> Option<String> {
        self.token_home.as_deref().and_then(bz_core::apitoken::read)
    }

    pub fn set_verbose(&mut self, v: u8) {
        self.verbose = v;
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `ws://.../api/v1/events`.
    pub fn events_url(&self) -> String {
        let b = self
            .base
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1);
        format!("{b}/events")
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.base, path);
        if self.verbose > 0 {
            eprintln!("→ {method} {url}");
        }
        let rb = self.http.request(method, url);
        match self.token() {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    fn send(&self, rb: RequestBuilder) -> ApiResult<Response> {
        let resp = rb.send().map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                CliError::Daemon(e.to_string())
            } else {
                CliError::Other(e.to_string())
            }
        })?;
        if self.verbose > 0 {
            eprintln!("← {}", resp.status());
        }
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let text = resp.text().unwrap_or_default();
        let (code, message) = match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.get("error").is_some() => (
                v["error"]["code"].as_str().unwrap_or("error").to_string(),
                v["error"]["message"].as_str().unwrap_or("").to_string(),
            ),
            _ => (
                match status {
                    404 => "not_found",
                    409 => "conflict",
                    400 | 422 => "invalid_input",
                    _ => "internal",
                }
                .to_string(),
                if text.is_empty() {
                    format!("HTTP {status}")
                } else {
                    text
                },
            ),
        };
        Err(CliError::Api {
            status,
            code,
            message,
        })
    }

    /// Raw call returning JSON (`null` for 204 / empty bodies).
    pub fn call(&self, method: Method, path: &str, body: Option<&Value>) -> ApiResult<Value> {
        let mut rb = self.req(method, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = self.send(rb)?;
        let text = resp.text().map_err(|e| CliError::Other(e.to_string()))?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| CliError::Other(format!("bad JSON from daemon: {e}")))
    }

    fn typed<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> ApiResult<T> {
        let v = self.call(method, path, body)?;
        serde_json::from_value(v)
            .map_err(|e| CliError::Other(format!("unexpected response for {path}: {e}")))
    }

    pub fn get(&self, path: &str) -> ApiResult<Value> {
        self.call(Method::GET, path, None)
    }
    pub fn post(&self, path: &str, body: Value) -> ApiResult<Value> {
        self.call(Method::POST, path, Some(&body))
    }
    pub fn patch(&self, path: &str, body: Value) -> ApiResult<Value> {
        self.call(Method::PATCH, path, Some(&body))
    }
    pub fn delete(&self, path: &str) -> ApiResult<Value> {
        self.call(Method::DELETE, path, None)
    }

    /// Quick liveness probe with a short timeout.
    pub fn health(&self) -> ApiResult<Health> {
        let rb = self
            .req(Method::GET, "/health")
            .timeout(Duration::from_millis(1500));
        let resp = self.send(rb)?;
        resp.json().map_err(|e| CliError::Daemon(e.to_string()))
    }

    // ---- typed endpoints -------------------------------------------------------------

    pub fn overview(&self) -> ApiResult<Overview> {
        self.typed(Method::GET, "/overview", None)
    }
    pub fn runtimes(&self) -> ApiResult<Vec<Runtime>> {
        self.typed(Method::GET, "/runtimes", None)
    }
    pub fn versions(&self, kind: RuntimeKind, refresh: bool) -> ApiResult<Vec<Version>> {
        let q = if refresh { "?refresh=true" } else { "" };
        self.typed(Method::GET, &format!("/runtimes/{kind}/versions{q}"), None)
    }
    pub fn install_runtime(&self, kind: RuntimeKind, version: &str) -> ApiResult<JobAccepted> {
        self.install_runtime_with(kind, version, false)
    }
    /// `allow_unverified`: accept (and pin) an asset without a published checksum (P1-1).
    pub fn install_runtime_with(
        &self,
        kind: RuntimeKind,
        version: &str,
        allow_unverified: bool,
    ) -> ApiResult<JobAccepted> {
        let body = if allow_unverified {
            json!({ "version": version, "allow_unverified": true })
        } else {
            json!({ "version": version })
        };
        self.typed(
            Method::POST,
            &format!("/runtimes/{kind}/install"),
            Some(&body),
        )
    }
    pub fn uninstall_runtime(&self, kind: RuntimeKind, version: &str) -> ApiResult<Value> {
        self.delete(&format!("/runtimes/{kind}/{}", enc(version)))
    }
    pub fn use_runtime(&self, kind: RuntimeKind, version: &str) -> ApiResult<Runtime> {
        self.typed(
            Method::POST,
            &format!("/runtimes/{kind}/use"),
            Some(&json!({ "version": version })),
        )
    }
    pub fn services(&self) -> ApiResult<Vec<Service>> {
        self.typed(Method::GET, "/services", None)
    }
    pub fn install_service(&self, name: &str, version: Option<&str>) -> ApiResult<JobAccepted> {
        let body = match version {
            Some(v) => json!({ "version": v }),
            None => json!({}),
        };
        self.typed(
            Method::POST,
            &format!("/services/{}/install", enc(name)),
            Some(&body),
        )
    }
    /// `action` ∈ start/stop/restart. For `all` the API returns `Service[]`.
    pub fn service_action(&self, name: &str, action: &str) -> ApiResult<Value> {
        self.post(&format!("/services/{}/{action}", enc(name)), json!({}))
    }
    pub fn configure_service(&self, name: &str, patch: &ServicePatch) -> ApiResult<Service> {
        let body = serde_json::to_value(patch).unwrap_or_default();
        self.typed(
            Method::PATCH,
            &format!("/services/{}", enc(name)),
            Some(&body),
        )
    }
    pub fn service_logs(&self, name: &str, lines: usize) -> ApiResult<LogLines> {
        self.typed(
            Method::GET,
            &format!("/services/{}/logs?lines={lines}", enc(name)),
            None,
        )
    }
    pub fn sites(&self) -> ApiResult<Vec<Site>> {
        self.typed(Method::GET, "/sites", None)
    }
    pub fn site(&self, id: &str) -> ApiResult<Site> {
        self.typed(Method::GET, &format!("/sites/{}", enc(id)), None)
    }
    pub fn create_site(&self, req: &CreateSiteRequest) -> ApiResult<Site> {
        let body = serde_json::to_value(req).unwrap_or_default();
        self.typed(Method::POST, "/sites", Some(&body))
    }
    pub fn patch_site(&self, id: &str, patch: &SitePatch) -> ApiResult<Site> {
        let body = serde_json::to_value(patch).unwrap_or_default();
        self.typed(Method::PATCH, &format!("/sites/{}", enc(id)), Some(&body))
    }
    pub fn delete_site(&self, id: &str) -> ApiResult<Value> {
        self.delete(&format!("/sites/{}", enc(id)))
    }
    pub fn site_action(&self, id: &str, action: &str) -> ApiResult<Site> {
        self.typed(
            Method::POST,
            &format!("/sites/{}/{action}", enc(id)),
            Some(&json!({})),
        )
    }
    pub fn site_logs(&self, id: &str, lines: usize) -> ApiResult<LogLines> {
        self.typed(
            Method::GET,
            &format!("/sites/{}/logs?lines={lines}", enc(id)),
            None,
        )
    }
    pub fn detect(&self, path: &str) -> ApiResult<Detection> {
        self.typed(Method::POST, "/detect", Some(&json!({ "path": path })))
    }
    pub fn dns(&self) -> ApiResult<DnsStatus> {
        self.typed(Method::GET, "/dns", None)
    }
    pub fn tls(&self) -> ApiResult<TlsStatus> {
        self.typed(Method::GET, "/tls", None)
    }
    pub fn port_owner(&self, port: u16) -> ApiResult<PortOwner> {
        self.typed(Method::GET, &format!("/ports/{port}"), None)
    }
    pub fn doctor(&self) -> ApiResult<Vec<DoctorCheck>> {
        self.typed(Method::GET, "/doctor", None)
    }
    pub fn jobs(&self) -> ApiResult<Vec<Job>> {
        self.typed(Method::GET, "/jobs", None)
    }
    pub fn job(&self, id: &str) -> ApiResult<Job> {
        self.typed(Method::GET, &format!("/jobs/{}", enc(id)), None)
    }
    /// POST that is expected to answer `202 {job_id}`.
    pub fn post_job(&self, path: &str, body: Value) -> ApiResult<JobAccepted> {
        self.typed(Method::POST, path, Some(&body))
    }
}

/// Percent-encode a path segment (ids, versions, names are simple, but be safe).
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_segments() {
        assert_eq!(enc("shop.test"), "shop.test");
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn events_url() {
        assert_eq!(
            Client::local(7878).events_url(),
            "ws://127.0.0.1:7878/api/v1/events"
        );
    }
}
