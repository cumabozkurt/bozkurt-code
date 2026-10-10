//! Runtime install / uninstall / switch, and the `bin/` shims.
//!
//! * Versions live in `runtimes/<kind>/<version>/` (extracted atomically).
//! * The global version is `current/<kind>` → symlink (junction on Windows).
//! * Shims in `bin/` (`sh` scripts on unix, `.cmd` on Windows) pick the version per
//!   project: `BOZKURT_<KIND>_VERSION` env, then walking up from the working directory
//!   looking for `bozkurt.toml` and the ecosystem files (`.nvmrc`, `.node-version`,
//!   `composer.json` `require.php`, `.python-version`, `go.mod`), then the global version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::catalogs::{latest_per_line, Catalog, Release};
use crate::download::{self, install_tree};
use crate::error::{CoreError, Result};
use crate::events::{Event, EventBus};
use crate::fsutil;
use crate::jobs::JobHandle;
use crate::models::{Channel, Runtime, RuntimeKind, Version};
use crate::paths::Paths;
use crate::platform::Platform;
use crate::state::StateStore;
use crate::version;

/// One command exposed in `bin/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShimSpec {
    /// Command name (`node`, `npm`, `composer`, ...).
    pub name: &'static str,
    pub kind: RuntimeKind,
    /// Binary relative to the version dir on unix.
    pub unix_rel: &'static str,
    /// Binary relative to the version dir on Windows.
    pub win_rel: &'static str,
    /// Extra leading arguments.
    pub pre: ShimPre,
}

/// Leading arguments injected by a shim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShimPre {
    None,
    /// Run `runtimes/composer/composer.phar` with the selected PHP.
    ComposerPhar,
    /// A literal argument (`bunx` → `bun x`).
    Literal(&'static str),
}

macro_rules! shim {
    ($n:expr, $k:ident, $u:expr, $w:expr) => {
        shim!($n, $k, $u, $w, ShimPre::None)
    };
    ($n:expr, $k:ident, $u:expr, $w:expr, $p:expr) => {
        ShimSpec {
            name: $n,
            kind: RuntimeKind::$k,
            unix_rel: $u,
            win_rel: $w,
            pre: $p,
        }
    };
}

/// Every shim BOZKURT writes.
pub const SHIMS: &[ShimSpec] = &[
    shim!("php", Php, "php", "php.exe"),
    shim!("composer", Php, "php", "php.exe", ShimPre::ComposerPhar),
    shim!("node", Node, "bin/node", "node.exe"),
    shim!("npm", Node, "bin/npm", "npm.cmd"),
    shim!("npx", Node, "bin/npx", "npx.cmd"),
    shim!("corepack", Node, "bin/corepack", "corepack.cmd"),
    shim!("bun", Bun, "bun", "bun.exe"),
    shim!("bunx", Bun, "bun", "bun.exe", ShimPre::Literal("x")),
    shim!("python", Python, "bin/python3", "python.exe"),
    shim!("python3", Python, "bin/python3", "python.exe"),
    shim!("pip", Python, "bin/pip3", "Scripts/pip.exe"),
    shim!("pip3", Python, "bin/pip3", "Scripts/pip.exe"),
    shim!("go", Go, "bin/go", "bin/go.exe"),
    shim!("gofmt", Go, "bin/gofmt", "bin/gofmt.exe"),
];

/// Main binary of a runtime relative to its version dir.
pub fn main_binary(kind: RuntimeKind, p: Platform) -> &'static str {
    let s = SHIMS
        .iter()
        .find(|s| s.kind == kind && s.pre == ShimPre::None)
        .expect("every kind has a main shim");
    if p.is_windows() {
        s.win_rel
    } else {
        s.unix_rel
    }
}

/// Directory (relative to the version dir) holding a runtime's executables.
pub fn bin_subdir(kind: RuntimeKind, p: Platform) -> &'static str {
    match (kind, p.is_windows()) {
        (RuntimeKind::Node, false) | (RuntimeKind::Python, false) | (RuntimeKind::Go, _) => "bin",
        _ => "",
    }
}

/// Env var that forces a version for shims and child processes.
pub fn version_env(kind: RuntimeKind) -> String {
    format!("BOZKURT_{}_VERSION", kind.as_str().to_ascii_uppercase())
}

/// Find the version a project asks for, walking up from `start`.
/// Returns the reduced constraint (e.g. `8.2` for `^8.2`), or `None`.
pub fn project_version(kind: RuntimeKind, start: &Path) -> Option<String> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if let Some(v) = version_in_dir(kind, d) {
            return Some(v);
        }
        dir = d.parent();
    }
    None
}

/// The version requested by files directly inside `d` (no walking).
pub fn version_in_dir(kind: RuntimeKind, d: &Path) -> Option<String> {
    if let Ok(Some(pf)) = crate::config::ProjectFile::load_from_dir(d) {
        let v = match kind {
            RuntimeKind::Php => pf.php,
            RuntimeKind::Node => pf.node,
            RuntimeKind::Python => pf.python,
            RuntimeKind::Bun => pf.bun,
            RuntimeKind::Go => pf.go,
        };
        if let Some(v) = v.and_then(|v| version::constraint_prefix(&v)) {
            return Some(v);
        }
    }
    let read = |f: &str| std::fs::read_to_string(d.join(f)).ok();
    let first_line = |s: String| s.lines().next().unwrap_or("").trim().to_string();
    match kind {
        RuntimeKind::Node => {
            for f in [".nvmrc", ".node-version"] {
                if let Some(s) = read(f) {
                    return version::constraint_prefix(&first_line(s));
                }
            }
            let pkg: serde_json::Value = serde_json::from_str(&read("package.json")?).ok()?;
            pkg.pointer("/engines/node")
                .and_then(|v| v.as_str())
                .and_then(version::constraint_prefix)
        }
        RuntimeKind::Php => {
            let c: serde_json::Value = serde_json::from_str(&read("composer.json")?).ok()?;
            c.pointer("/require/php")
                .and_then(|v| v.as_str())
                .and_then(version::constraint_prefix)
        }
        RuntimeKind::Python => read(".python-version")
            .map(first_line)
            .and_then(|s| version::constraint_prefix(&s)),
        RuntimeKind::Go => read("go.mod").and_then(|s| {
            s.lines()
                .find_map(|l| l.trim().strip_prefix("go ").map(|v| v.trim().to_string()))
        }),
        RuntimeKind::Bun => None,
    }
}

/// Per-`(kind, version)` install locks (P1-22).
type InstallLocks = HashMap<(RuntimeKind, String), Arc<tokio::sync::Mutex<()>>>;

/// Installs, removes and switches runtimes.
#[derive(Debug, Clone)]
pub struct RuntimeManager {
    paths: Paths,
    platform: Platform,
    catalog: Catalog,
    client: reqwest::Client,
    state: Arc<StateStore>,
    events: EventBus,
    /// One async lock per `(kind, version)` being installed (P1-22).
    install_locks: Arc<Mutex<InstallLocks>>,
}

impl RuntimeManager {
    pub fn new(
        paths: Paths,
        platform: Platform,
        catalog: Catalog,
        client: reqwest::Client,
        state: Arc<StateStore>,
        events: EventBus,
    ) -> RuntimeManager {
        RuntimeManager {
            paths,
            platform,
            catalog,
            client,
            state,
            events,
            install_locks: Arc::default(),
        }
    }

    /// The install lock of `kind` `version` (created on first use).
    fn install_lock(&self, kind: RuntimeKind, version: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.install_locks
            .lock()
            .expect("install locks")
            .entry((kind, version.to_string()))
            .or_default()
            .clone()
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Versions on disk (newest first). Hidden staging dirs are ignored.
    pub fn installed(&self, kind: RuntimeKind) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.paths.runtime_kind(kind))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        version::sort_desc(&mut v);
        v
    }

    /// The active (global) version, from the `current/<kind>` link.
    pub fn active(&self, kind: RuntimeKind) -> Option<String> {
        let target = fsutil::read_dir_link(&self.paths.current_link(kind))?;
        if !target.exists() {
            return None;
        }
        target.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    pub fn runtime(&self, kind: RuntimeKind) -> Runtime {
        Runtime {
            kind,
            active: self.active(kind),
            installed: self.installed(kind),
        }
    }

    pub fn all(&self) -> Vec<Runtime> {
        RuntimeKind::ALL.iter().map(|k| self.runtime(*k)).collect()
    }

    /// Newest installed version matching `want` (exact or prefix).
    pub fn find_installed(&self, kind: RuntimeKind, want: &str) -> Option<String> {
        version::best_match(&self.installed(kind), want)
    }

    /// Absolute path to `kind`'s main binary for `version`.
    pub fn binary(&self, kind: RuntimeKind, version: &str) -> PathBuf {
        self.paths
            .runtime_dir(kind, version)
            .join(main_binary(kind, self.platform))
    }

    /// Directory with the executables of `kind` `version` (for PATH).
    pub fn bin_dir(&self, kind: RuntimeKind, version: &str) -> PathBuf {
        let d = self.paths.runtime_dir(kind, version);
        match bin_subdir(kind, self.platform) {
            "" => d,
            s => d.join(s),
        }
    }

    /// Resolve the version to use for a project directory: explicit `pin` (site setting),
    /// then project files, then the active version.
    pub fn resolve_for_dir(
        &self,
        kind: RuntimeKind,
        dir: &Path,
        pin: Option<&str>,
    ) -> Option<String> {
        let want = pin
            .and_then(version::constraint_prefix)
            .or_else(|| project_version(kind, dir));
        want.and_then(|w| self.find_installed(kind, &w))
            .or_else(|| self.active(kind))
    }

    /// `GET /runtimes/{kind}/versions`: the newest release of each line from the live
    /// catalog, merged with installed versions.
    pub async fn versions(&self, kind: RuntimeKind, refresh: bool) -> Result<Vec<Version>> {
        let installed = self.installed(kind);
        let active = self.active(kind);
        let all = self.catalog.releases(kind, refresh).await?;
        let mut view = latest_per_line(kind, &all);
        for v in &installed {
            if !view.iter().any(|r| &r.version == v) {
                view.push(
                    all.iter()
                        .find(|r| &r.version == v)
                        .cloned()
                        .unwrap_or(Release {
                            version: v.clone(),
                            channel: Channel::Stable,
                            released: None,
                            assets: vec![],
                        }),
                );
            }
        }
        view.sort_by(|a, b| version::compare(&b.version, &a.version));
        Ok(view
            .into_iter()
            .map(|r| Version {
                installed: installed.contains(&r.version),
                active: active.as_deref() == Some(r.version.as_str()),
                size_bytes: r.size(),
                released: r.released.clone(),
                channel: r.channel,
                version: r.version,
            })
            .collect())
    }

    /// Download, verify and install `want` (exact, prefix, `latest`, `lts`). Becomes the
    /// active version if none is active yet. Returns the installed version.
    ///
    /// `allow_unverified` lets an asset without an upstream checksum (static-php.dev) be
    /// installed once and its sha256 pinned (P1-1); otherwise such installs are refused
    /// unless a pin exists.
    pub async fn install(
        &self,
        kind: RuntimeKind,
        want: &str,
        job: Option<&JobHandle>,
        allow_unverified: bool,
    ) -> Result<String> {
        let release = self.catalog.resolve(kind, want).await?;
        let dest = self.paths.runtime_dir(kind, &release.version);
        // Concurrent installs of the same version (UI double-click, UI + CLI) wait for the
        // first one and then find it installed, instead of downloading twice and failing
        // in `install_tree` (P1-22).
        let lock = self.install_lock(kind, &release.version);
        let _guard = match lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                if let Some(j) = job {
                    j.progress(
                        0.0,
                        Some(&format!(
                            "Waiting for another install of {kind} {}",
                            release.version
                        )),
                    );
                }
                lock.lock().await
            }
        };
        if dest.exists() {
            if let Some(j) = job {
                j.progress(
                    1.0,
                    Some(&format!("{kind} {} already installed", release.version)),
                );
            }
            return Ok(release.version);
        }
        let tmp = self
            .paths
            .tmp()
            .join(format!("dl-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp)?;
        let res = self
            .install_release(kind, &release, &tmp, &dest, job, allow_unverified)
            .await;
        let _ = std::fs::remove_dir_all(&tmp);
        res?;
        if kind == RuntimeKind::Php {
            if let Err(e) = self.ensure_composer().await {
                tracing::warn!("composer download failed: {e}");
            }
        }
        self.state.update(|s| {
            let rs = s.runtime_mut(kind);
            if !rs.installed.contains(&release.version) {
                rs.installed.push(release.version.clone());
                version::sort_desc(&mut rs.installed);
            }
            Ok(())
        })?;
        if self.active(kind).is_none() {
            self.use_version(kind, &release.version)?;
        } else {
            self.write_shims()?;
            self.events.emit(Event::RuntimeChanged {
                runtime: self.runtime(kind),
            });
        }
        Ok(release.version)
    }

    async fn install_release(
        &self,
        kind: RuntimeKind,
        release: &Release,
        tmp: &Path,
        dest: &Path,
        job: Option<&JobHandle>,
        allow_unverified: bool,
    ) -> Result<()> {
        let n = release.assets.len().max(1) as f64;
        let mut files = vec![];
        for (i, a) in release.assets.iter().enumerate() {
            let file = tmp.join(&a.filename);
            if let Some(j) = job {
                j.progress(
                    0.85 * i as f64 / n,
                    Some(&format!("Downloading {}", a.filename)),
                );
            }
            let jj = job.cloned();
            let (_, verified) = download::download_tofu(
                &self.client,
                &self.state,
                &a.url,
                &file,
                &a.checksum,
                allow_unverified,
                move |done, total| {
                    if let (Some(j), Some(t)) = (&jj, total) {
                        if t > 0 {
                            j.progress(0.85 * (i as f64 + done as f64 / t as f64) / n, None);
                        }
                    }
                },
            )
            .await?;
            if let (Some(j), download::Verification::FirstUse) = (job, verified) {
                j.progress(
                    0.85 * (i + 1) as f64 / n,
                    Some(&format!(
                        "Unverified download: {} has no published checksum; sha256 pinned for later installs",
                        a.filename
                    )),
                );
            }
            files.push(file);
        }
        if let Some(j) = job {
            j.progress(0.87, Some("Extracting"));
        }
        let dest = dest.to_path_buf();
        let platform = self.platform;
        tokio::task::spawn_blocking(move || {
            install_tree(&files, &dest, true, false, |staging| {
                post_install(kind, platform, staging)
            })
        })
        .await
        .map_err(|e| CoreError::Other(anyhow::anyhow!("extract task: {e}")))??;
        if let Some(j) = job {
            j.progress(0.98, Some("Installed"));
        }
        Ok(())
    }

    /// Download composer.phar (verified against getcomposer.org's published sha256) if absent.
    pub async fn ensure_composer(&self) -> Result<()> {
        let phar = self.paths.composer_phar();
        if phar.exists() {
            return Ok(());
        }
        let tmp = phar.with_extension("phar.part");
        download::download(
            &self.client,
            crate::catalogs::urls::COMPOSER_PHAR,
            &tmp,
            &download::Checksum::SumsFile {
                url: crate::catalogs::urls::COMPOSER_SHA.into(),
                filename: "composer.phar".into(),
            },
            |_, _| {},
        )
        .await?;
        std::fs::rename(&tmp, &phar)?;
        Ok(())
    }

    /// Remove an installed version. If it was active, the newest remaining version
    /// becomes active (or none).
    pub fn uninstall(&self, kind: RuntimeKind, version: &str) -> Result<()> {
        let lock = self.install_lock(kind, version);
        let Ok(_guard) = lock.try_lock() else {
            return Err(CoreError::Conflict(format!(
                "{kind} {version} is being installed"
            )));
        };
        let dir = self.paths.runtime_dir(kind, version);
        if version.is_empty()
            || version.contains(['/', '\\'])
            || version.starts_with('.')
            || !dir.is_dir()
        {
            return Err(CoreError::NotFound(format!(
                "{kind} {version} is not installed"
            )));
        }
        let was_active = self.active(kind).as_deref() == Some(version);
        if was_active {
            fsutil::remove_dir_link(&self.paths.current_link(kind))?;
        }
        let aside = self
            .paths
            .runtime_kind(kind)
            .join(format!(".rm-{}", uuid::Uuid::new_v4().simple()));
        std::fs::rename(&dir, &aside)?;
        std::fs::remove_dir_all(&aside)?;
        self.state.update(|s| {
            let rs = s.runtime_mut(kind);
            rs.installed.retain(|v| v != version);
            if was_active {
                rs.active = None;
            }
            Ok(())
        })?;
        if was_active {
            if let Some(next) = self.installed(kind).first().cloned() {
                return self.use_version(kind, &next).map(|_| ());
            }
        }
        self.events.emit(Event::RuntimeChanged {
            runtime: self.runtime(kind),
        });
        Ok(())
    }

    /// Make `want` (exact or prefix of an installed version) the global version.
    pub fn use_version(&self, kind: RuntimeKind, want: &str) -> Result<Runtime> {
        let v = self
            .find_installed(kind, want)
            .ok_or_else(|| CoreError::NotFound(format!("{kind} {want} is not installed")))?;
        fsutil::replace_dir_link(
            &self.paths.runtime_dir(kind, &v),
            &self.paths.current_link(kind),
        )?;
        self.state.update(|s| {
            s.runtime_mut(kind).active = Some(v.clone());
            Ok(())
        })?;
        self.write_shims()?;
        let rt = self.runtime(kind);
        self.events.emit(Event::RuntimeChanged {
            runtime: rt.clone(),
        });
        Ok(rt)
    }

    /// (Re)write every shim in `bin/`. Idempotent.
    pub fn write_shims(&self) -> Result<()> {
        write_shims(&self.paths, self.platform)
    }
}

/// Platform-specific fix-ups inside the staging dir before it is renamed into place.
fn post_install(kind: RuntimeKind, platform: Platform, staging: &Path) -> Result<()> {
    if kind == RuntimeKind::Php && platform.is_windows() {
        let ini = staging.join("php.ini");
        let dev = staging.join("php.ini-development");
        if !ini.exists() && dev.exists() {
            let mut s = std::fs::read_to_string(&dev)?;
            s = s.replace(";extension_dir = \"ext\"", "extension_dir = \"ext\"");
            for ext in [
                "curl",
                "fileinfo",
                "gd",
                "intl",
                "mbstring",
                "openssl",
                "pdo_mysql",
                "pdo_pgsql",
                "pdo_sqlite",
                "sqlite3",
                "zip",
                "mysqli",
                "sodium",
            ] {
                s = s.replace(
                    &format!(";extension={ext}\n"),
                    &format!("extension={ext}\n"),
                );
                s = s.replace(
                    &format!(";extension={ext}\r\n"),
                    &format!("extension={ext}\r\n"),
                );
            }
            std::fs::write(&ini, s)?;
        }
    }
    if !platform.is_windows() {
        // Make sure the main binary is executable even if the archive lost the mode bits.
        let main = staging.join(main_binary(kind, platform));
        if main.exists() {
            fsutil::make_executable(&main)?;
        }
        let fpm = staging.join("php-fpm");
        if kind == RuntimeKind::Php && fpm.exists() {
            fsutil::make_executable(&fpm)?;
        }
    }
    Ok(())
}

/// Write all shims for `platform` into `paths.bin()`.
pub fn write_shims(paths: &Paths, platform: Platform) -> Result<()> {
    std::fs::create_dir_all(paths.bin())?;
    for s in SHIMS {
        if platform.is_windows() {
            let body = cmd_shim(paths.home(), s);
            fsutil::atomic_write(
                &paths.bin().join(format!("{}.cmd", s.name)),
                body.as_bytes(),
            )?;
        } else {
            let p = paths.bin().join(s.name);
            fsutil::atomic_write(&p, sh_shim(paths.home(), s).as_bytes())?;
            fsutil::make_executable(&p)?;
        }
    }
    Ok(())
}

/// Escape for use inside a double-quoted sh string.
fn sh_dq_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '"' | '$' | '`' | '\\') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// Body of a POSIX `sh` shim.
pub fn sh_shim(home: &Path, s: &ShimSpec) -> String {
    let kind = s.kind.as_str();
    let env = version_env(s.kind);
    let rel = s.unix_rel;
    let path_dir = match Path::new(rel)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
    {
        Some(b) if !b.is_empty() => format!("$dir/{b}"),
        _ => "$dir".to_string(),
    };
    let mut checks = format!(
        "    if [ -f \"$d/bozkurt.toml\" ]; then\n      v=$(sed -n 's/^[[:space:]]*{kind}[[:space:]]*=[[:space:]]*[\"'\\'']\\([^\"'\\'']*\\)[\"'\\''].*/\\1/p' \"$d/bozkurt.toml\" | head -n 1)\n      [ -n \"$v\" ] && {{ want=\"$v\"; break; }}\n    fi\n"
    );
    let files: &[&str] = match s.kind {
        RuntimeKind::Node => &[".nvmrc", ".node-version"],
        RuntimeKind::Python => &[".python-version"],
        _ => &[],
    };
    for f in files {
        checks.push_str(&format!(
            "    if [ -f \"$d/{f}\" ]; then want=$(head -n 1 \"$d/{f}\"); break; fi\n"
        ));
    }
    match s.kind {
        RuntimeKind::Php => checks.push_str(
            "    if [ -f \"$d/composer.json\" ]; then\n      v=$(tr -d '\\n' < \"$d/composer.json\" | sed -n 's/.*\"require\"[[:space:]]*:[[:space:]]*{[^}]*\"php\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p')\n      [ -n \"$v\" ] && { want=\"$v\"; break; }\n    fi\n",
        ),
        RuntimeKind::Go => checks.push_str(
            "    if [ -f \"$d/go.mod\" ]; then\n      v=$(sed -n 's/^go[[:space:]]\\{1,\\}\\([0-9.]*\\).*/\\1/p' \"$d/go.mod\" | head -n 1)\n      [ -n \"$v\" ] && { want=\"$v\"; break; }\n    fi\n",
        ),
        _ => {}
    }
    let pre = match s.pre {
        ShimPre::None => String::new(),
        ShimPre::ComposerPhar => " \"$BZ_HOME/runtimes/composer/composer.phar\"".to_string(),
        ShimPre::Literal(a) => format!(" {a}"),
    };
    format!(
        r#"#!/bin/sh
# BOZKURT CODE shim for `{name}` ({kind}). Generated - do not edit.
BZ_HOME="${{BOZKURT_HOME:-{home}}}"
want="${{{env}:-}}"
if [ -z "$want" ]; then
  d="$PWD"
  while [ -n "$d" ]; do
{checks}    [ "$d" = "/" ] && break
    d=$(dirname "$d")
  done
fi
# reduce constraints like ^8.2, v20.11.1, >=18 to a numeric prefix
want=$(printf '%s' "$want" | sed -n 's/^[^0-9]*\([0-9][0-9]*\(\.[0-9][0-9]*\)\{{0,2\}}\).*/\1/p')
dir=""
base="$BZ_HOME/runtimes/{kind}"
if [ -n "$want" ]; then
  if [ -d "$base/$want" ]; then
    dir="$base/$want"
  else
    pat=$(printf '%s' "$want" | sed 's/\./\\./g')
    best=$(ls "$base" 2>/dev/null | grep -E "^$pat(\.|$)" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
    [ -n "$best" ] && dir="$base/$best"
  fi
fi
if [ -z "$dir" ] && [ -e "$BZ_HOME/current/{kind}" ]; then dir="$BZ_HOME/current/{kind}"; fi
if [ -z "$dir" ]; then
  echo "bozkurt: no {kind} version installed (run: bz runtime install {kind} <version>)" >&2
  exit 127
fi
PATH="{path_dir}:$PATH"
export PATH
exec "$dir/{rel}"{pre} "$@"
"#,
        name = s.name,
        home = sh_dq_escape(&home.to_string_lossy()),
    )
}

/// Body of a Windows `.cmd` shim. Handles `bozkurt.toml`, `.nvmrc`, `.node-version`,
/// `.python-version` and the `BOZKURT_<KIND>_VERSION` override (not composer.json/go.mod).
///
/// Argument safety (P1-10): the version lookup runs in an inner `setlocal
/// EnableDelayedExpansion` scope, but `%CD%`, `BOZKURT_HOME` and the result are carried
/// across with delayed expansion **off** (`endlocal & set "DIR=%DIR%"`), so `!` in
/// paths or arguments survives. `.exe` targets (php, composer, node, bun, python, go)
/// are started directly, without `call`, so `^` in quoted arguments
/// (`composer require "foo/bar:^1.0"`) is passed unchanged. `.cmd` targets (npm, npx,
/// corepack) still need `call`, which doubles `^` inside quoted arguments — a cmd.exe
/// limitation; a native shim executable would remove it.
pub fn cmd_shim(home: &Path, s: &ShimSpec) -> String {
    let kind = s.kind.as_str();
    let env = version_env(s.kind);
    let rel = s.win_rel.replace('/', "\\");
    let path_dir = match Path::new(s.win_rel)
        .parent()
        .map(|p| p.to_string_lossy().replace('/', "\\"))
    {
        Some(b) if !b.is_empty() => format!("!DIR!\\{b}"),
        _ => "!DIR!".to_string(),
    };
    let files: &[&str] = match s.kind {
        RuntimeKind::Node => &[".nvmrc", ".node-version"],
        RuntimeKind::Python => &[".python-version"],
        _ => &[],
    };
    let mut checks = String::new();
    for f in files {
        checks.push_str(&format!(
            "if exist \"!D!\\{f}\" ( set /p WANT=<\"!D!\\{f}\" & goto clean )\r\n"
        ));
    }
    let pre = match s.pre {
        ShimPre::None => String::new(),
        ShimPre::ComposerPhar => " \"%BZ_HOME%\\runtimes\\composer\\composer.phar\"".to_string(),
        ShimPre::Literal(a) => format!(" {a}"),
    };
    let is_cmd =
        rel.to_ascii_lowercase().ends_with(".cmd") || rel.to_ascii_lowercase().ends_with(".bat");
    let launch = if is_cmd {
        format!("call \"%DIR%\\{rel}\"{pre} %*")
    } else {
        format!("\"%DIR%\\{rel}\"{pre} %*")
    };
    // `%` is the only character that needs escaping in a quoted batch literal when
    // delayed expansion is off
    let home_lit = home.display().to_string().replace('%', "%%");
    let lines = [
        "@echo off".to_string(),
        format!("rem BOZKURT CODE shim for `{}` ({kind}). Generated - do not edit.", s.name),
        "setlocal EnableExtensions DisableDelayedExpansion".into(),
        format!("set \"BZ_HOME={home_lit}\""),
        "if defined BOZKURT_HOME set \"BZ_HOME=%BOZKURT_HOME%\"".into(),
        format!("set \"WANT=%{env}%\""),
        "set \"D=%CD%\"".into(),
        "set \"DIR=\"".into(),
        "setlocal EnableDelayedExpansion".into(),
        "if defined WANT goto clean".into(),
        ":walk".into(),
        "if exist \"!D!\\bozkurt.toml\" (".into(),
        format!("  for /f \"usebackq tokens=1,* delims==\" %%a in (`findstr /r /c:\"^ *{kind} *=\" \"!D!\\bozkurt.toml\"`) do set \"WANT=%%b\""),
        "  if defined WANT goto clean".into(),
        ")".into(),
        checks.trim_end().to_string(),
        "for %%P in (\"!D!\\..\") do set \"P=%%~fP\"".into(),
        "if /i \"!P!\"==\"!D!\" goto resolve".into(),
        "set \"D=!P!\"".into(),
        "goto walk".into(),
        ":clean".into(),
        "set \"WANT=!WANT:\"=!\"".into(),
        "set \"WANT=!WANT: =!\"".into(),
        "set \"WANT=!WANT:'=!\"".into(),
        "if /i \"!WANT:~0,1!\"==\"v\" set \"WANT=!WANT:~1!\"".into(),
        ":resolve".into(),
        format!("set \"BASE=!BZ_HOME!\\runtimes\\{kind}\""),
        "if defined WANT if exist \"!BASE!\\!WANT!\\\" set \"DIR=!BASE!\\!WANT!\"".into(),
        "if defined WANT if not defined DIR for /f \"delims=\" %%v in ('dir /b /ad /on \"!BASE!\\!WANT!.*\" 2^>nul') do set \"DIR=!BASE!\\%%v\"".into(),
        format!("if not defined DIR if exist \"!BZ_HOME!\\current\\{kind}\\\" set \"DIR=!BZ_HOME!\\current\\{kind}\""),
        format!("if not defined DIR ( echo bozkurt: no {kind} version installed ^(run: bz runtime install {kind} ^<version^>^) 1>&2 & exit /b 127 )"),
        format!("set \"PATH={path_dir};!PATH!\""),
        // leave the delayed-expansion scope; %DIR% / %PATH% are expanded before endlocal runs
        "endlocal & set \"DIR=%DIR%\" & set \"PATH=%PATH%\"".into(),
        launch,
        "exit /b %ERRORLEVEL%".into(),
    ];
    let mut out = lines
        .iter()
        .filter(|l| !l.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\r\n");
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn install_lock_is_per_version_and_blocks_uninstall() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure().unwrap();
        let client = download::http_client();
        let p = Platform::current();
        let m = RuntimeManager::new(
            paths.clone(),
            p,
            Catalog::new(paths.cache(), client.clone(), p),
            client,
            Arc::new(StateStore::open(&paths.state()).unwrap()),
            EventBus::default(),
        );
        let a = m.install_lock(RuntimeKind::Node, "22.1.0");
        assert!(Arc::ptr_eq(
            &a,
            &m.install_lock(RuntimeKind::Node, "22.1.0")
        ));
        assert!(!Arc::ptr_eq(
            &a,
            &m.install_lock(RuntimeKind::Node, "20.0.0")
        ));
        std::fs::create_dir_all(paths.runtime_dir(RuntimeKind::Node, "22.1.0")).unwrap();
        let held = a.lock().await;
        let e = m.uninstall(RuntimeKind::Node, "22.1.0").unwrap_err();
        assert!(matches!(e, CoreError::Conflict(_)), "{e}");
        drop(held);
        // a clone shares the locks
        let m2 = m.clone();
        let _g = m2
            .install_lock(RuntimeKind::Node, "22.1.0")
            .lock_owned()
            .await;
        assert!(m
            .install_lock(RuntimeKind::Node, "22.1.0")
            .try_lock()
            .is_err());
    }

    #[test]
    fn project_version_detection() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::write(root.join(".nvmrc"), "v20.11.1\n").unwrap();
        std::fs::write(
            root.join("composer.json"),
            r#"{"require":{"php":"^8.2","ext-json":"*"}}"#,
        )
        .unwrap();
        std::fs::write(root.join(".python-version"), "3.12\n").unwrap();
        std::fs::write(root.join("go.mod"), "module x\n\ngo 1.22.1\n").unwrap();
        let sub = root.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            project_version(RuntimeKind::Node, &sub).as_deref(),
            Some("20.11.1")
        );
        assert_eq!(
            project_version(RuntimeKind::Php, &sub).as_deref(),
            Some("8.2")
        );
        assert_eq!(
            project_version(RuntimeKind::Python, &sub).as_deref(),
            Some("3.12")
        );
        assert_eq!(
            project_version(RuntimeKind::Go, &sub).as_deref(),
            Some("1.22.1")
        );
        // bozkurt.toml in a nearer directory wins
        std::fs::write(
            root.join("a/bozkurt.toml"),
            "node = \"22\"\nphp = \"8.4\"\n",
        )
        .unwrap();
        assert_eq!(
            project_version(RuntimeKind::Node, &sub).as_deref(),
            Some("22")
        );
        assert_eq!(
            project_version(RuntimeKind::Php, &sub).as_deref(),
            Some("8.4")
        );
        // engines.node
        let e = root.join("eng");
        std::fs::create_dir_all(&e).unwrap();
        std::fs::write(e.join("package.json"), r#"{"engines":{"node":">=18"}}"#).unwrap();
        assert_eq!(version_in_dir(RuntimeKind::Node, &e).as_deref(), Some("18"));
    }

    #[test]
    fn cmd_shim_shape() {
        let s = SHIMS.iter().find(|s| s.name == "npm").unwrap();
        let b = cmd_shim(Path::new(r"C:\Users\u\AppData\Local\Bozkurt"), s);
        assert!(b.starts_with("@echo off\r\n"));
        assert!(b.contains("set \"BZ_HOME=C:\\Users\\u\\AppData\\Local\\Bozkurt\""));
        assert!(b.contains("call \"%DIR%\\npm.cmd\" %*"));
        assert!(b.contains(".nvmrc"));
        assert!(b.contains("BOZKURT_NODE_VERSION"));
        assert!(b.contains("setlocal EnableExtensions DisableDelayedExpansion"));
        assert!(b.contains("endlocal & set \"DIR=%DIR%\" & set \"PATH=%PATH%\""));
        // .exe targets are started without `call` (no caret doubling)
        let c = SHIMS.iter().find(|s| s.name == "composer").unwrap();
        let cb = cmd_shim(Path::new("C:\\b%x!y"), c);
        assert!(cb.contains(
            "\r\n\"%DIR%\\php.exe\" \"%BZ_HOME%\\runtimes\\composer\\composer.phar\" %*\r\n"
        ));
        assert!(!cb.contains("call "));
        // `%` in the home is escaped, `!` is safe with delayed expansion off
        assert!(cb.contains("set \"BZ_HOME=C:\\b%%x!y\""));
        // once delayed expansion is off again, no `!VAR!` may be used
        let after = cb.split("endlocal & ").nth(1).unwrap();
        assert!(!after.contains('!'), "{after}");
    }

    /// Runs the generated sh shim against a fake runtime tree.
    #[cfg(unix)]
    #[test]
    fn sh_shim_resolves_project_versions() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path().join("home"));
        paths.ensure().unwrap();
        write_shims(&paths, Platform::current()).unwrap();
        for v in ["20.11.1", "22.2.0", "22.10.0"] {
            let bin = paths.runtime_dir(RuntimeKind::Node, v).join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(
                bin.join("node"),
                format!("#!/bin/sh\necho node-{v} \"$@\"\n"),
            )
            .unwrap();
            fsutil::make_executable(&bin.join("node")).unwrap();
        }
        for v in ["8.2.10", "8.3.1"] {
            let d = paths.runtime_dir(RuntimeKind::Php, v);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("php"), format!("#!/bin/sh\necho php-{v} \"$@\"\n")).unwrap();
            fsutil::make_executable(&d.join("php")).unwrap();
        }
        fsutil::replace_dir_link(
            &paths.runtime_dir(RuntimeKind::Node, "20.11.1"),
            &paths.current_link(RuntimeKind::Node),
        )
        .unwrap();
        let run = |shim: &str, cwd: &Path, env: &[(&str, &str)]| -> String {
            let mut c = std::process::Command::new(paths.bin().join(shim));
            c.current_dir(cwd)
                .arg("-v")
                .env("BOZKURT_HOME", paths.home());
            c.env_remove("BOZKURT_NODE_VERSION")
                .env_remove("BOZKURT_PHP_VERSION");
            for (k, v) in env {
                c.env(k, v);
            }
            let o = c.output().unwrap();
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            )
        };
        let proj = t.path().join("proj/sub");
        std::fs::create_dir_all(&proj).unwrap();
        // no pin → global
        assert_eq!(run("node", &proj, &[]), "node-20.11.1 -v");
        // .nvmrc prefix → newest 22.x (numeric sort: 22.10 > 22.2)
        std::fs::write(t.path().join("proj/.nvmrc"), "v22\n").unwrap();
        assert_eq!(run("node", &proj, &[]), "node-22.10.0 -v");
        // bozkurt.toml in nearer dir wins
        std::fs::write(proj.join("bozkurt.toml"), "node = \"22.2\"\n").unwrap();
        assert_eq!(run("node", &proj, &[]), "node-22.2.0 -v");
        // env override wins
        assert_eq!(
            run("node", &proj, &[("BOZKURT_NODE_VERSION", "20")]),
            "node-20.11.1 -v"
        );
        // composer.json require.php
        std::fs::write(
            t.path().join("proj/composer.json"),
            "{\n  \"require\": {\n    \"php\": \"^8.2\",\n    \"laravel/framework\": \"^11.0\"\n  }\n}\n",
        )
        .unwrap();
        assert_eq!(run("php", &proj, &[]), "php-8.2.10 -v");
        // nothing installed for go → error message
        assert!(run("go", &proj, &[]).contains("no go version installed"));
    }
}
