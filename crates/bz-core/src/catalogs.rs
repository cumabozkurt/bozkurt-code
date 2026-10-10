//! Live upstream version catalogs for every runtime, with a 6 h on-disk cache.
//!
//! | runtime | source | asset |
//! |---|---|---|
//! | PHP (mac/linux) | `https://dl.static-php.dev/static-php-cli/bulk/?format=json` | `php-<v>-cli-<os>-<arch>.tar.gz` + `php-<v>-fpm-…` |
//! | PHP (Windows) | `https://windows.php.net/downloads/releases/releases.json` | `nts-vs1x-x64` zip (sha256 in index) |
//! | Node | `https://nodejs.org/dist/index.json` (+ `SHASUMS256.txt`, Release `schedule.json`) | `node-v<v>-<os>-<arch>.{tar.gz,tar.xz,zip}` |
//! | Bun | GitHub releases `oven-sh/bun` | `bun-<os>-<arch>.zip` (asset digest / `SHASUMS256.txt`) |
//! | Python | GitHub latest release `astral-sh/python-build-standalone` | `cpython-<v>+<tag>-<triple>-install_only.tar.gz` |
//! | Go | `https://go.dev/dl/?mode=json&include=all` | `go<v>.<os>-<arch>.{tar.gz,zip}` (sha256 in index) |
//!
//! The parse functions are pure (JSON in, releases out) and unit-tested against fixtures
//! captured from the real endpoints in `tests/fixtures/`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::download::Checksum;
use crate::error::{CoreError, Result};
use crate::models::{Channel, RuntimeKind};
use crate::platform::{Arch, Os, Platform};
use crate::version;

/// Upstream endpoints.
pub mod urls {
    pub const PHP_STATIC_INDEX: &str = "https://dl.static-php.dev/static-php-cli/bulk/?format=json";
    pub const PHP_STATIC_BASE: &str = "https://dl.static-php.dev/static-php-cli/bulk/";
    pub const PHP_WINDOWS_INDEX: &str = "https://windows.php.net/downloads/releases/releases.json";
    pub const PHP_WINDOWS_BASE: &str = "https://windows.php.net/downloads/releases/";
    pub const NODE_INDEX: &str = "https://nodejs.org/dist/index.json";
    pub const NODE_DIST: &str = "https://nodejs.org/dist/";
    pub const NODE_SCHEDULE: &str =
        "https://raw.githubusercontent.com/nodejs/Release/main/schedule.json";
    pub const BUN_RELEASES: &str = "https://api.github.com/repos/oven-sh/bun/releases?per_page=30";
    pub const PYTHON_LATEST: &str =
        "https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest";
    pub const GO_INDEX: &str = "https://go.dev/dl/?mode=json&include=all";
    pub const GO_DL: &str = "https://go.dev/dl/";
    pub const COMPOSER_PHAR: &str = "https://getcomposer.org/download/latest-stable/composer.phar";
    pub const COMPOSER_SHA: &str =
        "https://getcomposer.org/download/latest-stable/composer.phar.sha256";
}

/// Cache lifetime for upstream indexes.
pub const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);

/// A downloadable file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub url: String,
    pub filename: String,
    pub size: Option<u64>,
    pub checksum: Checksum,
}

/// One upstream release usable on the current platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub channel: Channel,
    pub released: Option<String>,
    /// All files to extract (in order) into the version dir. PHP on mac/linux has two:
    /// the CLI binary and the FPM binary.
    pub assets: Vec<Asset>,
}

impl Release {
    /// Total download size when every asset's size is known.
    pub fn size(&self) -> Option<u64> {
        let mut total = 0;
        for a in &self.assets {
            total += a.size?;
        }
        Some(total)
    }
}

/// Find the first `YYYY-MM-DD` in a string.
pub fn extract_date(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() < 10 {
        return None;
    }
    (0..=b.len() - 10).find_map(|i| {
        let w = &b[i..i + 10];
        let ok = w.iter().enumerate().all(|(j, c)| match j {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        });
        ok.then(|| String::from_utf8_lossy(w).into_owned())
    })
}

/// `25.4M`, `25.02MB`, `978B`, `1.2G` → bytes.
pub fn parse_human_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let idx = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (n, unit) = s.split_at(idx);
    let n: f64 = n.parse().ok()?;
    let mult = match unit
        .trim()
        .trim_end_matches(['B', 'b'])
        .to_ascii_uppercase()
        .as_str()
    {
        "" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

fn eol_channel(eol: Option<&str>, today: &str) -> Channel {
    match eol {
        Some(d) if today > d => Channel::Eol,
        _ => Channel::Stable,
    }
}

/// End-of-life dates of PHP minors (php.net/supported-versions).
fn php_eol(mm: &str) -> Option<&'static str> {
    Some(match mm {
        "8.0" => "2023-11-26",
        "8.1" => "2025-12-31",
        "8.2" => "2026-12-31",
        "8.3" => "2027-12-31",
        "8.4" => "2028-12-31",
        "8.5" => "2029-12-31",
        m if version::compare(m, "8.0") == std::cmp::Ordering::Less => "2022-11-28",
        _ => return None,
    })
}

/// End-of-life dates of CPython minors (devguide).
fn python_eol(mm: &str) -> Option<&'static str> {
    Some(match mm {
        "3.8" => "2024-10-07",
        "3.9" => "2025-10-31",
        "3.10" => "2026-10-31",
        "3.11" => "2027-10-31",
        "3.12" => "2028-10-31",
        "3.13" => "2029-10-31",
        "3.14" => "2030-10-31",
        "3.15" => "2031-10-31",
        _ => return None,
    })
}

// --------------------------------------------------------------------------------------
// PHP
// --------------------------------------------------------------------------------------

/// Parse the static-php.dev bulk directory listing (macOS / Linux).
pub fn parse_php_static(index: &Value, p: Platform, today: &str) -> Vec<Release> {
    let os = match p.os {
        Os::MacOs => "macos",
        Os::Linux => "linux",
        Os::Windows => return vec![],
    };
    let arch = p.arch_str();
    type Entry = (String, Option<u64>, Option<String>);
    let mut cli: BTreeMap<String, Entry> = BTreeMap::new();
    let mut fpm: BTreeMap<String, Entry> = BTreeMap::new();
    for item in index.as_array().into_iter().flatten() {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(rest) = name.strip_prefix("php-") else {
            continue;
        };
        let size = item
            .get("size")
            .and_then(Value::as_str)
            .and_then(parse_human_size);
        let date = item
            .get("last_modified")
            .and_then(Value::as_str)
            .and_then(extract_date);
        for (sapi, map) in [("cli", &mut cli), ("fpm", &mut fpm)] {
            let suffix = format!("-{sapi}-{os}-{arch}.tar.gz");
            if let Some(v) = rest.strip_suffix(&suffix) {
                if version::parse(v).len() == 3 && v.chars().all(|c| c.is_ascii_digit() || c == '.')
                {
                    map.insert(v.to_string(), (name.to_string(), size, date.clone()));
                }
            }
        }
    }
    let mut out: Vec<Release> = cli
        .into_iter()
        .map(|(v, (name, size, date))| {
            let mut assets = vec![Asset {
                url: format!("{}{}", urls::PHP_STATIC_BASE, name),
                filename: name,
                size,
                // static-php.dev publishes no checksums; served over HTTPS only.
                checksum: Checksum::None,
            }];
            if let Some((fname, fsize, _)) = fpm.get(&v) {
                assets.push(Asset {
                    url: format!("{}{}", urls::PHP_STATIC_BASE, fname),
                    filename: fname.clone(),
                    size: *fsize,
                    checksum: Checksum::None,
                });
            }
            Release {
                channel: eol_channel(php_eol(&version::major_minor(&v)), today),
                version: v,
                released: date,
                assets,
            }
        })
        .collect();
    out.sort_by(|a, b| version::compare(&b.version, &a.version));
    out
}

/// Parse windows.php.net `releases.json` (latest patch of each supported branch).
/// Picks the non-thread-safe x64 build (php-cgi FastCGI).
pub fn parse_php_windows(index: &Value, p: Platform, today: &str) -> Vec<Release> {
    if p.os != Os::Windows || p.arch != Arch::X64 {
        return vec![];
    }
    let mut out = vec![];
    for rel in index.as_object().into_iter().flat_map(|o| o.values()) {
        let Some(v) = rel.get("version").and_then(Value::as_str) else {
            continue;
        };
        let Some(obj) = rel.as_object() else { continue };
        let Some((_, build)) = obj
            .iter()
            .find(|(k, _)| k.starts_with("nts-") && k.ends_with("-x64"))
        else {
            continue;
        };
        let Some(zip) = build.get("zip") else {
            continue;
        };
        let Some(path) = zip.get("path").and_then(Value::as_str) else {
            continue;
        };
        let checksum = zip
            .get("sha256")
            .and_then(Value::as_str)
            .map(Checksum::sha256)
            .unwrap_or(Checksum::None);
        out.push(Release {
            version: v.to_string(),
            channel: eol_channel(php_eol(&version::major_minor(v)), today),
            released: build
                .get("mtime")
                .and_then(Value::as_str)
                .and_then(extract_date),
            assets: vec![Asset {
                url: format!("{}{}", urls::PHP_WINDOWS_BASE, path),
                filename: path.to_string(),
                size: zip
                    .get("size")
                    .and_then(Value::as_str)
                    .and_then(parse_human_size),
                checksum,
            }],
        });
    }
    out.sort_by(|a, b| version::compare(&b.version, &a.version));
    out
}

// --------------------------------------------------------------------------------------
// Node
// --------------------------------------------------------------------------------------

/// Parse `nodejs.org/dist/index.json`; `schedule` is the nodejs/Release `schedule.json`
/// used to classify lts / current / eol (falls back to the index's `lts` flag).
pub fn parse_node(
    index: &Value,
    schedule: Option<&Value>,
    p: Platform,
    today: &str,
) -> Vec<Release> {
    let (key, os_part, ext) = match (p.os, p.arch) {
        (Os::MacOs, Arch::Arm64) => ("osx-arm64-tar", "darwin-arm64", "tar.gz"),
        (Os::MacOs, Arch::X64) => ("osx-x64-tar", "darwin-x64", "tar.gz"),
        (Os::Linux, Arch::Arm64) => ("linux-arm64", "linux-arm64", "tar.xz"),
        (Os::Linux, Arch::X64) => ("linux-x64", "linux-x64", "tar.xz"),
        (Os::Windows, Arch::Arm64) => ("win-arm64-zip", "win-arm64", "zip"),
        (Os::Windows, Arch::X64) => ("win-x64-zip", "win-x64", "zip"),
    };
    let mut out = vec![];
    for item in index.as_array().into_iter().flatten() {
        let Some(tag) = item.get("version").and_then(Value::as_str) else {
            continue;
        };
        let has = item
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|f| f.iter().any(|x| x.as_str() == Some(key)));
        if !has {
            continue;
        }
        let v = tag.trim_start_matches('v').to_string();
        let major = version::parse(&v).first().copied().unwrap_or(0);
        let lts_flag = item.get("lts").is_some_and(|l| l.is_string());
        let channel = node_channel(major, lts_flag, schedule, today);
        let filename = format!("node-{tag}-{os_part}.{ext}");
        out.push(Release {
            version: v,
            channel,
            released: item.get("date").and_then(Value::as_str).map(str::to_string),
            assets: vec![Asset {
                url: format!("{}{tag}/{filename}", urls::NODE_DIST),
                checksum: Checksum::SumsFile {
                    url: format!("{}{tag}/SHASUMS256.txt", urls::NODE_DIST),
                    filename: filename.clone(),
                },
                filename,
                size: None,
            }],
        });
    }
    out.sort_by(|a, b| version::compare(&b.version, &a.version));
    out
}

fn node_channel(major: u64, lts_flag: bool, schedule: Option<&Value>, today: &str) -> Channel {
    if major == 0 {
        return Channel::Eol;
    }
    let key = format!("v{major}");
    if let Some(s) = schedule.and_then(|s| s.get(&key)) {
        let end = s.get("end").and_then(Value::as_str).unwrap_or("9999-12-31");
        if today > end {
            return Channel::Eol;
        }
        if let Some(lts) = s.get("lts").and_then(Value::as_str) {
            if today >= lts {
                return Channel::Lts;
            }
        }
        return Channel::Current;
    }
    if lts_flag {
        Channel::Lts
    } else {
        Channel::Current
    }
}

// --------------------------------------------------------------------------------------
// GitHub based (Bun, Python)
// --------------------------------------------------------------------------------------

/// Find an asset by exact name in a GitHub release object.
pub fn gh_asset<'a>(release: &'a Value, name: &str) -> Option<&'a Value> {
    release
        .get("assets")?
        .as_array()?
        .iter()
        .find(|a| a.get("name").and_then(Value::as_str) == Some(name))
}

fn gh_checksum(release: &Value, asset: &Value, sums_name: &str) -> Checksum {
    let name = asset.get("name").and_then(Value::as_str).unwrap_or("");
    if let Some(c) = Checksum::from_github_digest(asset.get("digest").and_then(Value::as_str)) {
        return c;
    }
    if let Some(s) = gh_asset(release, sums_name)
        .and_then(|s| s.get("browser_download_url"))
        .and_then(Value::as_str)
    {
        return Checksum::SumsFile {
            url: s.to_string(),
            filename: name.to_string(),
        };
    }
    Checksum::None
}

/// Convert a GitHub release asset to an [`Asset`]; checksum from the asset `digest`,
/// else from the release's `sums_name` file.
pub fn gh_to_asset(release: &Value, asset: &Value, sums_name: &str) -> Option<Asset> {
    Some(Asset {
        url: asset.get("browser_download_url")?.as_str()?.to_string(),
        filename: asset.get("name")?.as_str()?.to_string(),
        size: asset.get("size").and_then(Value::as_u64),
        checksum: gh_checksum(release, asset, sums_name),
    })
}

/// Parse the GitHub releases list of `oven-sh/bun`.
pub fn parse_bun(releases: &Value, p: Platform) -> Vec<Release> {
    let os = match p.os {
        Os::MacOs => "darwin",
        Os::Linux => "linux",
        Os::Windows => "windows",
    };
    let arch = match p.arch {
        Arch::X64 => "x64",
        Arch::Arm64 => "aarch64",
    };
    let name = format!("bun-{os}-{arch}.zip");
    let mut out = vec![];
    let mut first = true;
    for r in releases.as_array().into_iter().flatten() {
        if r.get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || r.get("draft").and_then(Value::as_bool).unwrap_or(false)
        {
            continue;
        }
        let Some(tag) = r.get("tag_name").and_then(Value::as_str) else {
            continue;
        };
        let Some(v) = tag.strip_prefix("bun-v") else {
            continue;
        };
        let Some(asset) = gh_asset(r, &name).and_then(|a| gh_to_asset(r, a, "SHASUMS256.txt"))
        else {
            continue;
        };
        out.push(Release {
            version: v.to_string(),
            channel: if first {
                Channel::Current
            } else {
                Channel::Stable
            },
            released: r
                .get("published_at")
                .and_then(Value::as_str)
                .and_then(extract_date),
            assets: vec![asset],
        });
        first = false;
    }
    out
}

/// Parse one GitHub release of `astral-sh/python-build-standalone` (the latest release
/// carries the newest patch of every supported minor). Uses `install_only` builds.
pub fn parse_python(release: &Value, p: Platform, today: &str) -> Vec<Release> {
    let suffix = format!("-{}-install_only.tar.gz", p.triple());
    let mut out: Vec<Release> = vec![];
    for a in release
        .get("assets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = a.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(rest) = name.strip_prefix("cpython-") else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(&suffix) else {
            continue;
        };
        let v = rest.split('+').next().unwrap_or(rest);
        if !v.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue; // pre-releases like 3.15.0rc3
        }
        let Some(asset) = gh_to_asset(release, a, "SHA256SUMS") else {
            continue;
        };
        out.push(Release {
            version: v.to_string(),
            channel: eol_channel(python_eol(&version::major_minor(v)), today),
            released: release
                .get("published_at")
                .and_then(Value::as_str)
                .and_then(extract_date),
            assets: vec![asset],
        });
    }
    out.sort_by(|a, b| version::compare(&b.version, &a.version));
    out.dedup_by(|a, b| a.version == b.version);
    out
}

// --------------------------------------------------------------------------------------
// Go
// --------------------------------------------------------------------------------------

/// Parse `go.dev/dl/?mode=json&include=all` (stable releases only). The two newest
/// minors are `stable`, older ones `eol` (Go's support policy).
pub fn parse_go(index: &Value, p: Platform) -> Vec<Release> {
    let os = match p.os {
        Os::MacOs => "darwin",
        Os::Linux => "linux",
        Os::Windows => "windows",
    };
    let mut out = vec![];
    for r in index.as_array().into_iter().flatten() {
        if !r.get("stable").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let Some(tag) = r.get("version").and_then(Value::as_str) else {
            continue;
        };
        let file = r.get("files").and_then(Value::as_array).and_then(|fs| {
            fs.iter().find(|f| {
                f.get("kind").and_then(Value::as_str) == Some("archive")
                    && f.get("os").and_then(Value::as_str) == Some(os)
                    && f.get("arch").and_then(Value::as_str) == Some(p.goarch())
            })
        });
        let Some(f) = file else { continue };
        let Some(filename) = f.get("filename").and_then(Value::as_str) else {
            continue;
        };
        out.push(Release {
            version: tag.trim_start_matches("go").to_string(),
            channel: Channel::Stable,
            released: None,
            assets: vec![Asset {
                url: format!("{}{}", urls::GO_DL, filename),
                filename: filename.to_string(),
                size: f.get("size").and_then(Value::as_u64),
                checksum: f
                    .get("sha256")
                    .and_then(Value::as_str)
                    .map(Checksum::sha256)
                    .unwrap_or(Checksum::None),
            }],
        });
    }
    out.sort_by(|a, b| version::compare(&b.version, &a.version));
    let mut minors: Vec<String> = out
        .iter()
        .map(|r| version::major_minor(&r.version))
        .collect();
    minors.dedup();
    for r in &mut out {
        let mm = version::major_minor(&r.version);
        if minors.iter().position(|m| *m == mm).unwrap_or(0) >= 2 {
            r.channel = Channel::Eol;
        }
    }
    out
}

/// Reduce a full release list to what the UI shows: the newest patch of each
/// major (Node) or major.minor (others); every Bun release.
pub fn latest_per_line(kind: RuntimeKind, releases: &[Release]) -> Vec<Release> {
    let mut seen = std::collections::HashSet::new();
    releases
        .iter()
        .filter(|r| {
            let key = match kind {
                RuntimeKind::Node => version::parse(&r.version)
                    .first()
                    .copied()
                    .unwrap_or(0)
                    .to_string(),
                RuntimeKind::Bun => r.version.clone(),
                _ => version::major_minor(&r.version),
            };
            seen.insert(key)
        })
        .cloned()
        .collect()
}

/// Pick a release from a list: `latest` → newest, `lts` → newest LTS (or newest
/// non-EOL), otherwise exact match or newest matching prefix.
pub fn resolve_in(releases: &[Release], want: &str) -> Option<Release> {
    let w = want.trim().trim_start_matches('v').to_ascii_lowercase();
    match w.as_str() {
        "" | "latest" | "current" => releases.first().cloned(),
        "lts" | "lts/*" | "stable" => releases
            .iter()
            .find(|r| r.channel == Channel::Lts)
            .or_else(|| releases.iter().find(|r| r.channel != Channel::Eol))
            .cloned(),
        _ => {
            if let Some(r) = releases.iter().find(|r| r.version == w) {
                return Some(r.clone());
            }
            releases
                .iter()
                .find(|r| version::matches_prefix(&r.version, &w))
                .cloned()
        }
    }
}

// --------------------------------------------------------------------------------------
// Fetching + cache
// --------------------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile<T> {
    fetched_at: i64,
    platform: String,
    data: T,
}

/// Small JSON-on-disk cache with TTL shared by the runtime and service catalogs.
#[derive(Debug, Clone)]
pub struct DiskCache {
    dir: PathBuf,
    ttl: Duration,
}

impl DiskCache {
    pub fn new(dir: PathBuf, ttl: Duration) -> DiskCache {
        DiskCache { dir, ttl }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// Returns `(data, fresh)` if an entry exists.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<(T, bool)> {
        let bytes = std::fs::read(self.path(key)).ok()?;
        let c: CacheFile<T> = serde_json::from_slice(&bytes).ok()?;
        let age = chrono::Utc::now().timestamp() - c.fetched_at;
        Some((c.data, age >= 0 && age < self.ttl.as_secs() as i64))
    }

    pub fn put<T: Serialize>(&self, key: &str, platform: &str, data: &T) {
        let c = CacheFile {
            fetched_at: chrono::Utc::now().timestamp(),
            platform: platform.to_string(),
            data,
        };
        if let Ok(bytes) = serde_json::to_vec(&c) {
            let _ = crate::fsutil::atomic_write(&self.path(key), &bytes);
        }
    }

    /// Cached value unless stale/`refresh`; otherwise `fetch`; on fetch failure a stale
    /// value is returned (with a warning).
    pub async fn get_or_fetch<T, F, Fut>(
        &self,
        key: &str,
        platform: &str,
        refresh: bool,
        fetch: F,
    ) -> Result<T>
    where
        T: Serialize + serde::de::DeserializeOwned + Clone,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let cached = self.get::<T>(key);
        if let Some((d, true)) = &cached {
            if !refresh {
                return Ok(d.clone());
            }
        }
        match fetch().await {
            Ok(d) => {
                self.put(key, platform, &d);
                Ok(d)
            }
            Err(e) => match cached {
                Some((d, _)) => {
                    tracing::warn!("refresh of {key} failed ({e}); using stale cache");
                    Ok(d)
                }
                None => Err(e),
            },
        }
    }
}

/// GET a JSON document (GitHub API gets the right Accept header and `GITHUB_TOKEN`).
pub async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value> {
    let mut req = client.get(url);
    if url.starts_with("https://api.github.com/") {
        req = req.header("Accept", "application/vnd.github+json");
        if let Ok(t) = std::env::var("GITHUB_TOKEN") {
            if !t.is_empty() {
                req = req.bearer_auth(t);
            }
        }
    }
    let resp = req
        .send()
        .await?
        .error_for_status()
        .map_err(|e| CoreError::Upstream(format!("GET {url}: {e}")))?;
    Ok(resp.json().await?)
}

/// Fetches and caches runtime catalogs for one platform.
#[derive(Debug, Clone)]
pub struct Catalog {
    cache: DiskCache,
    client: reqwest::Client,
    platform: Platform,
}

impl Catalog {
    pub fn new(cache_dir: PathBuf, client: reqwest::Client, platform: Platform) -> Catalog {
        Catalog {
            cache: DiskCache::new(cache_dir, CACHE_TTL),
            client,
            platform,
        }
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn cache(&self) -> &DiskCache {
        &self.cache
    }

    fn key(&self, kind: RuntimeKind) -> String {
        format!("catalog-{}-{}", kind, self.platform)
    }

    /// All releases of `kind` for this platform, newest first. Uses the cache unless it is
    /// older than 6 h or `refresh` is set; on network failure a stale cache is returned.
    pub async fn releases(&self, kind: RuntimeKind, refresh: bool) -> Result<Vec<Release>> {
        let key = self.key(kind);
        self.cache
            .get_or_fetch(&key, &self.platform.to_string(), refresh, || {
                self.fetch(kind)
            })
            .await
    }

    async fn fetch(&self, kind: RuntimeKind) -> Result<Vec<Release>> {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let p = self.platform;
        let c = &self.client;
        let r = match kind {
            RuntimeKind::Php if p.is_windows() => {
                parse_php_windows(&get_json(c, urls::PHP_WINDOWS_INDEX).await?, p, &today)
            }
            RuntimeKind::Php => {
                parse_php_static(&get_json(c, urls::PHP_STATIC_INDEX).await?, p, &today)
            }
            RuntimeKind::Node => {
                let index = get_json(c, urls::NODE_INDEX).await?;
                let schedule = get_json(c, urls::NODE_SCHEDULE).await.ok();
                parse_node(&index, schedule.as_ref(), p, &today)
            }
            RuntimeKind::Bun => parse_bun(&get_json(c, urls::BUN_RELEASES).await?, p),
            RuntimeKind::Python => {
                parse_python(&get_json(c, urls::PYTHON_LATEST).await?, p, &today)
            }
            RuntimeKind::Go => parse_go(&get_json(c, urls::GO_INDEX).await?, p),
        };
        if r.is_empty() {
            return Err(CoreError::Unsupported(format!(
                "no {kind} builds published for {p}"
            )));
        }
        Ok(r)
    }

    /// Resolve `want` (exact, prefix like `22`/`8.3`, or `latest`/`lts`) to a release.
    pub async fn resolve(&self, kind: RuntimeKind, want: &str) -> Result<Release> {
        let releases = self.releases(kind, false).await?;
        resolve_in(&releases, want).ok_or_else(|| {
            CoreError::NotFound(format!(
                "{kind} {want} is not available for {}",
                self.platform
            ))
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn fixture(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }

    const TODAY: &str = "2026-10-09";
    const LINUX_ARM: Platform = Platform::new(Os::Linux, Arch::Arm64);
    const LINUX_X64: Platform = Platform::new(Os::Linux, Arch::X64);
    const MAC_ARM: Platform = Platform::new(Os::MacOs, Arch::Arm64);
    const MAC_X64: Platform = Platform::new(Os::MacOs, Arch::X64);
    const WIN: Platform = Platform::new(Os::Windows, Arch::X64);

    #[test]
    fn helpers() {
        assert_eq!(parse_human_size("25.4M"), Some((25.4 * 1048576.0) as u64));
        assert_eq!(parse_human_size("978B"), Some(978));
        assert_eq!(
            parse_human_size("25.02MB"),
            Some((25.02 * 1048576.0) as u64)
        );
        assert_eq!(parse_human_size("-"), None);
        assert_eq!(
            extract_date("2025-07-31 03:18:48").as_deref(),
            Some("2025-07-31")
        );
        assert_eq!(
            extract_date("Thu 2024-07-11 12:09 AM (UTC+03:00)").as_deref(),
            Some("2024-07-11")
        );
        assert_eq!(
            extract_date("2026-09-05T05:55:00Z").as_deref(),
            Some("2026-09-05")
        );
        assert_eq!(extract_date("2026-09-05").as_deref(), Some("2026-09-05"));
        assert_eq!(extract_date("nope"), None);
    }

    #[test]
    fn php_static() {
        let idx = fixture("php_static_bulk.json");
        let r = parse_php_static(&idx, LINUX_ARM, TODAY);
        let vs: Vec<_> = r.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(vs, vec!["8.4.23", "8.4.22", "8.3.26", "8.3.25", "8.0.30"]);
        let top = &r[0];
        assert_eq!(top.assets.len(), 2);
        assert_eq!(
            top.assets[0].url,
            "https://dl.static-php.dev/static-php-cli/bulk/php-8.4.23-cli-linux-aarch64.tar.gz"
        );
        assert_eq!(
            top.assets[1].filename,
            "php-8.4.23-fpm-linux-aarch64.tar.gz"
        );
        assert!(top.size().unwrap() > 10_000_000);
        assert_eq!(top.channel, Channel::Stable);
        assert_eq!(r.last().unwrap().channel, Channel::Eol);
        let mac = parse_php_static(&idx, MAC_X64, TODAY);
        assert!(mac[0].assets[0]
            .filename
            .ends_with("-cli-macos-x86_64.tar.gz"));
        assert!(parse_php_static(&idx, WIN, TODAY).is_empty());
        let lines = latest_per_line(RuntimeKind::Php, &r);
        assert_eq!(lines.len(), 3);
        assert_eq!(resolve_in(&r, "8.3").unwrap().version, "8.3.26");
    }

    #[test]
    fn php_windows() {
        let idx = fixture("php_windows_releases.json");
        let r = parse_php_windows(&idx, WIN, TODAY);
        assert_eq!(r.len(), 2);
        assert!(r[0].version.starts_with("8.4."));
        let a = &r[0].assets[0];
        assert!(a
            .url
            .starts_with("https://windows.php.net/downloads/releases/php-8.4."));
        assert!(
            a.filename.contains("-nts-Win32-vs17-x64.zip"),
            "{}",
            a.filename
        );
        assert!(matches!(a.checksum, Checksum::Sha256 { .. }));
        assert!(parse_php_windows(&idx, LINUX_X64, TODAY).is_empty());
    }

    #[test]
    fn node() {
        let idx = fixture("node_index.json");
        let sched = fixture("node_schedule.json");
        let r = parse_node(&idx, Some(&sched), LINUX_ARM, TODAY);
        assert_eq!(r[0].version, "26.11.1");
        let a = &r[0].assets[0];
        assert_eq!(
            a.url,
            "https://nodejs.org/dist/v26.11.1/node-v26.11.1-linux-arm64.tar.xz"
        );
        assert_eq!(
            a.checksum,
            Checksum::SumsFile {
                url: "https://nodejs.org/dist/v26.11.1/SHASUMS256.txt".into(),
                filename: "node-v26.11.1-linux-arm64.tar.xz".into()
            }
        );
        let ch = |v: &str| r.iter().find(|x| x.version == v).unwrap().channel;
        assert_eq!(ch("22.23.3"), Channel::Lts);
        assert_eq!(ch("16.20.2"), Channel::Eol);
        assert_eq!(ch("25.9.0"), Channel::Eol);
        assert_eq!(ch("26.11.1"), Channel::Current);
        // v0.12 has no linux-arm64 build
        assert!(r.iter().all(|x| !x.version.starts_with("0.")));
        let mac = parse_node(&idx, Some(&sched), MAC_ARM, TODAY);
        assert!(mac[0].assets[0].filename.ends_with("darwin-arm64.tar.gz"));
        let win = parse_node(&idx, None, WIN, TODAY);
        assert!(win[0].assets[0].filename.ends_with("win-x64.zip"));
        // without schedule the index lts flag is used
        assert_eq!(
            win.iter().find(|x| x.version == "22.23.3").unwrap().channel,
            Channel::Lts
        );
        let lines = latest_per_line(RuntimeKind::Node, &r);
        assert_eq!(lines.len(), 6);
        assert_eq!(resolve_in(&r, "22").unwrap().version, "22.23.3");
        assert_eq!(resolve_in(&r, "lts").unwrap().version, "24.21.0");
        assert_eq!(resolve_in(&r, "v20.20.1").unwrap().version, "20.20.1");
        assert!(resolve_in(&r, "18").is_none());
        let sums = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/node_shasums.txt"),
        )
        .unwrap();
        assert!(crate::download::parse_sums(&sums, "node-v22.23.3-linux-arm64.tar.xz").is_some());
    }

    #[test]
    fn bun() {
        let r = parse_bun(&fixture("bun_releases.json"), LINUX_ARM);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].channel, Channel::Current);
        assert_eq!(r[1].channel, Channel::Stable);
        let a = &r[0].assets[0];
        assert_eq!(
            a.url,
            format!(
                "https://github.com/oven-sh/bun/releases/download/bun-v{}/bun-linux-aarch64.zip",
                r[0].version
            )
        );
        assert!(matches!(a.checksum, Checksum::Sha256 { .. }));
        assert_eq!(r[0].released.as_deref().map(|d| d.len()), Some(10));
        let w = parse_bun(&fixture("bun_releases.json"), WIN);
        assert_eq!(w[0].assets[0].filename, "bun-windows-x64.zip");
        let m = parse_bun(&fixture("bun_releases.json"), MAC_X64);
        assert_eq!(m[0].assets[0].filename, "bun-darwin-x64.zip");
    }

    #[test]
    fn python() {
        let rel = fixture("python_release.json");
        let r = parse_python(&rel, LINUX_ARM, TODAY);
        assert!(r.len() >= 4);
        assert!(r.iter().all(|x| x.version.starts_with("3.")));
        // newest first, one entry per version, no pre-releases
        for w in r.windows(2) {
            assert_eq!(
                version::compare(&w[0].version, &w[1].version),
                std::cmp::Ordering::Greater
            );
        }
        let a = &r[0].assets[0];
        assert!(a
            .filename
            .ends_with("-aarch64-unknown-linux-gnu-install_only.tar.gz"));
        assert!(!a.filename.contains("freethreaded") && !a.filename.contains("stripped"));
        assert!(matches!(a.checksum, Checksum::Sha256 { .. }));
        let w = parse_python(&rel, WIN, TODAY);
        assert!(w[0].assets[0].filename.contains("x86_64-pc-windows-msvc"));
        let m = parse_python(&rel, MAC_ARM, "2031-12-01");
        assert!(m.iter().all(|x| x.channel == Channel::Eol));
        let mm = version::major_minor(&r[1].version);
        assert_eq!(resolve_in(&r, &mm).unwrap().version, r[1].version);
    }

    #[test]
    fn go() {
        let r = parse_go(&fixture("go_dl.json"), LINUX_ARM);
        let vs: Vec<_> = r.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(vs, vec!["1.27.2", "1.27.1", "1.26.9", "1.25.0"]);
        assert_eq!(
            r[0].assets[0].url,
            "https://go.dev/dl/go1.27.2.linux-arm64.tar.gz"
        );
        assert!(matches!(r[0].assets[0].checksum, Checksum::Sha256 { .. }));
        assert_eq!(r[2].channel, Channel::Stable);
        assert_eq!(r[3].channel, Channel::Eol);
        let w = parse_go(&fixture("go_dl.json"), WIN);
        assert_eq!(w[0].assets[0].filename, "go1.27.2.windows-amd64.zip");
        let m = parse_go(&fixture("go_dl.json"), MAC_ARM);
        assert_eq!(m[0].assets[0].filename, "go1.27.2.darwin-arm64.tar.gz");
    }

    #[tokio::test]
    async fn cache_is_used_within_ttl() {
        let t = tempfile::tempdir().unwrap();
        let c = Catalog::new(
            t.path().to_path_buf(),
            crate::download::http_client(),
            LINUX_ARM,
        );
        let rel = parse_go(&fixture("go_dl.json"), LINUX_ARM);
        c.cache
            .put(&c.key(RuntimeKind::Go), &LINUX_ARM.to_string(), &rel);
        // no network needed: served from cache
        assert_eq!(c.releases(RuntimeKind::Go, false).await.unwrap(), rel);
        assert_eq!(
            c.resolve(RuntimeKind::Go, "1.26").await.unwrap().version,
            "1.26.9"
        );
        // a failing fetch falls back to the (stale) cache
        let d = DiskCache::new(t.path().to_path_buf(), Duration::from_secs(0));
        let v: Vec<Release> = d
            .get_or_fetch(&c.key(RuntimeKind::Go), "x", true, || async {
                Err(CoreError::Upstream("offline".into()))
            })
            .await
            .unwrap();
        assert_eq!(v, rel);
    }
}
