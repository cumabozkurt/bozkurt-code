//! `config.toml` (user configuration) and per-project `bozkurt.toml`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::fsutil::atomic_write;
use crate::models::SiteKind;

/// Remote (LAN) listener settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    pub enabled: bool,
    pub port: u16,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        RemoteConfig {
            enabled: false,
            port: 7879,
        }
    }
}

/// `config.toml`. Unknown keys are preserved in `extra` so other components (daemon,
/// net) can store their own settings without bz-core dropping them on save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub tld: String,
    pub dns_port: u16,
    /// Preferred HTTP port; falls back to 8080 if 80 cannot be bound.
    pub http_port: u16,
    /// Preferred HTTPS port; falls back to 8443 if 443 cannot be bound.
    pub https_port: u16,
    pub api_port: u16,
    /// Services started when the daemon starts.
    pub autostart: Vec<String>,
    pub remote: RemoteConfig,
    /// Serve `.test` sites on every interface instead of loopback only (P0-2). Off by
    /// default: Caddy binds `127.0.0.1` / `::1`.
    pub lan_access: bool,
    /// Accept downloads whose upstream publishes no checksum (static-php.dev PHP builds,
    /// and Windows PHP / Go / cloudflared when the index drops its hash). Off by default:
    /// such downloads are refused unless a sha256 was pinned on an earlier, explicitly
    /// allowed install (P1-1). A single install can also pass `allow_unverified: true`.
    pub allow_unverified: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            tld: "test".into(),
            dns_port: 5390,
            http_port: 80,
            https_port: 443,
            api_port: 7878,
            autostart: vec!["caddy".into(), "dns".into()],
            remote: RemoteConfig::default(),
            lan_access: false,
            allow_unverified: false,
            extra: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Load from `path`; a missing file yields defaults.
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(s) => Config::from_toml(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn from_toml(s: &str) -> Result<Config> {
        let c: Config = toml::from_str(s)?;
        c.validate()?;
        Ok(c)
    }

    /// Save atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let s = toml::to_string_pretty(self)
            .map_err(|e| CoreError::Other(anyhow::anyhow!("serialize config: {e}")))?;
        atomic_write(path, s.as_bytes())?;
        Ok(())
    }

    /// Basic sanity checks.
    pub fn validate(&self) -> Result<()> {
        if self.tld.is_empty()
            || !self
                .tld
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(CoreError::InvalidInput(format!(
                "invalid tld `{}`",
                self.tld
            )));
        }
        if self.tld == "dev" || self.tld == "app" {
            return Err(CoreError::InvalidInput(format!(
                "`.{}` is a real HSTS-preloaded TLD; use `test` or another reserved name",
                self.tld
            )));
        }
        self.validate_ports()
    }

    /// Every port in `1..=65535` and pairwise distinct (D1). `0` is an error, not "unset".
    pub fn validate_ports(&self) -> Result<()> {
        let ports = self.ports();
        for (key, p) in &ports {
            if *p == 0 {
                return Err(CoreError::InvalidInput(format!(
                    "{key} must be between 1 and 65535"
                )));
            }
        }
        for (i, (key, p)) in ports.iter().enumerate() {
            if let Some((other, _)) = ports[..i].iter().find(|(_, q)| q == p) {
                return Err(CoreError::InvalidInput(format!(
                    "{key} ({p}) is already used by {other}"
                )));
            }
        }
        Ok(())
    }

    /// `(key, port)` for every port the daemon binds, in a stable order.
    pub fn ports(&self) -> [(&'static str, u16); 5] {
        [
            ("http_port", self.http_port),
            ("https_port", self.https_port),
            ("dns_port", self.dns_port),
            ("api_port", self.api_port),
            ("remote.port", self.remote.port),
        ]
    }

    /// The config as JSON (for `GET /config`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Apply a JSON merge patch (`PATCH /config`) and return the new config.
    pub fn patched(&self, patch: &serde_json::Value) -> Result<Config> {
        if !patch.is_object() {
            return Err(CoreError::InvalidInput(
                "config patch must be an object".into(),
            ));
        }
        let mut v = self.to_json();
        merge_json(&mut v, patch);
        let c: Config = serde_json::from_value(v)?;
        c.validate()?;
        Ok(c)
    }
}

/// RFC 7386 JSON merge patch.
pub fn merge_json(target: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Default::default());
            }
            let t = target.as_object_mut().expect("object");
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_json(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Per-project `bozkurt.toml` (optional, committed to the project repo).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectFile {
    /// `shop` → `shop.test`; a full domain is kept as is.
    pub domain: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<SiteKind>,
    pub root: Option<String>,
    pub php: Option<String>,
    pub node: Option<String>,
    pub python: Option<String>,
    pub bun: Option<String>,
    pub go: Option<String>,
    pub command: Option<String>,
    pub aliases: Vec<String>,
    pub services: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl ProjectFile {
    pub const FILE_NAME: &'static str = "bozkurt.toml";

    pub fn parse(s: &str) -> Result<ProjectFile> {
        Ok(toml::from_str(s)?)
    }

    /// Load `<dir>/bozkurt.toml` if present.
    pub fn load_from_dir(dir: &Path) -> Result<Option<ProjectFile>> {
        let p = dir.join(Self::FILE_NAME);
        match std::fs::read_to_string(&p) {
            Ok(s) => Ok(Some(ProjectFile::parse(&s)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_architecture() {
        let c = Config::default();
        assert_eq!(c.tld, "test");
        assert_eq!(c.dns_port, 5390);
        assert_eq!((c.http_port, c.https_port, c.api_port), (80, 443, 7878));
        assert_eq!(c.autostart, vec!["caddy", "dns"]);
        assert!(!c.remote.enabled);
        assert_eq!(c.remote.port, 7879);
        assert!(!c.lan_access);
        assert!(!c.allow_unverified);
    }

    #[test]
    fn load_save_roundtrip_keeps_unknown_keys() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("config.toml");
        assert_eq!(Config::load(&p).unwrap(), Config::default());
        std::fs::write(
            &p,
            "tld = \"local\"\nhttp_port = 8080\nfuture_key = 3\n[remote]\nenabled = true\n",
        )
        .unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(c.tld, "local");
        assert_eq!(c.http_port, 8080);
        assert_eq!(c.https_port, 443);
        assert!(c.remote.enabled);
        assert_eq!(c.remote.port, 7879);
        c.save(&p).unwrap();
        let again = Config::load(&p).unwrap();
        assert_eq!(again, c);
        assert!(std::fs::read_to_string(&p).unwrap().contains("future_key"));
    }

    #[test]
    fn patch() {
        let c = Config::default();
        let n = c
            .patched(&serde_json::json!({"tld": "localhost", "remote": {"enabled": true}}))
            .unwrap();
        assert_eq!(n.tld, "localhost");
        assert!(n.remote.enabled);
        assert_eq!(n.remote.port, 7879);
        assert!(c.patched(&serde_json::json!({"tld": "dev"})).is_err());
        assert!(c.patched(&serde_json::json!({"tld": "a b"})).is_err());
    }

    #[test]
    fn ports_are_validated() {
        let c = Config::default();
        let e = c
            .patched(&serde_json::json!({"https_port": 0}))
            .unwrap_err()
            .to_string();
        assert_eq!(e, "invalid input: https_port must be between 1 and 65535");
        let e = c
            .patched(&serde_json::json!({"remote": {"port": 0}}))
            .unwrap_err();
        assert_eq!(e.code(), "invalid_input");
        let e = c
            .patched(&serde_json::json!({"http_port": 443, "https_port": 443}))
            .unwrap_err()
            .to_string();
        assert_eq!(
            e,
            "invalid input: https_port (443) is already used by http_port"
        );
        let e = c
            .patched(&serde_json::json!({"remote": {"port": 7878}}))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("remote.port (7878) is already used by api_port"),
            "{e}"
        );
        assert!(c.patched(&serde_json::json!({"http_port": 70000})).is_err());
        assert!(c.patched(&serde_json::json!({"dns_port": 5353})).is_ok());
        assert!(Config::from_toml("http_port = 0\n").is_err());
    }

    #[test]
    fn project_file() {
        let p = ProjectFile::parse(
            r#"
domain = "shop"
type = "php"
root = "public"
php = "8.3"
node = "22"
command = "npm run dev"
aliases = ["admin.shop"]
services = ["mariadb", "redis", "mailpit"]
[env]
APP_ENV = "local"
"#,
        )
        .unwrap();
        assert_eq!(p.domain.as_deref(), Some("shop"));
        assert_eq!(p.kind, Some(SiteKind::Php));
        assert_eq!(p.root.as_deref(), Some("public"));
        assert_eq!(p.php.as_deref(), Some("8.3"));
        assert_eq!(p.aliases, vec!["admin.shop"]);
        assert_eq!(p.services.len(), 3);
        assert_eq!(p.env["APP_ENV"], "local");
    }
}
