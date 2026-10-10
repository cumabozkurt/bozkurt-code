//! `state.json`: sites, installed/active runtime versions and per-service state.
//! Always written atomically (temp file + rename).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::fsutil::atomic_write;
use crate::models::{RuntimeKind, Site};

/// Current schema version of `state.json`.
pub const STATE_VERSION: u32 = 1;

/// Installed versions of one runtime kind.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeState {
    pub installed: Vec<String>,
    pub active: Option<String>,
}

/// Persistent per-service state. Autostart lives in `config.toml` (`autostart = [...]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceState {
    /// Installed upstream version, if the service is installed.
    pub version: Option<String>,
    /// User override of the listening port.
    pub port: Option<u16>,
    /// True once the data directory has been initialised (mariadb/postgres).
    pub initialized: bool,
}

/// The whole `state.json` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub sites: Vec<Site>,
    pub runtimes: BTreeMap<RuntimeKind, RuntimeState>,
    pub services: BTreeMap<String, ServiceState>,
    /// Trust-on-first-use sha256 pins (download URL → hex digest) for assets whose
    /// upstream publishes no checksum (static-php.dev, P1-1).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub download_pins: BTreeMap<String, String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            version: STATE_VERSION,
            sites: vec![],
            runtimes: BTreeMap::new(),
            services: BTreeMap::new(),
            download_pins: BTreeMap::new(),
        }
    }
}

impl State {
    pub fn site(&self, id: &str) -> Option<&Site> {
        self.sites.iter().find(|s| s.id == id)
    }
    pub fn site_mut(&mut self, id: &str) -> Option<&mut Site> {
        self.sites.iter_mut().find(|s| s.id == id)
    }
    pub fn runtime(&self, kind: RuntimeKind) -> RuntimeState {
        self.runtimes.get(&kind).cloned().unwrap_or_default()
    }
    pub fn runtime_mut(&mut self, kind: RuntimeKind) -> &mut RuntimeState {
        self.runtimes.entry(kind).or_default()
    }
    pub fn service(&self, name: &str) -> ServiceState {
        self.services.get(name).cloned().unwrap_or_default()
    }
    pub fn service_mut(&mut self, name: &str) -> &mut ServiceState {
        self.services.entry(name.to_string()).or_default()
    }
}

/// Thread-safe owner of `state.json`. Every mutation goes through [`StateStore::update`],
/// which persists atomically before returning.
#[derive(Debug)]
pub struct StateStore {
    path: PathBuf,
    inner: Mutex<State>,
}

impl StateStore {
    /// Load `path` (missing → empty state). A corrupt file is moved aside to
    /// `state.json.corrupt-<ts>` and an empty state is used.
    pub fn open(path: &Path) -> Result<StateStore> {
        let state = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    let aside = path.with_extension(format!(
                        "json.corrupt-{}",
                        chrono::Utc::now().format("%Y%m%d%H%M%S")
                    ));
                    tracing::warn!("state.json unreadable ({e}); moved to {aside:?}");
                    std::fs::rename(path, &aside)?;
                    State::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(StateStore {
            path: path.to_path_buf(),
            inner: Mutex::new(state),
        })
    }

    /// A snapshot of the current state.
    pub fn get(&self) -> State {
        self.inner.lock().expect("state lock").clone()
    }

    /// Read with a closure without cloning everything.
    pub fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        f(&self.inner.lock().expect("state lock"))
    }

    /// Mutate and persist atomically. If `f` errors nothing is written and the in-memory
    /// state is left unchanged.
    pub fn update<R>(&self, f: impl FnOnce(&mut State) -> Result<R>) -> Result<R> {
        let mut guard = self.inner.lock().expect("state lock");
        let mut next = guard.clone();
        let r = f(&mut next)?;
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|e| CoreError::Other(anyhow::anyhow!("serialize state: {e}")))?;
        atomic_write(&self.path, &bytes)?;
        *guard = next;
        Ok(r)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SiteKind;

    fn site(id: &str) -> Site {
        Site {
            id: id.into(),
            domain: format!("{id}.test"),
            aliases: vec![],
            path: "/p".into(),
            kind: SiteKind::Static,
            root: None,
            php: None,
            node: None,
            proxy_port: None,
            command: None,
            app_port: None,
            app_status: None,
            https: true,
            wildcard: false,
            framework: None,
            env: Default::default(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn persist_and_reload() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("state.json");
        let s = StateStore::open(&p).unwrap();
        s.update(|st| {
            st.sites.push(site("a"));
            st.runtime_mut(RuntimeKind::Node)
                .installed
                .push("22.1.0".into());
            st.runtime_mut(RuntimeKind::Node).active = Some("22.1.0".into());
            st.service_mut("mariadb").port = Some(3307);
            Ok(())
        })
        .unwrap();
        let s2 = StateStore::open(&p).unwrap();
        let st = s2.get();
        assert_eq!(st.sites.len(), 1);
        assert_eq!(
            st.runtime(RuntimeKind::Node).active.as_deref(),
            Some("22.1.0")
        );
        assert_eq!(st.service("mariadb").port, Some(3307));
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert!(raw["runtimes"]["node"].is_object());
    }

    #[test]
    fn failed_update_changes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("state.json");
        let s = StateStore::open(&p).unwrap();
        let r: Result<()> = s.update(|st| {
            st.sites.push(site("x"));
            Err(CoreError::Conflict("no".into()))
        });
        assert!(r.is_err());
        assert!(s.get().sites.is_empty());
        assert!(!p.exists());
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("state.json");
        std::fs::write(&p, "{not json").unwrap();
        let s = StateStore::open(&p).unwrap();
        assert!(s.get().sites.is_empty());
        assert!(!p.exists());
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 1);
    }
}
