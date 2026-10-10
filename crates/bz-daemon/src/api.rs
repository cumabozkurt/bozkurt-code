//! HTTP API v1 (`docs/API.md`): router, handlers, remote auth middleware, CORS and the
//! `/events` WebSocket.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequest, Path, Query, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use bz_core::events::Event;
use bz_core::models::*;
use bz_core::Engine;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::error::{ApiErr, ApiResult};
use crate::state::{Scope, Shared};
use crate::{databases, import, remote, server};

/// Origins allowed by CORS and by the Origin check (both listeners): the Tauri webviews
/// (`tauri://localhost` on macOS/iOS/Linux, `http(s)://tauri.localhost` on Windows/Android).
pub const CORS_ORIGINS: [&str; 3] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
];

/// Development origins (desktop `tauri dev` on :1420/:1430, the `packages/ui` playground on
/// :5173). Allowed only in debug builds or when the daemon runs with `BOZKURT_DEV=1`.
pub const DEV_ORIGINS: [&str; 6] = [
    "http://localhost:1420",
    "http://127.0.0.1:1420",
    "http://localhost:1430",
    "http://127.0.0.1:1430",
    "http://localhost:5173",
    "http://127.0.0.1:5173",
];

/// Whether the dev origins are allowed in this process.
pub fn dev_origins_enabled() -> bool {
    cfg!(debug_assertions)
        || std::env::var("BOZKURT_DEV").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Every origin a browser request may come from.
pub fn allowed_origins() -> Vec<&'static str> {
    let mut v = CORS_ORIGINS.to_vec();
    if dev_origins_enabled() {
        v.extend(DEV_ORIGINS);
    }
    v
}

fn origin_allowed(o: &str) -> bool {
    allowed_origins().iter().any(|a| a.eq_ignore_ascii_case(o))
}

// ------------------------------------------------------------------------------------
// extractors
// ------------------------------------------------------------------------------------

/// JSON body with envelope errors; an empty body is read as `{}`.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = ApiErr;
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let json_ct = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_json_content_type);
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| ApiErr::bad_request(format!("reading body: {e}")))?;
        let slice: &[u8] = if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            b"{}"
        } else if !json_ct {
            // A browser can send a "simple" cross-site request (text/plain, form) without
            // a CORS preflight; JSON bodies must say so.
            return Err(unsupported_media_type());
        } else {
            &bytes
        };
        serde_json::from_slice(slice)
            .map(Body)
            .map_err(|e| ApiErr::bad_request(format!("invalid JSON body: {e}")))
    }
}

/// `application/json` or `application/<x>+json`, with optional parameters.
pub fn is_json_content_type(v: &str) -> bool {
    let m = v
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    m == "application/json" || (m.starts_with("application/") && m.ends_with("+json"))
}

fn unsupported_media_type() -> ApiErr {
    ApiErr::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported_media_type",
        "request bodies must be sent as Content-Type: application/json",
    )
}

type Q = Query<HashMap<String, String>>;

fn lines_param(q: &HashMap<String, String>) -> usize {
    q.get("lines")
        .and_then(|l| l.parse().ok())
        .unwrap_or(200usize)
        .clamp(1, 10_000)
}

fn parse<T: FromStr<Err = String>>(s: &str) -> ApiResult<T> {
    s.parse::<T>().map_err(ApiErr::bad_request)
}

fn accepted(job_id: String) -> Response {
    (StatusCode::ACCEPTED, Json(JobAccepted { job_id })).into_response()
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

// ------------------------------------------------------------------------------------
// router
// ------------------------------------------------------------------------------------

/// The `/api/v1` router for one listener.
pub fn router(state: Shared, scope: Scope) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/overview", get(overview))
        .route("/runtimes", get(runtimes))
        .route("/runtimes/{kind}/versions", get(runtime_versions))
        .route(
            "/runtimes/{kind}/{arg}",
            post(runtime_action).delete(runtime_uninstall),
        )
        .route("/services", get(services))
        .route(
            "/services/{name}",
            axum::routing::patch(service_patch).get(service_get),
        )
        .route(
            "/services/{name}/{action}",
            post(service_post).get(service_sub),
        )
        .route("/services/{name}/databases/{db}", delete(database_drop))
        .route("/sites", get(sites).post(site_create))
        .route(
            "/sites/{id}",
            get(site_get).patch(site_patch).delete(site_delete),
        )
        .route("/sites/{id}/{action}", post(site_action).get(site_sub))
        .route("/detect", post(detect))
        .route("/dns", get(dns))
        .route("/dns/{action}", post(dns_action))
        .route("/tls", get(tls))
        .route("/tls/{action}", post(tls_action))
        .route("/ports/{port}", get(port_owner))
        .route("/doctor", get(doctor))
        .route("/doctor/fix/{id}", post(doctor_fix))
        .route("/jobs", get(jobs))
        .route("/jobs/{id}", get(job))
        .route("/config", get(config_get).patch(config_patch))
        .route("/import/scan", get(import_scan))
        .route("/import", post(import_apply))
        .route(
            "/share/{site_id}",
            post(share_start).delete(share_stop).get(share_get),
        )
        .route("/remote", get(remote_get))
        .route("/remote/{action}", post(remote_action))
        .route("/remote/devices/{id}", delete(remote_device_delete))
        .route("/daemon/shutdown", post(daemon_shutdown))
        .route("/events", get(events));
    let mut app = Router::new()
        .nest("/api/v1", api)
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(Extension(scope));
    if scope == Scope::Remote {
        app = app.layer(middleware::from_fn_with_state(state.clone(), remote_auth));
    }
    app.layer(middleware::from_fn_with_state(
        (state.clone(), scope),
        guard,
    ))
    .layer(cors())
    .with_state(state)
}

fn header_str(req: &Request, name: impl axum::http::header::AsHeaderName) -> Option<&str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

/// The `?token=` query value (browsers cannot set headers on a WebSocket).
fn query_token(req: &Request) -> Option<String> {
    req.uri().query().and_then(|q| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == "token")
            .map(|(_, v)| v.to_string())
    })
}

/// `Authorization: Bearer <t>`, then `X-Bozkurt-Token: <t>`.
fn header_token(req: &Request) -> Option<String> {
    header_str(req, header::AUTHORIZATION)
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .or_else(|| header_str(req, bz_core::apitoken::HEADER))
        .map(|t| t.trim().to_string())
}

/// Whether `host` (the Host header) names the local listener itself: `127.0.0.1:<port>`,
/// `localhost:<port>` or `[::1]:<port>`. Anything else is a DNS-rebinding attempt.
pub fn host_allowed(host: &str, port: u16) -> bool {
    let host = host.trim().to_ascii_lowercase();
    let (name, p) = match host.rsplit_once(':') {
        Some((n, p)) if !n.is_empty() && !p.contains(']') => (n.to_string(), p.parse::<u16>().ok()),
        _ => (host.clone(), None),
    };
    // HTTP clients omit the port only for :80
    let port_ok = p.unwrap_or(80) == port;
    port_ok && matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

/// Local API protection (`docs/API.md` → "Local API protection"), on both listeners:
/// - a present `Origin` must be allow-listed (every method, including the WebSocket
///   upgrade) — stops CSRF and cross-site WebSocket reads;
/// - mutating requests carrying a non-JSON `Content-Type` are refused (no preflight-free
///   "simple" requests);
/// - local listener only: the `Host` must be the listener itself (DNS rebinding), and a
///   request without an allow-listed `Origin` (CLI, MCP, scripts, other local users) must
///   carry the per-install token from `$BOZKURT_HOME/api-token`. `GET /health` and CORS
///   preflights are exempt.
async fn guard(State((st, scope)): State<(Shared, Scope)>, req: Request, next: Next) -> Response {
    let origin = header_str(&req, header::ORIGIN).map(str::to_string);
    if let Some(o) = &origin {
        if !origin_allowed(o) {
            return ApiErr::forbidden(format!("origin {o} is not allowed")).into_response();
        }
    }
    let m = req.method().clone();
    if matches!(
        m,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        if let Some(ct) = header_str(&req, header::CONTENT_TYPE) {
            if !is_json_content_type(ct) {
                return unsupported_media_type().into_response();
            }
        }
    }
    if scope == Scope::Local {
        let host = header_str(&req, header::HOST).unwrap_or("");
        if !host_allowed(host, st.local_port) {
            return ApiErr::new(
                StatusCode::MISDIRECTED_REQUEST,
                "forbidden",
                format!(
                    "host {host:?} is not this daemon (use 127.0.0.1:{})",
                    st.local_port
                ),
            )
            .into_response();
        }
        let path = req.uri().path();
        let exempt = m == Method::OPTIONS
            || (m == Method::GET && path == "/api/v1/health")
            || origin.is_some();
        if !exempt {
            let mut token = header_token(&req);
            if token.is_none() && path == "/api/v1/events" {
                token = query_token(&req);
            }
            match token {
                Some(t) if bz_core::apitoken::matches(&st.api_token, &t) => {}
                Some(_) => {
                    return ApiErr::unauthorized("invalid local API token").into_response();
                }
                None => {
                    return ApiErr::unauthorized(
                        "missing local API token (send Authorization: Bearer <token> from $BOZKURT_HOME/api-token)",
                    )
                    .into_response();
                }
            }
        }
    }
    next.run(req).await
}

/// CORS per the v1 additions.
pub fn cors() -> CorsLayer {
    let origins: Vec<HeaderValue> = allowed_origins()
        .into_iter()
        .map(HeaderValue::from_static)
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::HeaderName::from_static(bz_core::apitoken::HEADER),
        ])
        .max_age(std::time::Duration::from_secs(600))
}

async fn not_found(req: Request) -> ApiErr {
    ApiErr::not_found(format!(
        "no route for {} {}",
        req.method(),
        req.uri().path()
    ))
}

async fn method_not_allowed(req: Request) -> ApiErr {
    ApiErr::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        format!("{} is not allowed on {}", req.method(), req.uri().path()),
    )
}

/// Bearer-token auth on the remote listener. `POST /remote/pair` is open; `/events`
/// also accepts `?token=` (browsers cannot set WebSocket headers).
async fn remote_auth(State(st): State<Shared>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if req.method() == Method::OPTIONS
        || (req.method() == Method::POST && path == "/api/v1/remote/pair")
    {
        return next.run(req).await;
    }
    let mut token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(|t| t.trim().to_string());
    if token.is_none() && path == "/api/v1/events" {
        token = query_token(&req);
    }
    match token {
        Some(t) if st.remote_auth.authorize(&t).is_some() => next.run(req).await,
        Some(_) => ApiErr::unauthorized("invalid or revoked token").into_response(),
        None => {
            ApiErr::unauthorized("missing bearer token (pair this device first)").into_response()
        }
    }
}

// ------------------------------------------------------------------------------------
// health / overview / runtimes
// ------------------------------------------------------------------------------------

/// `POST /daemon/shutdown` (local listener only): graceful stop, same path as SIGTERM.
async fn daemon_shutdown(
    State(st): State<Shared>,
    Extension(scope): Extension<Scope>,
) -> ApiResult<Response> {
    if scope != Scope::Local {
        return Err(ApiErr::forbidden(
            "the daemon can only be stopped from this computer",
        ));
    }
    let st2 = st.clone();
    tokio::spawn(async move {
        // let the 202 reach the client first
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        st2.shutdown_request.notify_one();
    });
    Ok((StatusCode::ACCEPTED, Json(json!({"stopping": true}))).into_response())
}

async fn health(State(st): State<Shared>) -> Json<Health> {
    Json(st.engine.health())
}

async fn overview(State(st): State<Shared>) -> ApiResult<Json<Overview>> {
    Ok(Json(st.engine.overview().await?))
}

async fn runtimes(State(st): State<Shared>) -> Json<Vec<Runtime>> {
    Json(st.engine.runtimes())
}

async fn runtime_versions(
    State(st): State<Shared>,
    Path(kind): Path<String>,
    Query(q): Q,
) -> ApiResult<Json<Vec<Version>>> {
    let kind: RuntimeKind = parse(&kind)?;
    let refresh = q.get("refresh").is_some_and(|v| v == "true" || v == "1");
    Ok(Json(st.engine.runtime_versions(kind, refresh).await?))
}

#[derive(Deserialize)]
struct VersionBody {
    version: Option<String>,
    /// P1-1: accept (and pin) an asset without a published checksum for this install.
    #[serde(default)]
    allow_unverified: bool,
}

async fn runtime_action(
    State(st): State<Shared>,
    Path((kind, action)): Path<(String, String)>,
    Body(b): Body<VersionBody>,
) -> ApiResult<Response> {
    let kind: RuntimeKind = parse(&kind)?;
    let version = b
        .version
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ApiErr::bad_request("`version` is required"))?;
    match action.as_str() {
        "install" => Ok(accepted(
            st.engine
                .install_runtime_with(kind, &version, b.allow_unverified)
                .job_id,
        )),
        "use" => Ok(Json(st.engine.use_runtime(kind, &version)?).into_response()),
        o => Err(ApiErr::not_found(format!("unknown runtime action `{o}`"))),
    }
}

async fn runtime_uninstall(
    State(st): State<Shared>,
    Path((kind, version)): Path<(String, String)>,
) -> ApiResult<Response> {
    let kind: RuntimeKind = parse(&kind)?;
    st.engine.uninstall_runtime(kind, &version).await?;
    Ok(no_content())
}

// ------------------------------------------------------------------------------------
// services (+ databases)
// ------------------------------------------------------------------------------------

async fn services(State(st): State<Shared>) -> Json<Vec<Service>> {
    Json(st.engine.services().await)
}

async fn service_get(
    State(st): State<Shared>,
    Path(name): Path<String>,
) -> ApiResult<Json<Service>> {
    Ok(Json(st.engine.service(&name).await?))
}

async fn service_patch(
    State(st): State<Shared>,
    Path(name): Path<String>,
    Body(p): Body<ServicePatch>,
) -> ApiResult<Json<Service>> {
    Ok(Json(st.engine.patch_service(&name, &p).await?))
}

async fn service_post(
    State(st): State<Shared>,
    Path((name, action)): Path<(String, String)>,
    Body(b): Body<Value>,
) -> ApiResult<Response> {
    match action.as_str() {
        "install" => {
            let version = b
                .get("version")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            Ok(accepted(st.engine.install_service(&name, version)?.job_id))
        }
        "databases" => {
            let db = b
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiErr::bad_request("`name` is required"))?
                .to_string();
            databases::create(&st.engine, &name, &db).await?;
            Ok((StatusCode::CREATED, Json(json!({ "name": db }))).into_response())
        }
        a => {
            let action: ServiceAction = parse(a)?;
            if name == "all" {
                Ok(Json(st.engine.all_services_action(action).await?).into_response())
            } else {
                Ok(Json(st.engine.service_action(&name, action).await?).into_response())
            }
        }
    }
}

async fn service_sub(
    State(st): State<Shared>,
    Path((name, sub)): Path<(String, String)>,
    Query(q): Q,
) -> ApiResult<Response> {
    match sub.as_str() {
        "logs" => Ok(Json(st.engine.service_logs(&name, lines_param(&q))?).into_response()),
        "databases" => {
            let list = databases::list(&st.engine, &name).await?;
            Ok(Json(json!({ "databases": list })).into_response())
        }
        o => Err(ApiErr::not_found(format!("unknown service resource `{o}`"))),
    }
}

async fn database_drop(
    State(st): State<Shared>,
    Path((name, db)): Path<(String, String)>,
) -> ApiResult<Response> {
    databases::drop(&st.engine, &name, &db).await?;
    Ok(no_content())
}

// ------------------------------------------------------------------------------------
// sites
// ------------------------------------------------------------------------------------

async fn sites(State(st): State<Shared>) -> Json<Vec<Site>> {
    Json(st.engine.sites())
}

async fn site_create(
    State(st): State<Shared>,
    Body(r): Body<CreateSiteRequest>,
) -> ApiResult<Response> {
    let site = st.engine.create_site(&r).await?;
    Ok((StatusCode::CREATED, Json(site)).into_response())
}

async fn site_get(State(st): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Site>> {
    Ok(Json(st.engine.site(&id)?))
}

async fn site_patch(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Body(p): Body<SitePatch>,
) -> ApiResult<Json<Site>> {
    Ok(Json(st.engine.update_site(&id, &p).await?))
}

async fn site_delete(State(st): State<Shared>, Path(id): Path<String>) -> ApiResult<Response> {
    let _ = st.shares.stop(&st.engine, &id).await;
    st.engine.delete_site(&id).await?;
    Ok(no_content())
}

async fn site_action(
    State(st): State<Shared>,
    Path((id, action)): Path<(String, String)>,
) -> ApiResult<Json<Site>> {
    let a: SiteAction = parse(&action)?;
    Ok(Json(st.engine.site_action(&id, a).await?))
}

async fn site_sub(
    State(st): State<Shared>,
    Path((id, sub)): Path<(String, String)>,
    Query(q): Q,
) -> ApiResult<Json<LogLines>> {
    match sub.as_str() {
        "logs" => Ok(Json(st.engine.site_logs(&id, lines_param(&q))?)),
        o => Err(ApiErr::not_found(format!("unknown site resource `{o}`"))),
    }
}

async fn detect(
    State(st): State<Shared>,
    Body(r): Body<DetectRequest>,
) -> ApiResult<Json<Detection>> {
    Ok(Json(st.engine.detect(&r.path)?))
}

// ------------------------------------------------------------------------------------
// dns / tls / ports
// ------------------------------------------------------------------------------------

async fn dns(State(st): State<Shared>) -> ApiResult<Json<DnsStatus>> {
    Ok(Json(st.engine.dns_status().await?))
}

async fn dns_action(
    State(st): State<Shared>,
    Path(action): Path<String>,
    Body(b): Body<Value>,
) -> ApiResult<Response> {
    match action.as_str() {
        "configure" => {
            let tld = b
                .get("tld")
                .and_then(|v| v.as_str())
                .filter(|t| !t.is_empty())
                .map(str::to_string);
            Ok(accepted(st.engine.configure_dns(tld).job_id))
        }
        "unconfigure" => Ok(accepted(unconfigure_dns_job(&st))),
        o => Err(ApiErr::not_found(format!("unknown dns action `{o}`"))),
    }
}

fn unconfigure_dns_job(st: &Shared) -> String {
    let net = st.bznet.clone();
    let tld = st.engine.config().tld;
    st.engine
        .jobs_manager()
        .spawn("Remove DNS routing", move |h| async move {
            h.progress(0.2, Some(&format!("Removing .{tld} routing")));
            if let Some(n) = net {
                n.unconfigure_dns(&tld).await?;
            }
            Ok(Some(format!(".{tld} is no longer routed to BOZKURT")))
        })
}

async fn tls(State(st): State<Shared>) -> ApiResult<Json<TlsStatus>> {
    Ok(Json(st.engine.tls_status().await?))
}

fn trust_job(st: &Shared, system: bool, trust: bool) -> String {
    let net = st.bznet.clone();
    let engine = st.engine.clone();
    let title = match (trust, system) {
        (true, false) => "Trust local CA",
        (true, true) => "Trust local CA (system store)",
        (false, _) => "Remove local CA trust",
    };
    st.engine.jobs_manager().spawn(title, move |h| async move {
        h.progress(0.1, None);
        match (net, trust) {
            (Some(n), true) => n.trust(system).await?,
            (Some(n), false) => n.untrust(system).await?,
            (None, true) => engine.net().trust_ca().await?,
            (None, false) => {}
        }
        Ok(Some(
            if trust {
                "Local CA trusted"
            } else {
                "Local CA trust removed"
            }
            .into(),
        ))
    })
}

async fn tls_action(
    State(st): State<Shared>,
    Path(action): Path<String>,
    Body(b): Body<Value>,
) -> ApiResult<Response> {
    let system = b.get("system").and_then(|v| v.as_bool()).unwrap_or(false);
    match action.as_str() {
        "trust" => Ok(accepted(trust_job(&st, system, true))),
        "untrust" => Ok(accepted(trust_job(&st, system, false))),
        o => Err(ApiErr::not_found(format!("unknown tls action `{o}`"))),
    }
}

async fn port_owner(State(st): State<Shared>, Path(port): Path<String>) -> ApiResult<Json<Value>> {
    let port: u16 = port
        .parse()
        .map_err(|_| ApiErr::bad_request(format!("invalid port `{port}`")))?;
    let o = st.engine.port_owner(port).await?;
    let in_use = o.pid.is_some()
        || o.ours
        || match &st.bznet {
            Some(n) => n.port_in_use(port),
            None => bz_core::ports::is_listening(port).await,
        };
    let mut v = serde_json::to_value(&o).map_err(|e| ApiErr::internal(e.to_string()))?;
    v["in_use"] = Value::Bool(in_use);
    Ok(Json(v))
}

// ------------------------------------------------------------------------------------
// doctor
// ------------------------------------------------------------------------------------

/// Map CLI-style fix hints to machine fix ids understood by `POST /doctor/fix/{id}`.
pub fn machine_fix(fix: &str) -> Option<String> {
    let f = fix.trim();
    match f {
        "bz dns configure" => return Some("dns.configure".into()),
        "bz tls trust" => return Some("tls.trust".into()),
        _ => {}
    }
    let rest = f.strip_prefix("bz service ")?;
    let mut it = rest.split_whitespace();
    let (action, name) = (it.next()?, it.next()?);
    if it.next().is_some() || !matches!(action, "start" | "stop" | "restart") {
        return None;
    }
    Some(format!("services.{name}.{action}"))
}

async fn doctor(State(st): State<Shared>) -> Json<Vec<DoctorCheck>> {
    let mut checks = st.engine.doctor().await;
    for c in &mut checks {
        if let Some(id) = c.fix.as_deref().and_then(machine_fix) {
            c.fix = Some(id);
        }
    }
    Json(checks)
}

async fn doctor_fix(State(st): State<Shared>, Path(id): Path<String>) -> ApiResult<Response> {
    match id.as_str() {
        "dns.configure" => return Ok(accepted(st.engine.configure_dns(None).job_id)),
        "tls.trust" => return Ok(accepted(trust_job(&st, false, true))),
        _ => {}
    }
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() >= 3 && parts[0] == "services" {
        let action: ServiceAction = parse(parts[parts.len() - 1])?;
        let name = parts[1..parts.len() - 1].join(".");
        st.engine.service(&name).await?; // validate before accepting
        let engine = st.engine.clone();
        let title = format!("{} {name}", parts[parts.len() - 1]);
        let jid = st.engine.jobs_manager().spawn(title, move |_h| async move {
            let s = engine.service_action(&name, action).await?;
            Ok(Some(format!("{} is {:?}", s.name, s.status).to_lowercase()))
        });
        return Ok(accepted(jid));
    }
    Err(ApiErr::not_found(format!("unknown fix `{id}`")))
}

// ------------------------------------------------------------------------------------
// jobs / config
// ------------------------------------------------------------------------------------

async fn jobs(State(st): State<Shared>) -> Json<Vec<Job>> {
    Json(st.engine.jobs())
}

async fn job(State(st): State<Shared>, Path(id): Path<String>) -> ApiResult<Json<Job>> {
    Ok(Json(st.engine.job(&id)?))
}

async fn config_get(State(st): State<Shared>) -> Json<Value> {
    Json(st.engine.config_json())
}

async fn config_patch(
    State(st): State<Shared>,
    Extension(scope): Extension<Scope>,
    Body(p): Body<Value>,
) -> ApiResult<Json<Value>> {
    if scope != Scope::Local {
        // D1: a paired phone must not be able to move the API or its own listener and
        // strand the desktop.
        let cur = st.engine.config();
        if let Some(key) = remote_forbidden_config_key(&cur, &p) {
            return Err(ApiErr::forbidden(format!(
                "{key} can only be changed locally"
            )));
        }
    }
    // bind (or stop) the remote listener before saving, so a failed bind (409) never
    // leaves `remote.enabled = true` on disk
    let before = st.engine.config().remote;
    let after = st.engine.config().patched(&p)?.remote;
    if after != before {
        server::reconcile_remote_to(&st, &after).await?;
    }
    match st.engine.patch_config(&p).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => {
            if after != before {
                let _ = server::reconcile_remote_to(&st, &before).await;
            }
            Err(e.into())
        }
    }
}

/// The first key in a `PATCH /config` body that the remote listener may not change
/// (`api_port`, `remote.port`), when the patch would change it.
fn remote_forbidden_config_key(cur: &bz_core::config::Config, p: &Value) -> Option<&'static str> {
    // an invalid patch falls through to the normal 400
    let new = cur.patched(p).ok()?;
    if new.api_port != cur.api_port {
        Some("api_port")
    } else if new.remote.port != cur.remote.port {
        Some("remote.port")
    } else {
        None
    }
}

// ------------------------------------------------------------------------------------
// import / share
// ------------------------------------------------------------------------------------

async fn import_scan() -> ApiResult<Json<import::ImportScan>> {
    let s = tokio::task::spawn_blocking(|| import::scan(&import::ScanRoots::system()))
        .await
        .map_err(|e| ApiErr::internal(e.to_string()))?;
    Ok(Json(s))
}

async fn import_apply(
    State(st): State<Shared>,
    Body(req): Body<import::ImportScan>,
) -> ApiResult<Response> {
    import::validate(&req)?;
    let engine = st.engine.clone();
    let jid = st
        .engine
        .jobs_manager()
        .spawn("Import sites", move |h| async move {
            let (ok, errs) = import::import(&engine, &req, |p, m| h.progress(p, Some(m))).await?;
            if !errs.is_empty() && ok.is_empty() && !req.vhosts.is_empty() {
                return Err(bz_core::CoreError::InvalidInput(errs.join("; ")));
            }
            let mut msg = format!("Imported {} site(s)", ok.len());
            if !errs.is_empty() {
                msg.push_str(&format!("; skipped: {}", errs.join("; ")));
            }
            let dns = import::dns_notes(&req.dns, &engine.config().tld);
            if !dns.is_empty() {
                msg.push_str(&format!("; DNS not imported: {}", dns.join("; ")));
            }
            Ok(Some(msg))
        });
    Ok(accepted(jid))
}

async fn share_start(State(st): State<Shared>, Path(site_id): Path<String>) -> ApiResult<Response> {
    let site = st.engine.site(&site_id)?;
    let s2 = st.clone();
    let jid =
        st.engine
            .jobs_manager()
            .spawn(format!("Share {}", site.domain), move |h| async move {
                let url = s2.shares.start(&s2.engine, &site_id, &h).await?;
                Ok(Some(url))
            });
    Ok(accepted(jid))
}

async fn share_get(
    State(st): State<Shared>,
    Path(site_id): Path<String>,
) -> ApiResult<Json<Value>> {
    st.engine.site(&site_id)?;
    Ok(Json(
        json!({ "site_id": site_id, "url": st.shares.url(&site_id) }),
    ))
}

async fn share_stop(State(st): State<Shared>, Path(site_id): Path<String>) -> ApiResult<Response> {
    st.shares.stop(&st.engine, &site_id).await?;
    Ok(no_content())
}

// ------------------------------------------------------------------------------------
// remote
// ------------------------------------------------------------------------------------

/// `GET /remote` body.
pub async fn remote_info(st: &Shared) -> Value {
    let cfg = st.engine.config().remote;
    let running = st.remote.lock().await.as_ref().map(|r| r.addr);
    let addresses: Vec<String> = if cfg.enabled {
        remote::lan_addresses()
            .into_iter()
            .map(|a| a.to_string())
            .collect()
    } else {
        vec![]
    };
    json!({
        "enabled": cfg.enabled,
        "running": running.is_some(),
        "port": running.map(|a| a.port()).unwrap_or(cfg.port),
        "addresses": addresses,
        "paired_devices": st.remote_auth.devices(),
    })
}

async fn remote_get(State(st): State<Shared>) -> Json<Value> {
    Json(remote_info(&st).await)
}

#[derive(Deserialize, Default)]
struct PairBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    device_name: String,
}

async fn remote_action(
    State(st): State<Shared>,
    Extension(scope): Extension<Scope>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(action): Path<String>,
    Body(b): Body<Value>,
) -> ApiResult<Response> {
    match action.as_str() {
        "enable" | "disable" => {
            let enabled = action == "enable";
            // bind first; persist `enabled` only once the listener is up (or down)
            let mut want = st.engine.config().remote;
            want.enabled = enabled;
            server::reconcile_remote_to(&st, &want).await?;
            st.engine
                .patch_config(&json!({"remote": {"enabled": enabled}}))
                .await?;
            Ok(Json(remote_info(&st).await).into_response())
        }
        "pairing-code" => {
            if scope != Scope::Local {
                return Err(ApiErr::forbidden(
                    "pairing codes can only be generated on this computer",
                ));
            }
            Ok(Json(st.remote_auth.new_pairing_code()).into_response())
        }
        "pair" => {
            if scope != Scope::Remote {
                return Err(ApiErr::forbidden(
                    "pairing happens on the remote listener (enable remote access first)",
                ));
            }
            let p: PairBody = serde_json::from_value(b)
                .map_err(|e| ApiErr::bad_request(format!("invalid body: {e}")))?;
            match st.remote_auth.pair(peer.ip(), &p.code, &p.device_name) {
                Ok((device_id, token)) => {
                    tracing::info!("paired device `{}` from {}", p.device_name, peer.ip());
                    Ok(Json(json!({ "token": token, "device_id": device_id })).into_response())
                }
                Err(remote::PairError::BadCode) => Err(ApiErr::new(
                    StatusCode::UNAUTHORIZED,
                    "bad_code",
                    "Pairing code is invalid or expired",
                )),
                Err(remote::PairError::RateLimited) => Err(ApiErr::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    "Too many pairing attempts; wait a minute",
                )),
                Err(remote::PairError::Locked) => Err(ApiErr::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "pairing_locked",
                    "Too many wrong pairing codes; the code was revoked. Generate a new one on the computer",
                )),
                Err(remote::PairError::Invalid(m)) => Err(ApiErr::bad_request(m)),
            }
        }
        o => Err(ApiErr::not_found(format!("unknown remote action `{o}`"))),
    }
}

async fn remote_device_delete(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    if st.remote_auth.remove(&id) {
        Ok(no_content())
    } else {
        Err(ApiErr::not_found(format!("device `{id}`")))
    }
}

// ------------------------------------------------------------------------------------
// events
// ------------------------------------------------------------------------------------

/// Rewrite supervisor log sources to the documented `site:<domain>` / `service:<name>`.
pub fn map_event(engine: &Engine, ev: Event) -> Event {
    match ev {
        Event::Log { source, line } => {
            let source = if let Some(id) = source.strip_prefix("site-") {
                match engine.site(id) {
                    Ok(s) => format!("site:{}", s.domain),
                    Err(_) => format!("site:{id}"),
                }
            } else if let Some(id) = source.strip_prefix("share-") {
                format!("share:{id}")
            } else if source.contains(':') {
                source
            } else {
                format!("service:{source}")
            };
            Event::Log { source, line }
        }
        e => e,
    }
}

async fn events(State(st): State<Shared>, ws: WebSocketUpgrade) -> Response {
    // subscribe before the upgrade completes so no event after the 101 is missed
    let rx = st.engine.subscribe();
    let engine = st.engine.clone();
    ws.on_upgrade(move |socket| pump(socket, rx, engine))
}

async fn pump(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<Event>,
    engine: Engine,
) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let ev = map_event(&engine, ev);
                    let Ok(text) = serde_json::to_string(&ev) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!("websocket client lagged {n} events");
                }
                Err(RecvError::Closed) => return,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(Message::Ping(p))) => {
                    let _ = socket.send(Message::Pong(p)).await;
                }
                Some(Ok(_)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes() {
        assert_eq!(
            machine_fix("bz dns configure").as_deref(),
            Some("dns.configure")
        );
        assert_eq!(machine_fix("bz tls trust").as_deref(), Some("tls.trust"));
        assert_eq!(
            machine_fix("bz service start dns").as_deref(),
            Some("services.dns.start")
        );
        assert_eq!(machine_fix("bz service logs caddy"), None);
        assert_eq!(machine_fix("Add to your shell profile"), None);
    }
}
