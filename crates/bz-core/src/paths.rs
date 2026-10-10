//! `$BOZKURT_HOME` resolution and the home directory layout (ARCHITECTURE.md).
//!
//! ```text
//! $BOZKURT_HOME/
//!   bin/  runtimes/<kind>/<ver>/  current/<kind>  services/<name>/  caddy/Caddyfile  caddy/bin/caddy
//!   ca/  certs/  logs/  cache/  tmp/  config.toml  state.json  backups/<stamp>/
//! ```

use std::path::{Path, PathBuf};

use crate::models::RuntimeKind;

/// Environment variable overriding the home directory.
pub const HOME_ENV: &str = "BOZKURT_HOME";

/// Resolve the BOZKURT home: `$BOZKURT_HOME`, else `~/.bozkurt` on macOS/Linux,
/// `%LOCALAPPDATA%\Bozkurt` on Windows.
pub fn default_home() -> PathBuf {
    if let Some(h) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(h);
    }
    platform_home()
}

#[cfg(windows)]
fn platform_home() -> PathBuf {
    if let Some(l) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(l).join("Bozkurt");
    }
    if let Some(p) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(p)
            .join("AppData")
            .join("Local")
            .join("Bozkurt");
    }
    PathBuf::from(r"C:\Bozkurt")
}

#[cfg(not(windows))]
fn platform_home() -> PathBuf {
    match std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        Some(h) => PathBuf::from(h).join(".bozkurt"),
        None => PathBuf::from("/tmp/.bozkurt"),
    }
}

/// All well-known locations inside a BOZKURT home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
}

impl Paths {
    /// Use `home` as the BOZKURT home (not created until [`Paths::ensure`]).
    pub fn new(home: impl Into<PathBuf>) -> Paths {
        Paths { home: home.into() }
    }

    /// Paths for [`default_home`].
    pub fn from_env() -> Paths {
        Paths::new(default_home())
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
    /// `bin/` — shims put on the user's PATH.
    pub fn bin(&self) -> PathBuf {
        self.home.join("bin")
    }
    /// `runtimes/`.
    pub fn runtimes(&self) -> PathBuf {
        self.home.join("runtimes")
    }
    /// `runtimes/<kind>/`.
    pub fn runtime_kind(&self, kind: RuntimeKind) -> PathBuf {
        self.runtimes().join(kind.as_str())
    }
    /// `runtimes/<kind>/<version>/`.
    pub fn runtime_dir(&self, kind: RuntimeKind, version: &str) -> PathBuf {
        self.runtime_kind(kind).join(version)
    }
    /// `runtimes/composer/composer.phar`.
    pub fn composer_phar(&self) -> PathBuf {
        self.runtimes().join("composer").join("composer.phar")
    }
    /// `current/`.
    pub fn current(&self) -> PathBuf {
        self.home.join("current")
    }
    /// `current/<kind>` — symlink/junction to the active version.
    pub fn current_link(&self, kind: RuntimeKind) -> PathBuf {
        self.current().join(kind.as_str())
    }
    /// `services/`.
    pub fn services(&self) -> PathBuf {
        self.home.join("services")
    }
    /// `services/<name>/`.
    pub fn service_dir(&self, name: &str) -> PathBuf {
        self.services().join(name)
    }
    /// `caddy/`.
    pub fn caddy(&self) -> PathBuf {
        self.home.join("caddy")
    }
    /// `caddy/Caddyfile`.
    pub fn caddyfile(&self) -> PathBuf {
        self.caddy().join("Caddyfile")
    }
    /// `caddy/bin/` (contains the caddy binary).
    pub fn caddy_bin_dir(&self) -> PathBuf {
        self.caddy().join("bin")
    }
    /// `caddy/run/` (mode 0700) — holds the admin API socket `admin.sock` (P1-3).
    pub fn caddy_run(&self) -> PathBuf {
        self.caddy().join("run")
    }
    /// `caddy/data/` — Caddy's own storage (XDG_DATA_HOME / AppData for the child).
    pub fn caddy_data(&self) -> PathBuf {
        self.caddy().join("data")
    }
    /// `caddy/snippets/<domain>.caddy` — optional per-site custom snippet.
    pub fn caddy_snippet(&self, domain: &str) -> PathBuf {
        self.caddy()
            .join("snippets")
            .join(format!("{domain}.caddy"))
    }
    /// `ca/`.
    pub fn ca(&self) -> PathBuf {
        self.home.join("ca")
    }
    /// `certs/`.
    pub fn certs(&self) -> PathBuf {
        self.home.join("certs")
    }
    /// `logs/`.
    pub fn logs(&self) -> PathBuf {
        self.home.join("logs")
    }
    /// `logs/<component>.log`.
    pub fn log_file(&self, component: &str) -> PathBuf {
        self.logs().join(format!("{component}.log"))
    }
    /// `cache/` — upstream catalog cache.
    pub fn cache(&self) -> PathBuf {
        self.home.join("cache")
    }
    /// `tmp/` — downloads and extraction staging (same filesystem → atomic rename).
    pub fn tmp(&self) -> PathBuf {
        self.home.join("tmp")
    }
    /// `backups/`.
    pub fn backups(&self) -> PathBuf {
        self.home.join("backups")
    }
    /// `backups/<stamp>/` for a new backup set.
    pub fn backup_set(&self, stamp: &str) -> PathBuf {
        self.backups().join(stamp)
    }
    /// `run/` — daemon runtime state (`children.json`, P1-7).
    pub fn run(&self) -> PathBuf {
        self.home.join("run")
    }
    /// `run/children.json` — pid/start time of every supervised child, used to reap
    /// orphans after a daemon crash.
    pub fn children_file(&self) -> PathBuf {
        self.run().join("children.json")
    }
    /// `config.toml`.
    pub fn config(&self) -> PathBuf {
        self.home.join("config.toml")
    }
    /// `state.json`.
    pub fn state(&self) -> PathBuf {
        self.home.join("state.json")
    }

    /// Every directory that must exist in a usable home.
    pub fn all_dirs(&self) -> Vec<PathBuf> {
        let mut v = vec![
            self.bin(),
            self.runtimes(),
            self.current(),
            self.services(),
            self.caddy(),
            self.caddy_bin_dir(),
            self.caddy_data(),
            self.caddy().join("snippets"),
            self.ca(),
            self.certs(),
            self.logs(),
            self.cache(),
            self.tmp(),
            self.backups(),
            self.run(),
        ];
        for k in RuntimeKind::ALL {
            v.push(self.runtime_kind(k));
        }
        v
    }

    /// Create the home and every sub-directory (idempotent). `ca/` is 0700 on unix.
    pub fn ensure(&self) -> std::io::Result<()> {
        for d in self.all_dirs() {
            std::fs::create_dir_all(&d)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(self.ca(), std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let p = Paths::new("/h");
        assert_eq!(p.caddyfile(), PathBuf::from("/h/caddy/Caddyfile"));
        assert_eq!(
            p.runtime_dir(RuntimeKind::Node, "22.1.0"),
            PathBuf::from("/h/runtimes/node/22.1.0")
        );
        assert_eq!(
            p.current_link(RuntimeKind::Php),
            PathBuf::from("/h/current/php")
        );
        assert_eq!(p.log_file("caddy"), PathBuf::from("/h/logs/caddy.log"));
    }

    #[test]
    fn ensure_creates_dirs() {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::new(t.path().join("home"));
        p.ensure().unwrap();
        for d in p.all_dirs() {
            assert!(d.is_dir(), "{d:?}");
        }
    }
}
