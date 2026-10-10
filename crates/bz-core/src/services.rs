//! Managed services: definitions, upstream catalogs, install, and process specs.
//!
//! | service | upstream | platforms |
//! |---|---|---|
//! | `caddy` | GitHub `caddyserver/caddy` releases (`caddy_<v>_<mac|linux|windows>_<amd64|arm64>`) | all |
//! | `mariadb` | `downloads.mariadb.org/rest-api` (newest LTS major, sha256 from the release API) | Linux x64 (systemd bintar), Windows x64 zip |
//! | `postgres` | GitHub `theseus-rs/postgresql-binaries` (`postgresql-<v>-<triple>.tar.gz`) | all |
//! | `redis` | Valkey `download.valkey.io` bintar (`valkey-<v>-jammy-<arch>.tar.gz` + `.sha256`) | Linux only |
//! | `mailpit` | GitHub `axllent/mailpit` releases | all |
//! | `php-fpm-X.Y` | PHP runtime's `php-fpm` (static-php.dev) / `php-cgi.exe -b` on Windows | all |
//! | `dns` | in-process responder from [`crate::net::NetProvider`] | all |
//!
//! A service whose port is already taken by a foreign process (e.g. a system MySQL on
//! 3306) reports status `external` and is used as is.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::catalogs::{get_json, gh_asset, gh_to_asset, Asset, DiskCache};
use crate::download::Checksum;
use crate::error::{CoreError, Result};
use crate::paths::Paths;
use crate::platform::{Arch, Os, Platform};
use crate::supervisor::{GracefulStop, ProcessSpec, RestartPolicy};
use crate::version;

/// Upstream endpoints for services.
pub mod urls {
    pub const CADDY_LATEST: &str = "https://api.github.com/repos/caddyserver/caddy/releases/latest";
    pub const MAILPIT_LATEST: &str = "https://api.github.com/repos/axllent/mailpit/releases/latest";
    pub const POSTGRES_RELEASES: &str =
        "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=40";
    pub const MARIADB_MAJORS: &str = "https://downloads.mariadb.org/rest-api/mariadb/";
    pub const MARIADB_API: &str = "https://downloads.mariadb.org/rest-api/mariadb/";
    pub const VALKEY_RELEASES: &str =
        "https://api.github.com/repos/valkey-io/valkey/releases?per_page=30";
    pub const VALKEY_DL: &str = "https://download.valkey.io/releases/";
}

/// Mailpit web UI port (SMTP is the service port).
pub const MAILPIT_UI_PORT: u16 = 8025;

/// A service BOZKURT knows how to manage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceKind {
    Caddy,
    Dns,
    Mariadb,
    Postgres,
    Redis,
    Mailpit,
    /// PHP FastCGI pool for a PHP `major.minor`.
    PhpFpm(String),
}

impl ServiceKind {
    /// Fixed services in display order (PHP pools are added per installed PHP).
    pub const FIXED: [ServiceKind; 6] = [
        ServiceKind::Caddy,
        ServiceKind::Dns,
        ServiceKind::Mariadb,
        ServiceKind::Postgres,
        ServiceKind::Redis,
        ServiceKind::Mailpit,
    ];

    /// API name (`caddy`, `php-fpm-8.3`, ...).
    pub fn name(&self) -> String {
        match self {
            ServiceKind::Caddy => "caddy".into(),
            ServiceKind::Dns => "dns".into(),
            ServiceKind::Mariadb => "mariadb".into(),
            ServiceKind::Postgres => "postgres".into(),
            ServiceKind::Redis => "redis".into(),
            ServiceKind::Mailpit => "mailpit".into(),
            ServiceKind::PhpFpm(v) => format!("php-fpm-{v}"),
        }
    }

    /// Parse an API name. `mysql` is accepted as an alias of `mariadb`, `valkey` of `redis`.
    pub fn parse(name: &str) -> Result<ServiceKind> {
        Ok(match name {
            "caddy" => ServiceKind::Caddy,
            "dns" => ServiceKind::Dns,
            "mariadb" | "mysql" => ServiceKind::Mariadb,
            "postgres" | "postgresql" => ServiceKind::Postgres,
            "redis" | "valkey" => ServiceKind::Redis,
            "mailpit" => ServiceKind::Mailpit,
            n => match n.strip_prefix("php-fpm-") {
                Some(v) if version::parse(v).len() == 2 => ServiceKind::PhpFpm(v.to_string()),
                _ => return Err(CoreError::NotFound(format!("unknown service `{n}`"))),
            },
        })
    }

    /// Default listening port.
    pub fn default_port(&self) -> Option<u16> {
        match self {
            ServiceKind::Caddy => None, // http/https ports come from config
            ServiceKind::Dns => Some(5390),
            ServiceKind::Mariadb => Some(3306),
            ServiceKind::Postgres => Some(5432),
            ServiceKind::Redis => Some(6379),
            ServiceKind::Mailpit => Some(1025),
            ServiceKind::PhpFpm(v) => Some(php_cgi_port(v)),
        }
    }

    /// Services that become `external` when a foreign process holds their port.
    pub fn may_be_external(&self) -> bool {
        matches!(
            self,
            ServiceKind::Mariadb
                | ServiceKind::Postgres
                | ServiceKind::Redis
                | ServiceKind::Mailpit
        )
    }

    /// Supervisor process name.
    pub fn process_name(&self) -> String {
        self.name()
    }
}

impl fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// TCP port of the first php-cgi FastCGI listener on Windows (`8.3` → 9083). A minor of
/// 10 or more (`8.10`, which would collide with `9.0`'s 9090) uses slot 0 of its block
/// from [`php_cgi_ports`] instead.
pub fn php_cgi_port(mm: &str) -> u16 {
    let p = version::parse(mm);
    let (major, minor) = (
        p.first().copied().unwrap_or(8),
        p.get(1).copied().unwrap_or(0),
    );
    if minor < 10 && major < 100 {
        9000 + (major * 10 + minor) as u16
    } else {
        php_cgi_block(mm)
    }
}

/// First port of the block of 8 reserved for PHP `mm` on Windows.
fn php_cgi_block(mm: &str) -> u16 {
    let p = version::parse(mm);
    let k = (p.first().copied().unwrap_or(8) * 100 + p.get(1).copied().unwrap_or(0)) % 1000;
    15000 + (k as u16) * 8
}

/// php-cgi processes per PHP minor on Windows. php-cgi serves one request at a time
/// (`PHP_FCGI_CHILDREN` is not supported on Windows), so a single process serializes all
/// requests and deadlocks a page that calls its own site (P1-13).
pub const PHP_CGI_INSTANCES: usize = 4;

/// Ports of the [`PHP_CGI_INSTANCES`] php-cgi listeners of `mm` on Windows: the first is
/// [`php_cgi_port`], the others are slots 1.. of a block of 8 per minor at
/// 15000 + 8 × (major × 100 + minor), so no two PHP minors ever share a port.
pub fn php_cgi_ports(mm: &str) -> Vec<u16> {
    let block = php_cgi_block(mm);
    let mut v = vec![php_cgi_port(mm)];
    for i in 1..PHP_CGI_INSTANCES {
        v.push(block + i as u16);
    }
    v
}

/// Supervisor names of a PHP pool's processes: `php-fpm-8.3` (the one the API reports),
/// plus `php-fpm-8.3#1` … on Windows.
pub fn php_pool_process_names(mm: &str, p: Platform) -> Vec<String> {
    let base = ServiceKind::PhpFpm(mm.to_string()).process_name();
    let n = if p.is_windows() { PHP_CGI_INSTANCES } else { 1 };
    (0..n)
        .map(|i| {
            if i == 0 {
                base.clone()
            } else {
                format!("{base}#{i}")
            }
        })
        .collect()
}

/// A downloadable service build.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServiceRelease {
    pub version: String,
    pub asset: Asset,
}

fn unsupported(kind: &ServiceKind, p: Platform, why: &str) -> CoreError {
    CoreError::Unsupported(format!("{kind} is not available for {p}: {why}"))
}

// --------------------------------------------------------------------------------------
// catalog parsers (pure)
// --------------------------------------------------------------------------------------

/// Caddy asset from a GitHub release.
pub fn parse_caddy(release: &Value, p: Platform) -> Result<ServiceRelease> {
    let tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Upstream("caddy release without tag".into()))?;
    let v = tag.trim_start_matches('v');
    let os = match p.os {
        Os::MacOs => "mac",
        Os::Linux => "linux",
        Os::Windows => "windows",
    };
    let ext = if p.is_windows() { "zip" } else { "tar.gz" };
    let name = format!("caddy_{v}_{os}_{}.{ext}", p.goarch());
    let asset = gh_asset(release, &name)
        .and_then(|a| gh_to_asset(release, a, &format!("caddy_{v}_checksums.txt")))
        .ok_or_else(|| unsupported(&ServiceKind::Caddy, p, &format!("asset {name} missing")))?;
    Ok(ServiceRelease {
        version: v.to_string(),
        asset,
    })
}

/// Mailpit asset from a GitHub release.
pub fn parse_mailpit(release: &Value, p: Platform) -> Result<ServiceRelease> {
    let tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Upstream("mailpit release without tag".into()))?;
    let os = match p.os {
        Os::MacOs => "darwin",
        Os::Linux => "linux",
        Os::Windows => "windows",
    };
    let ext = if p.is_windows() { "zip" } else { "tar.gz" };
    let name = format!("mailpit-{os}-{}.{ext}", p.goarch());
    let asset = gh_asset(release, &name)
        .and_then(|a| gh_to_asset(release, a, "checksums.txt"))
        .ok_or_else(|| unsupported(&ServiceKind::Mailpit, p, &format!("asset {name} missing")))?;
    Ok(ServiceRelease {
        version: tag.trim_start_matches('v').to_string(),
        asset,
    })
}

/// PostgreSQL build from theseus-rs/postgresql-binaries releases. `want` is a major
/// (`17`), a full version, or `None` for the newest.
pub fn parse_postgres(releases: &Value, p: Platform, want: Option<&str>) -> Result<ServiceRelease> {
    let mut candidates: Vec<(String, &Value)> = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            !r.get("prerelease")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|r| Some((r.get("tag_name")?.as_str()?.to_string(), r)))
        .filter(|(t, _)| t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .collect();
    candidates.sort_by(|a, b| version::compare(&b.0, &a.0));
    for (tag, r) in candidates {
        if let Some(w) = want {
            if tag != w && !version::matches_prefix(&tag, w) {
                continue;
            }
        }
        let name = format!("postgresql-{tag}-{}.tar.gz", p.triple());
        if let Some(a) = gh_asset(r, &name) {
            let mut asset = gh_to_asset(r, a, "")
                .ok_or_else(|| CoreError::Upstream(format!("bad asset {name}")))?;
            if asset.checksum == Checksum::None {
                if let Some(u) = gh_asset(r, &format!("{name}.sha256"))
                    .and_then(|s| s.get("browser_download_url"))
                    .and_then(Value::as_str)
                {
                    asset.checksum = Checksum::SumsFile {
                        url: u.to_string(),
                        filename: name.clone(),
                    };
                }
            }
            return Ok(ServiceRelease {
                version: tag,
                asset,
            });
        }
    }
    Err(unsupported(
        &ServiceKind::Postgres,
        p,
        &format!("no build for {}", want.unwrap_or("latest")),
    ))
}

/// Pick the MariaDB major to install: `want` (`11.8`) or the newest stable LTS.
pub fn mariadb_major(majors: &Value, want: Option<&str>) -> Option<String> {
    let list = majors.get("major_releases")?.as_array()?;
    if let Some(w) = want {
        let mm = version::major_minor(w);
        return list
            .iter()
            .filter_map(|m| m.get("release_id")?.as_str())
            .find(|id| *id == mm)
            .map(str::to_string);
    }
    list.iter()
        .filter(|m| {
            m.get("release_status").and_then(Value::as_str) == Some("Stable")
                && m.get("release_support_type").and_then(Value::as_str)
                    == Some("Long Term Support")
        })
        .filter_map(|m| m.get("release_id")?.as_str())
        .max_by(|a, b| version::compare(a, b))
        .map(str::to_string)
}

/// Pick the binary tarball/zip from a MariaDB release document
/// (`/rest-api/mariadb/<x.y.z>/` → `release_data`, or `/<x.y>/latest/` → `releases`).
pub fn parse_mariadb_release(doc: &Value, p: Platform) -> Result<ServiceRelease> {
    let kind = ServiceKind::Mariadb;
    let (os, cpu, suffix) = match (p.os, p.arch) {
        (Os::Linux, Arch::X64) => ("Linux", "x86_64", "-linux-systemd-x86_64.tar.gz"),
        (Os::Windows, Arch::X64) => ("Windows", "x86_64", "-winx64.zip"),
        _ => {
            return Err(unsupported(
                &kind,
                p,
                "MariaDB publishes official binaries only for Linux x86_64 and Windows x64; \
                 an existing MySQL/MariaDB on :3306 is used as `external`",
            ))
        }
    };
    let rels = doc
        .get("release_data")
        .or_else(|| doc.get("releases"))
        .and_then(Value::as_object)
        .ok_or_else(|| CoreError::Upstream("unexpected MariaDB API response".into()))?;
    let mut ids: Vec<&String> = rels.keys().collect();
    ids.sort_by(|a, b| version::compare(b, a));
    for id in ids {
        let r = &rels[id];
        for f in r
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = f.get("file_name").and_then(Value::as_str).unwrap_or("");
            if f.get("os").and_then(Value::as_str) == Some(os)
                && f.get("cpu").and_then(Value::as_str) == Some(cpu)
                && name.ends_with(suffix)
            {
                let url = f
                    .get("file_download_url")
                    .and_then(Value::as_str)
                    .map(|u| u.replacen("http://", "https://", 1))
                    .unwrap_or_else(|| format!("{}{id}/{name}", urls::MARIADB_API));
                let checksum = f
                    .pointer("/checksum/sha256sum")
                    .and_then(Value::as_str)
                    .map(Checksum::sha256)
                    .unwrap_or(Checksum::None);
                return Ok(ServiceRelease {
                    version: id.clone(),
                    asset: Asset {
                        url,
                        filename: name.to_string(),
                        size: None,
                        checksum,
                    },
                });
            }
        }
    }
    Err(unsupported(&kind, p, "no matching file in the release"))
}

/// Valkey (Redis-compatible) Linux bintar for the newest stable tag (or `want` prefix).
pub fn parse_valkey(releases: &Value, p: Platform, want: Option<&str>) -> Result<ServiceRelease> {
    let kind = ServiceKind::Redis;
    let arch = match (p.os, p.arch) {
        (Os::Linux, Arch::X64) => "x86_64",
        (Os::Linux, Arch::Arm64) => "arm64",
        (Os::MacOs, _) => {
            return Err(unsupported(
                &kind,
                p,
                "Valkey/Redis publish no official macOS binaries; a Redis already listening \
                 on :6379 is used as `external`",
            ))
        }
        (Os::Windows, _) => {
            return Err(unsupported(
                &kind,
                p,
                "Redis/Valkey have no native Windows build (use WSL or a Redis-compatible \
                 server such as Memurai on :6379, detected as `external`)",
            ))
        }
    };
    let mut tags: Vec<String> = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            !r.get("prerelease")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && !r.get("draft").and_then(Value::as_bool).unwrap_or(false)
        })
        .filter_map(|r| r.get("tag_name")?.as_str().map(str::to_string))
        .filter(|t| t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .filter(|t| want.is_none_or(|w| t == w || version::matches_prefix(t, w)))
        .collect();
    version::sort_desc(&mut tags);
    let v = tags
        .first()
        .ok_or_else(|| unsupported(&kind, p, "no stable Valkey release found"))?;
    let name = format!("valkey-{v}-jammy-{arch}.tar.gz");
    Ok(ServiceRelease {
        version: v.clone(),
        asset: Asset {
            url: format!("{}{name}", urls::VALKEY_DL),
            checksum: Checksum::SumsFile {
                url: format!("{}{name}.sha256", urls::VALKEY_DL),
                filename: name.clone(),
            },
            filename: name,
            size: None,
        },
    })
}

/// Fetches service builds (cached 6 h).
#[derive(Debug, Clone)]
pub struct ServiceCatalog {
    client: reqwest::Client,
    cache: DiskCache,
    platform: Platform,
}

impl ServiceCatalog {
    pub fn new(client: reqwest::Client, cache: DiskCache, platform: Platform) -> ServiceCatalog {
        ServiceCatalog {
            client,
            cache,
            platform,
        }
    }

    async fn cached_json(&self, key: &str, url: &str) -> Result<Value> {
        let c = self.client.clone();
        let u = url.to_string();
        self.cache
            .get_or_fetch(key, "any", false, || async move { get_json(&c, &u).await })
            .await
    }

    /// Resolve the build to install for `kind` (`want` = version / major, or latest).
    pub async fn resolve(&self, kind: &ServiceKind, want: Option<&str>) -> Result<ServiceRelease> {
        let p = self.platform;
        match kind {
            ServiceKind::Caddy => {
                parse_caddy(&self.cached_json("svc-caddy", urls::CADDY_LATEST).await?, p)
            }
            ServiceKind::Mailpit => parse_mailpit(
                &self
                    .cached_json("svc-mailpit", urls::MAILPIT_LATEST)
                    .await?,
                p,
            ),
            ServiceKind::Postgres => parse_postgres(
                &self
                    .cached_json("svc-postgres", urls::POSTGRES_RELEASES)
                    .await?,
                p,
                want,
            ),
            ServiceKind::Redis => parse_valkey(
                &self
                    .cached_json("svc-valkey", urls::VALKEY_RELEASES)
                    .await?,
                p,
                want,
            ),
            ServiceKind::Mariadb => {
                // fail fast on unsupported platforms before hitting the network
                if !matches!(
                    (p.os, p.arch),
                    (Os::Linux, Arch::X64) | (Os::Windows, Arch::X64)
                ) {
                    return parse_mariadb_release(&Value::Null, p);
                }
                let majors = self
                    .cached_json("svc-mariadb-majors", urls::MARIADB_MAJORS)
                    .await?;
                let major = mariadb_major(&majors, want).ok_or_else(|| {
                    CoreError::NotFound(format!("MariaDB {} not found", want.unwrap_or("LTS")))
                })?;
                let latest = self
                    .cached_json(
                        &format!("svc-mariadb-{major}"),
                        &format!("{}{major}/latest/", urls::MARIADB_API),
                    )
                    .await?;
                let id = latest
                    .get("releases")
                    .and_then(Value::as_object)
                    .and_then(|o| o.keys().max_by(|a, b| version::compare(a, b)).cloned())
                    .ok_or_else(|| CoreError::Upstream("MariaDB latest release missing".into()))?;
                let release = self
                    .cached_json(
                        &format!("svc-mariadb-rel-{id}"),
                        &format!("{}{id}/", urls::MARIADB_API),
                    )
                    .await?;
                parse_mariadb_release(&release, p)
            }
            ServiceKind::Dns | ServiceKind::PhpFpm(_) => Err(CoreError::InvalidInput(format!(
                "{kind} is not downloaded separately"
            ))),
        }
    }
}

// --------------------------------------------------------------------------------------
// layout + process specs
// --------------------------------------------------------------------------------------

/// Where a service's files live.
#[derive(Debug, Clone)]
pub struct ServiceLayout {
    pub kind: ServiceKind,
    /// Extracted upstream build (`caddy/bin` for caddy).
    pub dist: PathBuf,
    /// Data directory.
    pub data: PathBuf,
    /// Root of the service (`services/<name>`).
    pub root: PathBuf,
}

impl ServiceLayout {
    pub fn new(paths: &Paths, kind: &ServiceKind) -> ServiceLayout {
        let root = match kind {
            ServiceKind::Caddy => paths.caddy(),
            _ => paths.service_dir(&kind.name()),
        };
        let dist = match kind {
            ServiceKind::Caddy => paths.caddy_bin_dir(),
            _ => root.join("dist"),
        };
        ServiceLayout {
            kind: kind.clone(),
            data: match kind {
                ServiceKind::Caddy => paths.caddy_data(),
                _ => root.join("data"),
            },
            dist,
            root,
        }
    }

    /// Main executable.
    pub fn binary(&self, p: Platform) -> PathBuf {
        let exe = p.exe();
        match &self.kind {
            ServiceKind::Caddy => self.dist.join(format!("caddy{exe}")),
            ServiceKind::Mailpit => self.dist.join(format!("mailpit{exe}")),
            ServiceKind::Postgres => self.dist.join("bin").join(format!("postgres{exe}")),
            ServiceKind::Redis => self.dist.join("bin").join(format!("valkey-server{exe}")),
            ServiceKind::Mariadb => {
                let m = self.dist.join("bin").join(format!("mariadbd{exe}"));
                if m.exists() {
                    m
                } else {
                    self.dist.join("bin").join(format!("mysqld{exe}"))
                }
            }
            ServiceKind::Dns | ServiceKind::PhpFpm(_) => PathBuf::new(),
        }
    }

    /// Whether the build is present on disk.
    pub fn installed(&self, p: Platform) -> bool {
        match self.kind {
            ServiceKind::Dns | ServiceKind::PhpFpm(_) => true,
            _ => self.binary(p).exists(),
        }
    }

    /// Unix socket used by a PHP-FPM pool.
    pub fn fpm_socket(&self) -> PathBuf {
        self.root.join("fpm.sock")
    }
}

/// Unix: are we root? (several daemons refuse or need extra flags).
fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Commands that initialise a data directory before first start (mariadb, postgres).
/// Each entry is (program, args).
pub fn init_commands(
    layout: &ServiceLayout,
    p: Platform,
    port: u16,
) -> Vec<(PathBuf, Vec<String>)> {
    let exe = p.exe();
    match layout.kind {
        ServiceKind::Mariadb => {
            if p.is_windows() {
                vec![(
                    layout.dist.join("bin").join("mariadb-install-db.exe"),
                    vec![
                        format!("--datadir={}", s(&layout.data)),
                        format!("--port={port}"),
                    ],
                )]
            } else {
                let script = {
                    let a = layout.dist.join("scripts").join("mariadb-install-db");
                    if a.exists() {
                        a
                    } else {
                        layout.dist.join("scripts").join("mysql_install_db")
                    }
                };
                let mut args = vec![
                    "--no-defaults".to_string(),
                    format!("--basedir={}", s(&layout.dist)),
                    format!("--datadir={}", s(&layout.data)),
                    "--auth-root-authentication-method=normal".into(),
                    "--skip-test-db".into(),
                ];
                if running_as_root() {
                    args.push("--user=root".into());
                }
                vec![(script, args)]
            }
        }
        ServiceKind::Postgres => vec![(
            layout.dist.join("bin").join(format!("initdb{exe}")),
            vec![
                "-D".into(),
                s(&layout.data),
                "-U".into(),
                "postgres".into(),
                "--auth=trust".into(),
                "-E".into(),
                "UTF8".into(),
                "--locale=C".into(),
            ],
        )],
        _ => vec![],
    }
}

/// Build the supervisor spec for a downloaded service (not caddy/php-fpm/dns, which need
/// engine context and are built by [`caddy_spec`] / [`php_pool_spec`]).
pub fn service_spec(
    layout: &ServiceLayout,
    p: Platform,
    port: u16,
    log_file: PathBuf,
) -> Result<ProcessSpec> {
    let bin = layout.binary(p);
    let mut spec = ProcessSpec::new(layout.kind.process_name(), &bin).cwd(&layout.root);
    spec.log_file = Some(log_file);
    spec.stop_timeout = Duration::from_secs(20);
    spec = match layout.kind {
        ServiceKind::Mariadb => {
            let mut args = vec![
                "--no-defaults".to_string(),
                format!("--basedir={}", s(&layout.dist)),
                format!("--datadir={}", s(&layout.data)),
                format!("--port={port}"),
                "--bind-address=127.0.0.1".into(),
                "--console".into(),
            ];
            if !p.is_windows() {
                args.push(format!("--socket={}", s(&layout.root.join("mysql.sock"))));
                args.push(format!(
                    "--pid-file={}",
                    s(&layout.root.join("mariadb.pid"))
                ));
                if running_as_root() {
                    args.push("--user=root".into());
                }
            }
            spec.args(args)
        }
        ServiceKind::Postgres => {
            let mut args = vec![
                "-D".to_string(),
                s(&layout.data),
                "-p".into(),
                port.to_string(),
                "-h".into(),
                "127.0.0.1".into(),
            ];
            if !p.is_windows() {
                args.push("-k".into());
                args.push(s(&layout.root));
            }
            spec.args(args)
        }
        ServiceKind::Redis => spec.args([
            "--port".to_string(),
            port.to_string(),
            "--bind".into(),
            "127.0.0.1".into(),
            "--dir".into(),
            s(&layout.data),
            "--daemonize".into(),
            "no".into(),
        ]),
        ServiceKind::Mailpit => spec.args([
            "--smtp".to_string(),
            format!("127.0.0.1:{port}"),
            "--listen".into(),
            format!("127.0.0.1:{MAILPIT_UI_PORT}"),
            "--database".into(),
            s(&layout.data.join("mailpit.db")),
        ]),
        _ => {
            return Err(CoreError::InvalidInput(format!(
                "{} has no standalone spec",
                layout.kind
            )))
        }
    };
    if p.is_windows() {
        spec.graceful_stop = graceful_stop(layout, p, port);
    }
    Ok(spec)
}

/// Service-native shutdown command used on Windows, where there is no SIGTERM (P1-8).
/// `None` when the service needs none (Mailpit) or its client tool is not in the build;
/// the supervisor then force-kills the tree right away.
pub fn graceful_stop(layout: &ServiceLayout, p: Platform, port: u16) -> Option<GracefulStop> {
    let exe = p.exe();
    let bin = layout.dist.join("bin");
    let first = |names: &[&str]| {
        names
            .iter()
            .map(|n| bin.join(format!("{n}{exe}")))
            .find(|b| b.exists())
    };
    match layout.kind {
        ServiceKind::Mariadb => {
            first(&["mariadb-admin", "mysqladmin"]).map(|program| GracefulStop::Command {
                program,
                args: vec![
                    "--no-defaults".into(),
                    "--protocol=tcp".into(),
                    "--host=127.0.0.1".into(),
                    format!("--port={port}"),
                    "--user=root".into(),
                    "shutdown".into(),
                ],
            })
        }
        ServiceKind::Postgres => first(&["pg_ctl"]).map(|program| GracefulStop::Command {
            program,
            args: vec![
                "stop".into(),
                "-D".into(),
                s(&layout.data),
                "-m".into(),
                "fast".into(),
                // return once the request is sent; the supervisor waits for the exit
                "-W".into(),
            ],
        }),
        ServiceKind::Redis => {
            first(&["valkey-cli", "redis-cli"]).map(|program| GracefulStop::Command {
                program,
                args: vec![
                    "-h".into(),
                    "127.0.0.1".into(),
                    "-p".into(),
                    port.to_string(),
                    "shutdown".into(),
                ],
            })
        }
        _ => None,
    }
}

/// Caddy process spec: `caddy run --config Caddyfile --adapter caddyfile` with its data
/// kept inside the BOZKURT home.
pub fn caddy_spec(paths: &Paths, p: Platform) -> ProcessSpec {
    let layout = ServiceLayout::new(paths, &ServiceKind::Caddy);
    let data = s(&paths.caddy_data());
    let mut spec = ProcessSpec::new("caddy", layout.binary(p))
        .args([
            "run".to_string(),
            "--config".into(),
            s(&paths.caddyfile()),
            "--adapter".into(),
            "caddyfile".into(),
        ])
        .cwd(paths.caddy())
        .env("XDG_DATA_HOME", data.clone())
        .env("XDG_CONFIG_HOME", s(&paths.caddy_data().join("config")));
    if p.is_windows() {
        spec = spec.env("AppData", data);
    }
    spec.log_file = Some(paths.log_file("caddy"));
    spec
}

/// PHP-FPM pool configuration (unix).
pub fn fpm_config(layout: &ServiceLayout, logs: &Path) -> String {
    let name = layout.kind.name();
    format!(
        "; Generated by BOZKURT CODE\n[global]\npid = {pid}\nerror_log = {log}\ndaemonize = no\n\n[www]\nlisten = {sock}\nlisten.mode = 0660\npm = dynamic\npm.max_children = 10\npm.start_servers = 2\npm.min_spare_servers = 1\npm.max_spare_servers = 3\npm.max_requests = 500\ncatch_workers_output = yes\ndecorate_workers_output = no\nclear_env = no\nphp_admin_value[error_log] = {log}\nphp_admin_flag[log_errors] = on\n",
        pid = s(&layout.root.join("php-fpm.pid")),
        log = s(&logs.join(format!("{name}-error.log"))),
        sock = s(&layout.fpm_socket()),
    )
}

/// PHP pool specs: `php-fpm -F -y <conf>` on unix (one process, FPM manages workers);
/// on Windows [`PHP_CGI_INSTANCES`] × `php-cgi -b 127.0.0.1:<port_i>` named by
/// [`php_pool_process_names`] (P1-13). `php_dir` is the installed PHP version dir.
/// Returns (specs, fastcgi upstream(s) — space separated when there are several).
pub fn php_pool_spec(
    paths: &Paths,
    p: Platform,
    mm: &str,
    php_dir: &Path,
) -> Result<(Vec<ProcessSpec>, String)> {
    let kind = ServiceKind::PhpFpm(mm.to_string());
    let layout = ServiceLayout::new(paths, &kind);
    std::fs::create_dir_all(&layout.root)?;
    if p.is_windows() {
        let ports = php_cgi_ports(mm);
        let names = php_pool_process_names(mm, p);
        let specs = names
            .iter()
            .zip(&ports)
            .map(|(name, port)| {
                let mut spec = ProcessSpec::new(name.clone(), php_dir.join("php-cgi.exe"))
                    .args(["-b".to_string(), format!("127.0.0.1:{port}")])
                    .cwd(php_dir)
                    // unlimited requests per process: a clean exit would count as a crash
                    // for the supervisor's restart backoff
                    .env("PHP_FCGI_MAX_REQUESTS", "0");
                spec.log_file = Some(paths.log_file(&kind.name()));
                spec.restart = RestartPolicy::default();
                spec
            })
            .collect();
        let upstream = ports
            .iter()
            .map(|p| format!("127.0.0.1:{p}"))
            .collect::<Vec<_>>()
            .join(" ");
        return Ok((specs, upstream));
    }
    let env = BTreeMap::new();
    let (spec, upstream) = {
        let fpm = php_dir.join("php-fpm");
        if !fpm.exists() {
            return Err(CoreError::Unsupported(format!(
                "this PHP {mm} build has no php-fpm binary ({})",
                fpm.display()
            )));
        }
        let conf = layout.root.join("php-fpm.conf");
        crate::fsutil::atomic_write(&conf, fpm_config(&layout, &paths.logs()).as_bytes())?;
        let _ = std::fs::remove_file(layout.fpm_socket());
        let mut args = vec![
            "-F".to_string(),
            "-y".into(),
            s(&conf),
            "-p".into(),
            s(&layout.root),
        ];
        let ini = php_dir.join("php.ini");
        if ini.exists() {
            args.push("-c".into());
            args.push(s(&ini));
        }
        if running_as_root() {
            args.push("-R".into());
        }
        let spec = ProcessSpec::new(kind.name(), fpm)
            .args(args)
            .cwd(&layout.root);
        (spec, format!("unix/{}", s(&layout.fpm_socket())))
    };
    let mut spec = spec;
    spec.env = env;
    spec.log_file = Some(paths.log_file(&kind.name()));
    spec.restart = RestartPolicy::default();
    Ok((vec![spec], upstream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogs::tests::fixture;

    const LINUX_ARM: Platform = Platform::new(Os::Linux, Arch::Arm64);
    const LINUX_X64: Platform = Platform::new(Os::Linux, Arch::X64);
    const MAC_ARM: Platform = Platform::new(Os::MacOs, Arch::Arm64);
    const WIN: Platform = Platform::new(Os::Windows, Arch::X64);

    #[test]
    fn names() {
        for k in ServiceKind::FIXED {
            assert_eq!(ServiceKind::parse(&k.name()).unwrap(), k);
        }
        assert_eq!(
            ServiceKind::parse("php-fpm-8.3").unwrap(),
            ServiceKind::PhpFpm("8.3".into())
        );
        assert_eq!(ServiceKind::parse("mysql").unwrap(), ServiceKind::Mariadb);
        assert!(ServiceKind::parse("php-fpm-8").is_err());
        assert!(ServiceKind::parse("nginx").is_err());
        assert_eq!(php_cgi_port("8.3"), 9083);
        assert_eq!(ServiceKind::Mariadb.default_port(), Some(3306));
    }

    #[test]
    fn caddy_assets() {
        let r = fixture("caddy_release.json");
        let v = r["tag_name"]
            .as_str()
            .unwrap()
            .trim_start_matches('v')
            .to_string();
        let l = parse_caddy(&r, LINUX_ARM).unwrap();
        assert_eq!(l.version, v);
        assert_eq!(l.asset.filename, format!("caddy_{v}_linux_arm64.tar.gz"));
        assert!(matches!(l.asset.checksum, Checksum::Sha256 { .. }));
        assert!(parse_caddy(&r, MAC_ARM)
            .unwrap()
            .asset
            .filename
            .ends_with("_mac_arm64.tar.gz"));
        assert!(parse_caddy(&r, WIN)
            .unwrap()
            .asset
            .filename
            .ends_with("_windows_amd64.zip"));
    }

    #[test]
    fn mailpit_assets() {
        let r = fixture("mailpit_release.json");
        assert_eq!(
            parse_mailpit(&r, LINUX_X64).unwrap().asset.filename,
            "mailpit-linux-amd64.tar.gz"
        );
        assert_eq!(
            parse_mailpit(&r, MAC_ARM).unwrap().asset.filename,
            "mailpit-darwin-arm64.tar.gz"
        );
        assert_eq!(
            parse_mailpit(&r, WIN).unwrap().asset.filename,
            "mailpit-windows-amd64.zip"
        );
    }

    #[test]
    fn postgres_assets() {
        let r = fixture("postgres_releases.json");
        let newest = parse_postgres(&r, LINUX_ARM, None).unwrap();
        assert!(newest.version.starts_with("18."));
        assert_eq!(
            newest.asset.filename,
            format!(
                "postgresql-{}-aarch64-unknown-linux-gnu.tar.gz",
                newest.version
            )
        );
        let v17 = parse_postgres(&r, MAC_ARM, Some("17")).unwrap();
        assert!(v17.version.starts_with("17."));
        assert!(v17.asset.filename.ends_with("aarch64-apple-darwin.tar.gz"));
        let w = parse_postgres(&r, WIN, Some("16")).unwrap();
        assert!(w.asset.filename.ends_with("x86_64-pc-windows-msvc.tar.gz"));
        assert!(parse_postgres(&r, WIN, Some("9")).is_err());
    }

    #[test]
    fn mariadb_assets() {
        let majors = fixture("mariadb_majors.json");
        let lts = mariadb_major(&majors, None).unwrap();
        assert!(version::compare(&lts, "11.4") == std::cmp::Ordering::Greater);
        assert_eq!(
            mariadb_major(&majors, Some("11.8")).as_deref(),
            Some("11.8")
        );
        assert_eq!(
            mariadb_major(&majors, Some("11.8.3")).as_deref(),
            Some("11.8")
        );
        let rel = fixture("mariadb_release.json");
        let l = parse_mariadb_release(&rel, LINUX_X64).unwrap();
        assert_eq!(l.version, "11.8.9");
        assert_eq!(
            l.asset.filename,
            "mariadb-11.8.9-linux-systemd-x86_64.tar.gz"
        );
        assert!(l.asset.url.starts_with("https://"));
        assert!(matches!(l.asset.checksum, Checksum::Sha256 { .. }));
        let w = parse_mariadb_release(&rel, WIN).unwrap();
        assert_eq!(w.asset.filename, "mariadb-11.8.9-winx64.zip");
        assert!(matches!(
            parse_mariadb_release(&rel, MAC_ARM),
            Err(CoreError::Unsupported(_))
        ));
        assert!(matches!(
            parse_mariadb_release(&rel, LINUX_ARM),
            Err(CoreError::Unsupported(_))
        ));
    }

    #[test]
    fn valkey_assets() {
        let r = fixture("valkey_releases.json");
        let v = parse_valkey(&r, LINUX_ARM, None).unwrap();
        assert!(!v.version.contains("rc"));
        assert_eq!(
            v.asset.url,
            format!(
                "https://download.valkey.io/releases/valkey-{}-jammy-arm64.tar.gz",
                v.version
            )
        );
        let v8 = parse_valkey(&r, LINUX_X64, Some("8.1")).unwrap();
        assert!(v8.version.starts_with("8.1."));
        assert!(matches!(
            parse_valkey(&r, MAC_ARM, None),
            Err(CoreError::Unsupported(_))
        ));
        assert!(matches!(
            parse_valkey(&r, WIN, None),
            Err(CoreError::Unsupported(_))
        ));
    }

    #[test]
    fn specs() {
        let paths = Paths::new("/h");
        let p = LINUX_X64;
        let l = ServiceLayout::new(&paths, &ServiceKind::Postgres);
        let spec = service_spec(&l, p, 5433, paths.log_file("postgres")).unwrap();
        assert_eq!(
            spec.program,
            PathBuf::from("/h/services/postgres/dist/bin/postgres")
        );
        assert!(spec.args.windows(2).any(|w| w == ["-p", "5433"]));
        let init = init_commands(&l, p, 5433);
        assert_eq!(
            init[0].0,
            PathBuf::from("/h/services/postgres/dist/bin/initdb")
        );
        let m = ServiceLayout::new(&paths, &ServiceKind::Mailpit);
        let ms = service_spec(&m, p, 1025, paths.log_file("mailpit")).unwrap();
        assert!(ms.args.contains(&"127.0.0.1:1025".to_string()));
        let c = caddy_spec(&paths, p);
        assert_eq!(c.program, PathBuf::from("/h/caddy/bin/caddy"));
        assert_eq!(c.args[0], "run");
        assert_eq!(c.env["XDG_DATA_HOME"], "/h/caddy/data");
        let r = ServiceLayout::new(&paths, &ServiceKind::Redis);
        assert_eq!(
            r.binary(p),
            PathBuf::from("/h/services/redis/dist/bin/valkey-server")
        );
        let fpm = ServiceLayout::new(&paths, &ServiceKind::PhpFpm("8.3".into()));
        let conf = fpm_config(&fpm, Path::new("/h/logs"));
        assert!(conf.contains("listen = /h/services/php-fpm-8.3/fpm.sock"));
        assert!(conf.contains("daemonize = no"));
    }

    #[test]
    fn windows_graceful_stop_hooks() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        let m = ServiceLayout::new(&paths, &ServiceKind::Mariadb);
        // no client tool in the build → no hook (force kill)
        assert_eq!(graceful_stop(&m, WIN, 3306), None);
        std::fs::create_dir_all(m.dist.join("bin")).unwrap();
        std::fs::write(m.dist.join("bin/mysqladmin.exe"), "").unwrap();
        let spec = service_spec(&m, WIN, 3307, paths.log_file("mariadb")).unwrap();
        match spec.graceful_stop {
            Some(GracefulStop::Command { program, args }) => {
                assert!(program.ends_with("mysqladmin.exe"));
                assert!(args.contains(&"--port=3307".to_string()));
                assert_eq!(args.last().map(String::as_str), Some("shutdown"));
            }
            other => panic!("{other:?}"),
        }
        // unix keeps SIGTERM
        assert_eq!(
            service_spec(&m, LINUX_X64, 3307, paths.log_file("mariadb"))
                .unwrap()
                .graceful_stop,
            None
        );
        let pg = ServiceLayout::new(&paths, &ServiceKind::Postgres);
        std::fs::create_dir_all(pg.dist.join("bin")).unwrap();
        std::fs::write(pg.dist.join("bin/pg_ctl.exe"), "").unwrap();
        assert!(matches!(
            graceful_stop(&pg, WIN, 5432),
            Some(GracefulStop::Command { args, .. }) if args.starts_with(&["stop".to_string()])
        ));
        let mp = ServiceLayout::new(&paths, &ServiceKind::Mailpit);
        assert_eq!(graceful_stop(&mp, WIN, 1025), None);
    }

    #[test]
    fn php_pool_requires_fpm_binary() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        let php = t.path().join("runtimes/php/8.3.1");
        std::fs::create_dir_all(&php).unwrap();
        assert!(php_pool_spec(&paths, LINUX_X64, "8.3", &php).is_err());
        std::fs::write(php.join("php-fpm"), "").unwrap();
        let (specs, up) = php_pool_spec(&paths, LINUX_X64, "8.3", &php).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "php-fpm-8.3");
        assert!(up.starts_with("unix/"));
        assert!(t.path().join("services/php-fpm-8.3/php-fpm.conf").exists());
        let (wspecs, wup) = php_pool_spec(&paths, WIN, "8.2", &php).unwrap();
        assert_eq!(wspecs.len(), PHP_CGI_INSTANCES);
        assert_eq!(
            wup,
            "127.0.0.1:9082 127.0.0.1:21417 127.0.0.1:21418 127.0.0.1:21419"
        );
        assert!(wspecs.iter().all(|s| s.program.ends_with("php-cgi.exe")));
        let names: Vec<_> = wspecs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "php-fpm-8.2",
                "php-fpm-8.2#1",
                "php-fpm-8.2#2",
                "php-fpm-8.2#3"
            ]
        );
        assert_eq!(names, php_pool_process_names("8.2", WIN));
        assert_eq!(wspecs[1].args, ["-b", "127.0.0.1:21417"]);
    }

    #[test]
    fn php_cgi_ports_never_collide() {
        let mut seen = std::collections::HashSet::new();
        for major in 5..=9 {
            for minor in 0..=12 {
                for p in php_cgi_ports(&format!("{major}.{minor}")) {
                    assert!(seen.insert(p), "port {p} reused ({major}.{minor})");
                }
            }
        }
        assert_eq!(php_cgi_port("8.3"), 9083);
        assert_ne!(php_cgi_port("8.10"), php_cgi_port("9.0"));
    }
}
