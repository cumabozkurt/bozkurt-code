//! Wire types of the bozkurtd HTTP API v1 (`docs/API.md`).
//!
//! Every type here serializes to exactly the JSON shape documented in the API contract
//! (snake_case field names, optional fields omitted when absent). The daemon, CLI and MCP
//! server reuse these types so there is one definition of the protocol.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A language runtime BOZKURT can install and switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Php,
    Node,
    Python,
    Bun,
    Go,
}

impl RuntimeKind {
    /// All runtime kinds, in display order.
    pub const ALL: [RuntimeKind; 5] = [
        RuntimeKind::Php,
        RuntimeKind::Node,
        RuntimeKind::Python,
        RuntimeKind::Bun,
        RuntimeKind::Go,
    ];

    /// Lowercase identifier used in paths and URLs (`php`, `node`, ...).
    pub fn as_str(&self) -> &'static str {
        match self {
            RuntimeKind::Php => "php",
            RuntimeKind::Node => "node",
            RuntimeKind::Python => "python",
            RuntimeKind::Bun => "bun",
            RuntimeKind::Go => "go",
        }
    }
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RuntimeKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "php" => Ok(RuntimeKind::Php),
            "node" | "nodejs" => Ok(RuntimeKind::Node),
            "python" | "py" => Ok(RuntimeKind::Python),
            "bun" => Ok(RuntimeKind::Bun),
            "go" | "golang" => Ok(RuntimeKind::Go),
            other => Err(format!("unknown runtime kind `{other}`")),
        }
    }
}

/// Lifecycle status of a service or app process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
    External,
    NotInstalled,
}

/// Release channel of a runtime version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Lts,
    Current,
    Stable,
    Eol,
}

/// One installable version of a runtime (`GET /runtimes/{kind}/versions`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub version: String,
    pub channel: Channel,
    pub installed: bool,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Installed state of one runtime kind (`GET /runtimes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    pub kind: RuntimeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    pub installed: Vec<String>,
}

/// A managed service (`GET /services`). `name` is one of `caddy`, `dns`, `mariadb`,
/// `postgres`, `redis`, `mailpit` or `php-fpm-<major.minor>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub status: ServiceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub autostart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// How a site is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    /// `php_fastcgi` to a per-version PHP-FPM (php-cgi on Windows) pool.
    Php,
    /// Reverse proxy to a user-run port.
    Proxy,
    /// Plain file server.
    Static,
    /// BOZKURT runs the dev command, assigns `PORT`, and proxies to it.
    App,
}

impl SiteKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SiteKind::Php => "php",
            SiteKind::Proxy => "proxy",
            SiteKind::Static => "static",
            SiteKind::App => "app",
        }
    }
}

impl FromStr for SiteKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "php" => Ok(SiteKind::Php),
            "proxy" => Ok(SiteKind::Proxy),
            "static" => Ok(SiteKind::Static),
            "app" => Ok(SiteKind::App),
            other => Err(format!("unknown site kind `{other}`")),
        }
    }
}

/// A local site (`GET /sites`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Site {
    pub id: String,
    /// Full domain including the TLD, e.g. `shop.test`.
    pub domain: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Project directory.
    pub path: String,
    pub kind: SiteKind,
    /// Document root relative to `path` (or absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_status: Option<ServiceStatus>,
    pub https: bool,
    pub wildcard: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub created_at: String,
}

/// Status of a background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
}

/// A long-running operation (`GET /jobs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub title: String,
    pub status: JobStatus,
    /// 0.0 ..= 1.0
    pub progress: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// Dashboard summary (`GET /overview`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub home: String,
    pub tld: String,
    pub runtimes: Vec<Runtime>,
    pub services: Vec<Service>,
    pub sites_count: usize,
    pub ca_trusted: bool,
    pub dns_configured: bool,
    pub http_port: u16,
    pub https_port: u16,
}

/// Result level of a doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// One diagnosis (`GET /doctor`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub id: String,
    pub title: String,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Who holds a TCP port (`GET /ports/{port}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortOwner {
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    pub ours: bool,
}

/// Result of project detection (`POST /detect`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    pub kind: SiteKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub suggested_domain: String,
}

/// `GET /dns`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsStatus {
    pub tld: String,
    pub port: u16,
    pub configured: bool,
    pub running: bool,
}

/// `GET /tls`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TlsStatus {
    pub ca_path: String,
    pub ca_trusted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_expires: Option<String>,
}

// ---------------------------------------------------------------------------------------
// Request / response bodies
// ---------------------------------------------------------------------------------------

/// `GET /health`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
}

/// `202 {"job_id": "..."}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobAccepted {
    pub job_id: String,
}

/// Body of `POST /runtimes/{kind}/install`, `POST /runtimes/{kind}/use`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionRequest {
    pub version: String,
}

/// Body of `POST /services/{name}/install`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceInstallRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Body of `PATCH /services/{name}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServicePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// `{lines: [...]}` returned by the log endpoints.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogLines {
    pub lines: Vec<String>,
}

/// Body of `POST /sites`. Missing fields are filled by detection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CreateSiteRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SiteKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wildcard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
}

/// Body of `PATCH /sites/{id}` (partial Site). `id`, `created_at`, `app_port` and
/// `app_status` are not patchable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SitePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SiteKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub php: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wildcard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
}

/// Body of `POST /detect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectRequest {
    pub path: String,
}

/// Inner part of the error envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorInner {
    pub code: String,
    pub message: String,
}

/// Error envelope `{"error": {"code", "message"}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: ApiErrorInner,
}

/// Actions for `POST /services/{name}/{action}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

impl FromStr for ServiceAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "restart" => Ok(Self::Restart),
            o => Err(format!("unknown service action `{o}`")),
        }
    }
}

/// Actions for `POST /sites/{id}/{action}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteAction {
    Start,
    Stop,
    Restart,
    Secure,
    Unsecure,
}

impl FromStr for SiteAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "restart" => Ok(Self::Restart),
            "secure" => Ok(Self::Secure),
            "unsecure" => Ok(Self::Unsecure),
            o => Err(format!("unknown site action `{o}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_serialize_like_api() {
        assert_eq!(serde_json::to_string(&RuntimeKind::Php).unwrap(), "\"php\"");
        assert_eq!(
            serde_json::to_string(&ServiceStatus::NotInstalled).unwrap(),
            "\"not_installed\""
        );
        assert_eq!(serde_json::to_string(&Channel::Lts).unwrap(), "\"lts\"");
        assert_eq!(serde_json::to_string(&SiteKind::App).unwrap(), "\"app\"");
        assert_eq!(
            serde_json::to_string(&CheckStatus::Warn).unwrap(),
            "\"warn\""
        );
        assert_eq!(serde_json::to_string(&JobStatus::Done).unwrap(), "\"done\"");
    }

    #[test]
    fn optional_fields_are_omitted() {
        let v = Version {
            version: "22.1.0".into(),
            channel: Channel::Lts,
            installed: false,
            active: false,
            released: None,
            size_bytes: None,
        };
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!({"version":"22.1.0","channel":"lts","installed":false,"active":false})
        );
    }

    #[test]
    fn site_roundtrip() {
        let json = serde_json::json!({
            "id": "shop", "domain": "shop.test", "aliases": [], "path": "/p/shop", "kind": "php",
            "root": "public", "php": "8.3", "https": true, "wildcard": false,
            "framework": "laravel", "env": {"APP_ENV": "local"}, "created_at": "2026-01-01T00:00:00Z"
        });
        let s: Site = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(s.kind, SiteKind::Php);
        assert_eq!(serde_json::to_value(&s).unwrap(), json);
    }

    #[test]
    fn kind_parsing() {
        assert_eq!("NodeJS".parse::<RuntimeKind>().unwrap(), RuntimeKind::Node);
        assert!("ruby".parse::<RuntimeKind>().is_err());
        assert_eq!("secure".parse::<SiteAction>().unwrap(), SiteAction::Secure);
    }
}
