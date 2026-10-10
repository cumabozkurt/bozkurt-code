//! The [`Engine`] facade: one object owning paths, config, state, catalogs, runtimes,
//! services, sites, the supervisor, jobs and the event bus. Every core-related endpoint
//! of `docs/API.md` maps to one async method here; the daemon only translates HTTP.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use crate::caddyfile::{self, CaddyInput};
use crate::catalogs::{Catalog, DiskCache, CACHE_TTL};
use crate::config::Config;
use crate::detection;
use crate::download::{self, http_client, install_tree};
use crate::error::{CoreError, Result};
use crate::events::{Event, EventBus};
use crate::jobs::JobManager;
use crate::models::*;
use crate::net::NetProvider;
use crate::paths::Paths;
use crate::platform::Platform;
use crate::ports;
use crate::runtimes::{version_env, RuntimeManager};
use crate::services::{
    self, caddy_spec, init_commands, php_pool_process_names, php_pool_spec, service_spec,
    ServiceCatalog, ServiceKind, ServiceLayout,
};
use crate::sites;
use crate::state::StateStore;
use crate::supervisor::{ProcessInfo, ProcessSpec, Supervisor};
use crate::version;

/// Version of bz-core (reported as the product version in `/health` and `/overview`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// First port tried for `app` sites.
pub const APP_PORT_BASE: u16 = 41000;

struct Inner {
    paths: Paths,
    platform: Platform,
    client: reqwest::Client,
    config: RwLock<Config>,
    state: Arc<StateStore>,
    events: EventBus,
    jobs: JobManager,
    supervisor: Supervisor,
    runtimes: RuntimeManager,
    svc_catalog: ServiceCatalog,
    net: Arc<dyn NetProvider>,
    /// Effective (http, https) ports Caddy was started with.
    caddy_ports: Mutex<Option<(u16, u16)>>,
    /// Admin API endpoint of the Caddy we started (P1-3).
    caddy_admin: Mutex<Option<caddyfile::AdminEndpoint>>,
    /// PHP `major.minor` → FastCGI upstream of the running pool.
    php_upstreams: Mutex<BTreeMap<String, String>>,
    /// site id → assigned PORT of a running app.
    app_ports: Mutex<HashMap<String, u16>>,
    /// Serialises Caddyfile writes/reloads.
    caddy_lock: tokio::sync::Mutex<()>,
}

/// Env var Vite reads to extend `server.allowedHosts` (comma separated, P1-16).
pub const VITE_ALLOWED_HOSTS_ENV: &str = "__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS";

/// The BOZKURT engine. Cheap to clone (shared state behind an `Arc`).
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("home", &self.inner.paths.home())
            .field("platform", &self.inner.platform)
            .finish()
    }
}

impl Engine {
    /// Open (creating if needed) the BOZKURT home at `home`. Must be called inside a
    /// tokio runtime (a background task forwards supervisor status to the event bus).
    pub async fn new(home: impl Into<PathBuf>, net: Arc<dyn NetProvider>) -> Result<Engine> {
        Engine::with_platform(home, net, Platform::current()).await
    }

    /// Like [`Engine::new`] with an explicit platform (asset selection only; tests).
    pub async fn with_platform(
        home: impl Into<PathBuf>,
        net: Arc<dyn NetProvider>,
        platform: Platform,
    ) -> Result<Engine> {
        let paths = Paths::new(home);
        paths.ensure()?;
        let config = Config::load(&paths.config())?;
        if !paths.config().exists() {
            config.save(&paths.config())?;
        }
        let state = Arc::new(StateStore::open(&paths.state())?);
        let events = EventBus::default();
        let client = http_client();
        let catalog = Catalog::new(paths.cache(), client.clone(), platform);
        let runtimes = RuntimeManager::new(
            paths.clone(),
            platform,
            catalog,
            client.clone(),
            state.clone(),
            events.clone(),
        );
        runtimes.write_shims()?;
        let svc_catalog = ServiceCatalog::new(
            client.clone(),
            DiskCache::new(paths.cache(), CACHE_TTL),
            platform,
        );
        let inner = Arc::new(Inner {
            supervisor: Supervisor::new(paths.logs(), events.clone()),
            jobs: JobManager::new(events.clone()),
            paths,
            platform,
            client,
            config: RwLock::new(config),
            state,
            events,
            runtimes,
            svc_catalog,
            net,
            caddy_ports: Mutex::new(None),
            caddy_admin: Mutex::new(None),
            php_upstreams: Mutex::new(BTreeMap::new()),
            app_ports: Mutex::new(HashMap::new()),
            caddy_lock: tokio::sync::Mutex::new(()),
        });
        let engine = Engine { inner };
        engine.spawn_status_forwarder();
        Ok(engine)
    }

    /// Kill children left over from a previous daemon that crashed or was killed (their
    /// pid and start time are in `run/children.json`), then start recording ours there
    /// (P1-7). Call once, after this process is known to own the home (the daemon calls it
    /// right after binding its API port) and before anything is started. Returns the
    /// names of the reaped processes.
    pub async fn reap_orphans(&self) -> Vec<String> {
        let file = self.inner.paths.children_file();
        let f = file.clone();
        let killed = tokio::task::spawn_blocking(move || crate::procguard::reap_orphans(&f))
            .await
            .unwrap_or_default();
        self.inner.supervisor.set_child_registry(file);
        killed
    }

    fn spawn_status_forwarder(&self) {
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let mut rx = self.inner.supervisor.subscribe_status();
        tokio::spawn(async move {
            loop {
                let (name, info) = match rx.recv().await {
                    Ok(v) => v,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                };
                let Some(inner) = weak.upgrade() else { return };
                let engine = Engine { inner };
                if let Some(id) = name.strip_prefix("site-") {
                    if let Some(site) = engine.site_opt(id) {
                        engine.inner.events.emit(Event::SiteChanged { site });
                    }
                } else if let Ok(kind) = ServiceKind::parse(&name) {
                    let service = engine.service_from_info(&kind, Some(&info)).await;
                    engine.inner.events.emit(Event::ServiceStatus { service });
                }
            }
        });
    }

    // ----------------------------------------------------------------------------
    // accessors
    // ----------------------------------------------------------------------------

    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }
    pub fn platform(&self) -> Platform {
        self.inner.platform
    }
    pub fn events(&self) -> &EventBus {
        &self.inner.events
    }
    /// Subscribe to the event stream (`GET /events`).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }
    pub fn jobs_manager(&self) -> &JobManager {
        &self.inner.jobs
    }
    pub fn supervisor(&self) -> &Supervisor {
        &self.inner.supervisor
    }
    pub fn runtime_manager(&self) -> &RuntimeManager {
        &self.inner.runtimes
    }
    pub fn net(&self) -> &Arc<dyn NetProvider> {
        &self.inner.net
    }
    /// Snapshot of the current config.
    pub fn config(&self) -> Config {
        self.inner.config.read().expect("config").clone()
    }

    fn tld(&self) -> String {
        self.config().tld
    }

    /// Admin endpoint of our Caddy: the one it was started with, else where the next start
    /// would put it (a unix socket; on Windows a placeholder rewritten at start).
    fn caddy_admin(&self) -> caddyfile::AdminEndpoint {
        if let Some(a) = self.inner.caddy_admin.lock().expect("admin").clone() {
            return a;
        }
        #[cfg(unix)]
        {
            let sock = self.inner.paths.caddy_run().join("admin.sock");
            if sock.as_os_str().len() < 100 {
                return caddyfile::AdminEndpoint::Unix(sock);
            }
        }
        caddyfile::AdminEndpoint::Tcp(0)
    }

    /// Effective (http, https) ports: what Caddy runs with, else the configured ones.
    pub fn web_ports(&self) -> (u16, u16) {
        self.inner
            .caddy_ports
            .lock()
            .expect("ports")
            .unwrap_or_else(|| {
                let c = self.config();
                (c.http_port, c.https_port)
            })
    }

    // ----------------------------------------------------------------------------
    // health / overview
    // ----------------------------------------------------------------------------

    /// `GET /health`.
    pub fn health(&self) -> Health {
        Health {
            ok: true,
            version: VERSION.to_string(),
        }
    }

    /// `GET /overview`.
    pub async fn overview(&self) -> Result<Overview> {
        let cfg = self.config();
        let ca_trusted = self
            .inner
            .net
            .ca_status()
            .await
            .map(|s| s.ca_trusted)
            .unwrap_or(false);
        let dns_configured = self
            .inner
            .net
            .dns_status(&cfg.tld)
            .await
            .map(|s| s.configured)
            .unwrap_or(false);
        let (http_port, https_port) = self.web_ports();
        Ok(Overview {
            version: VERSION.to_string(),
            os: self.inner.platform.os_str().to_string(),
            arch: self.inner.platform.arch_str().to_string(),
            home: self.inner.paths.home().to_string_lossy().into_owned(),
            tld: cfg.tld,
            runtimes: self.runtimes(),
            services: self.services().await,
            sites_count: self.inner.state.read(|s| s.sites.len()),
            ca_trusted,
            dns_configured,
            http_port,
            https_port,
        })
    }

    // ----------------------------------------------------------------------------
    // runtimes
    // ----------------------------------------------------------------------------

    /// `GET /runtimes`.
    pub fn runtimes(&self) -> Vec<Runtime> {
        self.inner.runtimes.all()
    }

    /// Download with the P1-1 policy: a published checksum is enforced; without one, a
    /// sha256 pinned in `state.json` is enforced, and an unpinned asset is refused unless
    /// the `allow_unverified` config key is set (then it is pinned).
    pub async fn download_verified(
        &self,
        client: &reqwest::Client,
        url: &str,
        dest: &std::path::Path,
        checksum: &download::Checksum,
        progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<String> {
        download::download_tofu(
            client,
            &self.inner.state,
            url,
            dest,
            checksum,
            self.config().allow_unverified,
            progress,
        )
        .await
        .map(|(d, _)| d)
    }

    /// `GET /runtimes/{kind}/versions?refresh=`.
    pub async fn runtime_versions(&self, kind: RuntimeKind, refresh: bool) -> Result<Vec<Version>> {
        self.inner.runtimes.versions(kind, refresh).await
    }

    /// `POST /runtimes/{kind}/install` → job.
    pub fn install_runtime(&self, kind: RuntimeKind, version: &str) -> JobAccepted {
        self.install_runtime_with(kind, version, false)
    }

    /// [`Engine::install_runtime`]; `allow_unverified` (or the `allow_unverified` config
    /// key) accepts an asset without a published checksum once and pins its sha256 (P1-1).
    pub fn install_runtime_with(
        &self,
        kind: RuntimeKind,
        version: &str,
        allow_unverified: bool,
    ) -> JobAccepted {
        let e = self.clone();
        let want = version.to_string();
        let allow = allow_unverified || self.config().allow_unverified;
        let id = self
            .inner
            .jobs
            .spawn(format!("Install {kind} {want}"), move |h| async move {
                let v = e
                    .inner
                    .runtimes
                    .install(kind, &want, Some(&h), allow)
                    .await?;
                if kind == RuntimeKind::Php && e.caddy_active() {
                    if let Err(err) = e.apply_caddy().await {
                        tracing::warn!("caddy apply after php install: {err}");
                    }
                }
                Ok(Some(format!("Installed {kind} {v}")))
            });
        JobAccepted { job_id: id }
    }

    /// `DELETE /runtimes/{kind}/{version}`.
    pub async fn uninstall_runtime(&self, kind: RuntimeKind, version: &str) -> Result<()> {
        if kind == RuntimeKind::Php {
            let mm = version::major_minor(version);
            let same_minor_left = self
                .inner
                .runtimes
                .installed(kind)
                .iter()
                .filter(|v| v.as_str() != version)
                .any(|v| version::major_minor(v) == mm);
            if !same_minor_left {
                self.stop_pool(&mm).await;
            }
        }
        self.inner.runtimes.uninstall(kind, version)
    }

    /// `POST /runtimes/{kind}/use`.
    pub fn use_runtime(&self, kind: RuntimeKind, version: &str) -> Result<Runtime> {
        self.inner.runtimes.use_version(kind, version)
    }

    // ----------------------------------------------------------------------------
    // services
    // ----------------------------------------------------------------------------

    fn service_kinds(&self) -> Vec<ServiceKind> {
        let mut v: Vec<ServiceKind> = ServiceKind::FIXED.to_vec();
        let mut mms: Vec<String> = self
            .inner
            .runtimes
            .installed(RuntimeKind::Php)
            .iter()
            .map(|v| version::major_minor(v))
            .collect();
        mms.dedup();
        v.extend(mms.into_iter().map(ServiceKind::PhpFpm));
        v
    }

    fn service_port(&self, kind: &ServiceKind) -> Option<u16> {
        match kind {
            ServiceKind::Caddy => Some(self.web_ports().1),
            ServiceKind::Dns => Some(self.config().dns_port),
            _ => self
                .inner
                .state
                .read(|s| s.service(&kind.name()).port)
                .or(kind.default_port()),
        }
    }

    fn autostart(&self, kind: &ServiceKind) -> bool {
        self.config().autostart.contains(&kind.name())
    }

    fn caddy_active(&self) -> bool {
        self.inner.supervisor.is_active("caddy")
    }

    async fn service_from_info(&self, kind: &ServiceKind, info: Option<&ProcessInfo>) -> Service {
        let name = kind.name();
        let p = self.inner.platform;
        let port = self.service_port(kind);
        let mut svc = Service {
            name: name.clone(),
            status: ServiceStatus::Stopped,
            pid: None,
            port,
            version: None,
            autostart: self.autostart(kind),
            uptime_secs: None,
            message: None,
        };
        match kind {
            ServiceKind::Dns => {
                match self.inner.net.dns_status(&self.tld()).await {
                    Ok(d) => {
                        svc.status = if d.running {
                            ServiceStatus::Running
                        } else {
                            ServiceStatus::Stopped
                        };
                        if !d.configured {
                            svc.message =
                                Some(format!("OS resolver for .{} is not configured yet", d.tld));
                        }
                    }
                    Err(e) => svc.message = Some(e.to_string()),
                }
                return svc;
            }
            ServiceKind::PhpFpm(mm) => {
                svc.version = self.inner.runtimes.find_installed(RuntimeKind::Php, mm);
                if svc.version.is_none() {
                    svc.status = ServiceStatus::NotInstalled;
                    return svc;
                }
                if !p.is_windows() {
                    svc.port = None;
                    svc.message = Some(format!(
                        "socket {}",
                        ServiceLayout::new(&self.inner.paths, kind)
                            .fpm_socket()
                            .display()
                    ));
                }
            }
            _ => {
                svc.version = self.inner.state.read(|s| s.service(&name).version);
                let layout = ServiceLayout::new(&self.inner.paths, kind);
                if !layout.installed(p) {
                    svc.status = ServiceStatus::NotInstalled;
                    if kind.may_be_external() {
                        if let Some(port) = port {
                            if ports::is_listening(port).await {
                                svc.status = ServiceStatus::External;
                                svc.message = Some(self.external_message(port).await);
                            }
                        }
                    }
                    return svc;
                }
                if *kind == ServiceKind::Mailpit {
                    svc.message = Some(format!(
                        "web UI http://127.0.0.1:{}",
                        services::MAILPIT_UI_PORT
                    ));
                }
            }
        }
        let info = info.cloned().or_else(|| self.inner.supervisor.info(&name));
        if let Some(i) = info {
            svc.status = i.status;
            svc.pid = i.pid;
            svc.uptime_secs = i.uptime_secs;
            if matches!(i.status, ServiceStatus::Crashed) {
                svc.message = i.last_exit.clone().map(|e| format!("last exit: {e}"));
            }
        }
        if svc.status == ServiceStatus::Stopped && kind.may_be_external() {
            if let Some(port) = port {
                if ports::is_listening(port).await {
                    svc.status = ServiceStatus::External;
                    svc.message = Some(self.external_message(port).await);
                }
            }
        }
        svc
    }

    async fn external_message(&self, port: u16) -> String {
        match self.inner.net.port_owner(port).await {
            Ok(PortOwner {
                process: Some(p),
                pid,
                ..
            }) => format!(
                "port {port} is served by an existing {p}{}; it is used as is",
                pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
            ),
            _ => format!("port {port} is served by an existing process; it is used as is"),
        }
    }

    /// `GET /services`.
    pub async fn services(&self) -> Vec<Service> {
        let mut out = vec![];
        for k in self.service_kinds() {
            out.push(self.service_from_info(&k, None).await);
        }
        out
    }

    /// One service by API name.
    pub async fn service(&self, name: &str) -> Result<Service> {
        let kind = ServiceKind::parse(name)?;
        Ok(self.service_from_info(&kind, None).await)
    }

    /// `POST /services/{name}/install` → job.
    pub fn install_service(&self, name: &str, version: Option<String>) -> Result<JobAccepted> {
        let kind = ServiceKind::parse(name)?;
        let e = self.clone();
        let title = format!(
            "Install {}{}",
            kind,
            version
                .as_deref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default()
        );
        let id = self.inner.jobs.spawn(title, move |h| async move {
            let msg = e
                .install_service_now(&kind, version.as_deref(), Some(&h))
                .await?;
            Ok(Some(msg))
        });
        Ok(JobAccepted { job_id: id })
    }

    /// Install synchronously (used by the job; public for CLIs/tests).
    pub async fn install_service_now(
        &self,
        kind: &ServiceKind,
        want: Option<&str>,
        job: Option<&crate::jobs::JobHandle>,
    ) -> Result<String> {
        match kind {
            ServiceKind::Dns => return Ok("dns is built in".into()),
            ServiceKind::PhpFpm(mm) => {
                let v = self
                    .inner
                    .runtimes
                    .install(
                        RuntimeKind::Php,
                        want.unwrap_or(mm),
                        job,
                        self.config().allow_unverified,
                    )
                    .await?;
                return Ok(format!("Installed PHP {v}"));
            }
            _ => {}
        }
        let rel = self.inner.svc_catalog.resolve(kind, want).await?;
        let layout = ServiceLayout::new(&self.inner.paths, kind);
        let tmp = self
            .inner
            .paths
            .tmp()
            .join(format!("svc-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp)?;
        let file = tmp.join(&rel.asset.filename);
        if let Some(j) = job {
            j.progress(0.0, Some(&format!("Downloading {}", rel.asset.filename)));
        }
        let jj = job.cloned();
        let res = self
            .download_verified(
                &self.inner.client,
                &rel.asset.url,
                &file,
                &rel.asset.checksum,
                move |d, t| {
                    if let (Some(j), Some(t)) = (&jj, t) {
                        if t > 0 {
                            j.progress(0.85 * d as f64 / t as f64, None);
                        }
                    }
                },
            )
            .await;
        if let Err(e) = res {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        let was_running = self.inner.supervisor.is_active(&kind.process_name());
        if was_running {
            self.inner.supervisor.stop(&kind.process_name()).await?;
        }
        if let Some(j) = job {
            j.progress(0.88, Some("Extracting"));
        }
        let dist = layout.dist.clone();
        let p = self.inner.platform;
        let bin = layout.binary(p);
        let res = tokio::task::spawn_blocking(move || {
            install_tree(&[file], &dist, true, true, |_| Ok(()))?;
            if bin.exists() {
                crate::fsutil::make_executable(&bin)?;
            }
            Ok::<_, CoreError>(())
        })
        .await
        .map_err(|e| CoreError::Other(anyhow::anyhow!("extract task: {e}")))?;
        let _ = std::fs::remove_dir_all(&tmp);
        res?;
        #[cfg(unix)]
        if layout.dist.join("bin").is_dir() {
            for e in std::fs::read_dir(layout.dist.join("bin"))?.flatten() {
                let _ = crate::fsutil::make_executable(&e.path());
            }
        }
        std::fs::create_dir_all(&layout.data)?;
        self.inner.state.update(|s| {
            s.service_mut(&kind.name()).version = Some(rel.version.clone());
            Ok(())
        })?;
        if was_running {
            self.start_service(kind).await?;
        } else if *kind == ServiceKind::Caddy && self.autostart(kind) {
            // Caddy is in `autostart` (the default) but could not start before it was
            // installed: bring it up now instead of waiting for the next daemon start.
            if let Err(e) = self.start_service(kind).await {
                tracing::warn!("starting caddy after install: {e}");
                return Ok(format!(
                    "Installed {kind} {} (could not start it: {e})",
                    rel.version
                ));
            }
            return Ok(format!("Installed and started {kind} {}", rel.version));
        }
        Ok(format!("Installed {kind} {}", rel.version))
    }

    /// `POST /services/{name}/{action}`.
    pub async fn service_action(&self, name: &str, action: ServiceAction) -> Result<Service> {
        let kind = ServiceKind::parse(name)?;
        match action {
            ServiceAction::Start => self.start_service(&kind).await,
            ServiceAction::Stop => self.stop_service(&kind).await,
            ServiceAction::Restart => {
                self.stop_service(&kind).await?;
                self.start_service(&kind).await
            }
        }
    }

    /// `POST /services/all/{action}` (installed services only; `restart` restarts
    /// running ones, `start` starts every installed one).
    pub async fn all_services_action(&self, action: ServiceAction) -> Result<Vec<Service>> {
        let mut out = vec![];
        for kind in self.service_kinds() {
            let current = self.service_from_info(&kind, None).await;
            let skip = match action {
                ServiceAction::Start => matches!(
                    current.status,
                    ServiceStatus::NotInstalled | ServiceStatus::External
                ),
                ServiceAction::Stop => !matches!(
                    current.status,
                    ServiceStatus::Running | ServiceStatus::Starting | ServiceStatus::Crashed
                ),
                ServiceAction::Restart => current.status != ServiceStatus::Running,
            };
            if skip {
                out.push(current);
                continue;
            }
            match self.service_action(&kind.name(), action).await {
                Ok(s) => out.push(s),
                Err(e) => {
                    let mut s = self.service_from_info(&kind, None).await;
                    s.message = Some(e.to_string());
                    out.push(s);
                }
            }
        }
        Ok(out)
    }

    /// Start one service (idempotent).
    pub async fn start_service(&self, kind: &ServiceKind) -> Result<Service> {
        let p = self.inner.platform;
        let name = kind.process_name();
        if self.inner.supervisor.is_active(&name) {
            return Ok(self.service_from_info(kind, None).await);
        }
        match kind {
            ServiceKind::Dns => {
                let c = self.config();
                self.inner.net.start_dns(c.dns_port, &c.tld).await?;
            }
            ServiceKind::Caddy => {
                let layout = ServiceLayout::new(&self.inner.paths, kind);
                if !layout.installed(p) {
                    return Err(CoreError::NotFound(
                        "caddy is not installed (POST /services/caddy/install)".into(),
                    ));
                }
                let c = self.config();
                let pick = |pref: u16, fallback: u16| {
                    if ports::can_bind_any(pref) {
                        pref
                    } else {
                        tracing::warn!("port {pref} unavailable; using {fallback}");
                        fallback
                    }
                };
                let http = pick(
                    c.http_port,
                    if c.http_port == 80 { 8080 } else { c.http_port },
                );
                let https = pick(
                    c.https_port,
                    if c.https_port == 443 {
                        8443
                    } else {
                        c.https_port
                    },
                );
                *self.inner.caddy_ports.lock().expect("ports") = Some((http, https));
                let admin = caddyfile::AdminEndpoint::choose(&self.inner.paths)?;
                admin.prepare()?;
                *self.inner.caddy_admin.lock().expect("admin") = Some(admin.clone());
                self.write_caddyfile(true).await?;
                let mut spec = caddy_spec(&self.inner.paths, p);
                if p.is_windows() {
                    // no SIGTERM on Windows: ask Caddy itself to stop (P1-8)
                    spec.graceful_stop = Some(crate::supervisor::GracefulStop::HttpPost(format!(
                        "{}/stop",
                        admin.base_url()
                    )));
                }
                self.inner.supervisor.start(spec).await?;
                if !admin.wait_ready(Duration::from_secs(10)).await {
                    tracing::warn!(
                        "caddy admin endpoint {} did not come up within 10s",
                        admin.address()
                    );
                }
            }
            ServiceKind::PhpFpm(mm) => {
                self.start_pool(mm).await?;
            }
            _ => {
                let layout = ServiceLayout::new(&self.inner.paths, kind);
                let port = self.service_port(kind).unwrap_or(0);
                if !layout.installed(p) {
                    if kind.may_be_external() && ports::is_listening(port).await {
                        return Ok(self.service_from_info(kind, None).await);
                    }
                    return Err(CoreError::NotFound(format!(
                        "{kind} is not installed (POST /services/{kind}/install)"
                    )));
                }
                if ports::is_listening(port).await {
                    // someone else serves this port → external, never grab it
                    return Ok(self.service_from_info(kind, None).await);
                }
                self.init_service(kind, &layout, port).await?;
                let spec = service_spec(&layout, p, port, self.inner.paths.log_file(&kind.name()))?;
                self.inner.supervisor.start(spec).await?;
                let _ = ports::wait_listening(port, Duration::from_secs(15)).await;
            }
        }
        Ok(self.service_from_info(kind, None).await)
    }

    async fn init_service(
        &self,
        kind: &ServiceKind,
        layout: &ServiceLayout,
        port: u16,
    ) -> Result<()> {
        let cmds = init_commands(layout, self.inner.platform, port);
        if cmds.is_empty() {
            return Ok(());
        }
        let initialized = self
            .inner
            .state
            .read(|s| s.service(&kind.name()).initialized);
        let data_has_files = std::fs::read_dir(&layout.data)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        if initialized && data_has_files {
            return Ok(());
        }
        if data_has_files {
            // initialised outside of our bookkeeping (e.g. state reset): trust it
        } else {
            std::fs::create_dir_all(&layout.data)?;
            for (prog, args) in cmds {
                let out = tokio::process::Command::new(&prog)
                    .args(&args)
                    .current_dir(&layout.root)
                    .output()
                    .await
                    .map_err(|e| CoreError::Other(anyhow::anyhow!("{}: {e}", prog.display())))?;
                let log = self.inner.paths.log_file(&kind.name());
                let _ = crate::supervisor::append_rotating(&log, &out.stdout);
                let _ = crate::supervisor::append_rotating(&log, &out.stderr);
                if !out.status.success() {
                    return Err(CoreError::Other(anyhow::anyhow!(
                        "initialising {kind} failed ({}): {}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
            }
        }
        self.inner.state.update(|s| {
            s.service_mut(&kind.name()).initialized = true;
            Ok(())
        })
    }

    /// Start the PHP pool for `mm` (if not running) and return its FastCGI upstream.
    async fn start_pool(&self, mm: &str) -> Result<String> {
        let all_active = php_pool_process_names(mm, self.inner.platform)
            .iter()
            .all(|n| self.inner.supervisor.is_active(n));
        if all_active {
            if let Some(u) = self.inner.php_upstreams.lock().expect("php").get(mm) {
                return Ok(u.clone());
            }
        }
        let v = self
            .inner
            .runtimes
            .find_installed(RuntimeKind::Php, mm)
            .ok_or_else(|| CoreError::NotFound(format!("PHP {mm} is not installed")))?;
        let dir = self.inner.paths.runtime_dir(RuntimeKind::Php, &v);
        let (specs, upstream) = php_pool_spec(&self.inner.paths, self.inner.platform, mm, &dir)?;
        // Windows: several php-cgi processes (P1-13); start the ones not running yet
        for spec in specs {
            if !self.inner.supervisor.is_active(&spec.name) {
                if let Err(e) = self.inner.supervisor.start(spec).await {
                    self.stop_pool(mm).await;
                    return Err(e);
                }
            }
        }
        self.inner
            .php_upstreams
            .lock()
            .expect("php")
            .insert(mm.to_string(), upstream.clone());
        Ok(upstream)
    }

    /// Stop every process of PHP pool `mm` (one php-fpm, or the Windows php-cgi set).
    async fn stop_pool(&self, mm: &str) {
        let names = php_pool_process_names(mm, self.inner.platform);
        futures_util::future::join_all(names.iter().map(|n| self.inner.supervisor.stop(n))).await;
        self.inner.php_upstreams.lock().expect("php").remove(mm);
    }

    /// Stop one service.
    pub async fn stop_service(&self, kind: &ServiceKind) -> Result<Service> {
        match kind {
            ServiceKind::Dns => self.inner.net.stop_dns().await?,
            ServiceKind::PhpFpm(mm) => self.stop_pool(mm).await,
            _ => {
                self.inner.supervisor.stop(&kind.process_name()).await?;
                if *kind == ServiceKind::Caddy {
                    *self.inner.caddy_ports.lock().expect("ports") = None;
                }
            }
        }
        Ok(self.service_from_info(kind, None).await)
    }

    /// `PATCH /services/{name}`.
    pub async fn patch_service(&self, name: &str, patch: &ServicePatch) -> Result<Service> {
        let kind = ServiceKind::parse(name)?;
        if let Some(a) = patch.autostart {
            let mut cfg = self.config();
            cfg.autostart.retain(|n| n != &kind.name());
            if a {
                cfg.autostart.push(kind.name());
            }
            self.save_config(cfg)?;
        }
        if let Some(port) = patch.port {
            match kind {
                ServiceKind::Caddy => {
                    let mut cfg = self.config();
                    cfg.https_port = port;
                    self.save_config(cfg)?;
                }
                ServiceKind::Dns => {
                    let mut cfg = self.config();
                    cfg.dns_port = port;
                    self.save_config(cfg)?;
                }
                ServiceKind::PhpFpm(_) => {
                    return Err(CoreError::InvalidInput(
                        "php-fpm pools use fixed sockets/ports".into(),
                    ))
                }
                _ => self.inner.state.update(|s| {
                    s.service_mut(&kind.name()).port = Some(port);
                    Ok(())
                })?,
            }
        }
        Ok(self.service_from_info(&kind, None).await)
    }

    /// `GET /services/{name}/logs?lines=`.
    pub fn service_logs(&self, name: &str, lines: usize) -> Result<LogLines> {
        let kind = ServiceKind::parse(name)?;
        let n = kind.process_name();
        let mut l = self.inner.supervisor.logs(&n, lines);
        if l.is_empty() {
            l = crate::fsutil::tail_lines(&self.inner.paths.log_file(&n), lines)?;
        }
        Ok(LogLines { lines: l })
    }

    /// Start every service listed in `config.autostart` (errors are logged).
    pub async fn start_autostart(&self) -> Vec<Service> {
        let mut out = vec![];
        for name in self.config().autostart {
            let Ok(kind) = ServiceKind::parse(&name) else {
                continue;
            };
            match self.start_service(&kind).await {
                Ok(s) => out.push(s),
                Err(e) => tracing::warn!("autostart {name}: {e}"),
            }
        }
        out
    }

    /// Stop everything (daemon shutdown).
    pub async fn shutdown(&self) {
        self.inner.supervisor.stop_all().await;
        let _ = self.inner.net.stop_dns().await;
    }

    // ----------------------------------------------------------------------------
    // Caddy
    // ----------------------------------------------------------------------------

    /// Regenerate the Caddyfile and hot-reload Caddy if it is running.
    pub async fn apply_caddy(&self) -> Result<()> {
        self.write_caddyfile(false).await
    }

    async fn write_caddyfile(&self, starting: bool) -> Result<()> {
        let _guard = self.inner.caddy_lock.lock().await;
        let sites = self.sites();
        let running = self.caddy_active();
        let start_pools = running || starting;
        // PHP upstreams
        let mut upstreams = BTreeMap::new();
        for s in sites.iter().filter(|s| s.kind == SiteKind::Php) {
            let Some(v) = self.inner.runtimes.resolve_for_dir(
                RuntimeKind::Php,
                Path::new(&s.path),
                s.php.as_deref(),
            ) else {
                continue;
            };
            let mm = version::major_minor(&v);
            let up = if start_pools {
                match self.start_pool(&mm).await {
                    Ok(u) => Some(u),
                    Err(e) => {
                        tracing::warn!("php pool {mm} for {}: {e}", s.domain);
                        None
                    }
                }
            } else {
                self.inner
                    .php_upstreams
                    .lock()
                    .expect("php")
                    .get(&mm)
                    .cloned()
            };
            if let Some(u) = up {
                upstreams.insert(s.id.clone(), u);
            }
        }
        // certificates
        let mut certs = BTreeMap::new();
        for s in sites.iter().filter(|s| s.https) {
            for g in caddyfile::cert_groups(s) {
                match self.inner.net.ensure_cert(&g.primary, g.wildcard).await {
                    Ok((c, k)) if c.exists() && k.exists() => {
                        certs.insert(g.primary.clone(), (c, k));
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("certificate for {}: {e}", g.primary),
                }
            }
        }
        let mut snippets = BTreeMap::new();
        for s in &sites {
            let p = self.inner.paths.caddy_snippet(&s.domain);
            if p.exists() {
                snippets.insert(s.id.clone(), p);
            }
        }
        let (http_port, https_port) = self.web_ports();
        let admin = self.caddy_admin();
        let admin_addr = admin.address();
        let text = caddyfile::generate(&CaddyInput {
            sites: &sites,
            http_port,
            https_port,
            admin: &admin_addr,
            logs_dir: self.inner.paths.logs(),
            certs,
            php_upstreams: upstreams,
            snippets,
            lan: self.config().lan_access,
        });
        crate::fsutil::atomic_write(&self.inner.paths.caddyfile(), text.as_bytes())?;
        if running {
            if let Err(e) = caddyfile::reload_via_admin(&admin, &text).await {
                tracing::warn!("admin reload failed ({e}); trying `caddy reload`");
                let bin = ServiceLayout::new(&self.inner.paths, &ServiceKind::Caddy)
                    .binary(self.inner.platform);
                caddyfile::reload_via_cli(&bin, &self.inner.paths.caddyfile(), &admin_addr).await?;
            }
        }
        Ok(())
    }

    async fn apply_caddy_logged(&self) {
        if let Err(e) = self.apply_caddy().await {
            tracing::warn!("caddy apply: {e}");
        }
    }

    // ----------------------------------------------------------------------------
    // sites
    // ----------------------------------------------------------------------------

    fn decorate(&self, mut s: Site) -> Site {
        if s.kind == SiteKind::App {
            let info = self.inner.supervisor.info(&sites::process_name(&s.id));
            s.app_status = Some(
                info.as_ref()
                    .map(|i| i.status)
                    .unwrap_or(ServiceStatus::Stopped),
            );
            s.app_port = if self.inner.supervisor.is_active(&sites::process_name(&s.id)) {
                self.inner
                    .app_ports
                    .lock()
                    .expect("apps")
                    .get(&s.id)
                    .copied()
            } else {
                None
            };
        }
        s
    }

    fn site_opt(&self, id: &str) -> Option<Site> {
        self.inner
            .state
            .read(|st| st.site(id).cloned())
            .map(|s| self.decorate(s))
    }

    /// `GET /sites`.
    pub fn sites(&self) -> Vec<Site> {
        let list = self.inner.state.read(|s| s.sites.clone());
        list.into_iter().map(|s| self.decorate(s)).collect()
    }

    /// `GET /sites/{id}`.
    pub fn site(&self, id: &str) -> Result<Site> {
        self.site_opt(id)
            .ok_or_else(|| CoreError::NotFound(format!("site `{id}`")))
    }

    /// `POST /sites` (missing fields filled by `bozkurt.toml` + detection).
    pub async fn create_site(&self, req: &CreateSiteRequest) -> Result<Site> {
        let tld = self.tld();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let site = self.inner.state.update(|st| {
            let s = sites::build_site(req, &tld, &st.sites, &now)?;
            st.sites.push(s.clone());
            Ok(s)
        })?;
        let site = self.decorate(site);
        self.inner
            .events
            .emit(Event::SiteChanged { site: site.clone() });
        self.apply_caddy_logged().await;
        Ok(site)
    }

    /// `PATCH /sites/{id}`.
    pub async fn update_site(&self, id: &str, patch: &SitePatch) -> Result<Site> {
        let tld = self.tld();
        let site = self.inner.state.update(|st| {
            let cur = st
                .site(id)
                .cloned()
                .ok_or_else(|| CoreError::NotFound(format!("site `{id}`")))?;
            let next = sites::apply_patch(&cur, patch, &tld, &st.sites)?;
            *st.site_mut(id).expect("exists") = next.clone();
            Ok(next)
        })?;
        let pname = sites::process_name(id);
        if self.inner.supervisor.is_active(&pname) {
            self.inner.supervisor.stop(&pname).await?;
            if site.kind == SiteKind::App {
                self.start_app(&site).await?;
            }
        }
        self.apply_caddy_logged().await;
        let site = self.site(id)?;
        self.inner
            .events
            .emit(Event::SiteChanged { site: site.clone() });
        Ok(site)
    }

    /// `DELETE /sites/{id}` (project files are never touched).
    pub async fn delete_site(&self, id: &str) -> Result<()> {
        self.site(id)?;
        self.inner.supervisor.stop(&sites::process_name(id)).await?;
        self.inner.app_ports.lock().expect("apps").remove(id);
        self.inner.state.update(|st| {
            st.sites.retain(|s| s.id != id);
            Ok(())
        })?;
        self.inner
            .events
            .emit(Event::SiteRemoved { id: id.to_string() });
        self.apply_caddy_logged().await;
        Ok(())
    }

    /// `POST /sites/{id}/{action}`.
    pub async fn site_action(&self, id: &str, action: SiteAction) -> Result<Site> {
        let site = self.site(id)?;
        match action {
            SiteAction::Secure | SiteAction::Unsecure => {
                return self
                    .update_site(
                        id,
                        &SitePatch {
                            https: Some(action == SiteAction::Secure),
                            ..Default::default()
                        },
                    )
                    .await;
            }
            _ if site.kind != SiteKind::App => {
                return Err(CoreError::InvalidInput(format!(
                    "start/stop/restart only apply to app sites (`{id}` is {})",
                    site.kind.as_str()
                )));
            }
            SiteAction::Start => {
                if !self.inner.supervisor.is_active(&sites::process_name(id)) {
                    self.start_app(&site).await?;
                }
            }
            SiteAction::Stop => {
                self.inner.supervisor.stop(&sites::process_name(id)).await?;
                self.inner.app_ports.lock().expect("apps").remove(id);
            }
            SiteAction::Restart => {
                self.inner.supervisor.stop(&sites::process_name(id)).await?;
                self.start_app(&site).await?;
            }
        }
        self.apply_caddy_logged().await;
        let site = self.site(id)?;
        self.inner
            .events
            .emit(Event::SiteChanged { site: site.clone() });
        Ok(site)
    }

    /// Environment for running a site's command: site env, `PORT`, PATH with the
    /// resolved runtime versions first, and `BOZKURT_<KIND>_VERSION` pins for the shims.
    pub fn app_env(&self, site: &Site, port: u16) -> BTreeMap<String, String> {
        let dir = Path::new(&site.path);
        let mut env = site.env.clone();
        env.insert("PORT".into(), port.to_string());
        env.insert("HOST".into(), "127.0.0.1".into());
        // Vite >= 6.0.9 / 5.4.12 / 4.5.6 answers 403 "Blocked request. This host is not
        // allowed" for any Host but localhost/IPs; Caddy forwards the site's Host (P1-16).
        // A leading dot allows the name and every subdomain. A value from the site's own
        // env is kept and extended.
        let mut hosts: Vec<String> = env
            .get(VITE_ALLOWED_HOSTS_ENV)
            .map(|v| {
                v.split(',')
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        for name in std::iter::once(&site.domain).chain(&site.aliases) {
            let name = name.trim_start_matches("*.").trim_start_matches('.');
            if name.is_empty() {
                continue;
            }
            let h = format!(".{name}");
            if !hosts.contains(&h) {
                hosts.push(h);
            }
        }
        if !hosts.is_empty() {
            env.insert(VITE_ALLOWED_HOSTS_ENV.into(), hosts.join(","));
        }
        env.insert("BOZKURT_SITE".into(), site.id.clone());
        env.insert(
            "BOZKURT_HOME".into(),
            self.inner.paths.home().to_string_lossy().into_owned(),
        );
        let mut path_parts: Vec<PathBuf> = vec![];
        for kind in RuntimeKind::ALL {
            let pin = match kind {
                RuntimeKind::Node => site.node.as_deref(),
                RuntimeKind::Php => site.php.as_deref(),
                _ => None,
            };
            if let Some(v) = self.inner.runtimes.resolve_for_dir(kind, dir, pin) {
                path_parts.push(self.inner.runtimes.bin_dir(kind, &v));
                env.insert(version_env(kind), v);
            }
        }
        path_parts.push(self.inner.paths.bin());
        if let Some(p) = std::env::var_os("PATH") {
            path_parts.extend(std::env::split_paths(&p));
        }
        if let Ok(joined) = std::env::join_paths(path_parts) {
            env.insert("PATH".into(), joined.to_string_lossy().into_owned());
        }
        env
    }

    async fn start_app(&self, site: &Site) -> Result<()> {
        let cmd = site
            .command
            .clone()
            .ok_or_else(|| CoreError::InvalidInput(format!("site `{}` has no command", site.id)))?;
        let taken: Vec<u16> = self
            .inner
            .app_ports
            .lock()
            .expect("apps")
            .values()
            .copied()
            .collect();
        let port = ports::free_port_from(APP_PORT_BASE, 2000, &taken)
            .ok_or_else(|| CoreError::Conflict("no free port for the app".into()))?;
        let mut spec = ProcessSpec::shell(
            sites::process_name(&site.id),
            &sites::expand_command(&cmd, port),
        )
        .cwd(&site.path);
        spec.env = self.app_env(site, port);
        spec.log_file = Some(self.inner.paths.log_file(&sites::process_name(&site.id)));
        self.inner
            .app_ports
            .lock()
            .expect("apps")
            .insert(site.id.clone(), port);
        if let Err(e) = self.inner.supervisor.start(spec).await {
            self.inner.app_ports.lock().expect("apps").remove(&site.id);
            return Err(e);
        }
        Ok(())
    }

    /// `GET /sites/{id}/logs?lines=`: app output for `app` sites, else the access log.
    pub fn site_logs(&self, id: &str, lines: usize) -> Result<LogLines> {
        let site = self.site(id)?;
        let lines = if site.kind == SiteKind::App {
            let mut l = self.inner.supervisor.logs(&sites::process_name(id), lines);
            if l.is_empty() {
                l = crate::fsutil::tail_lines(
                    &self.inner.paths.log_file(&sites::process_name(id)),
                    lines,
                )?;
            }
            l
        } else {
            crate::fsutil::tail_lines(
                &self.inner.paths.logs().join(format!("site-{id}.log")),
                lines,
            )?
        };
        Ok(LogLines { lines })
    }

    /// `POST /detect`.
    pub fn detect(&self, path: &str) -> Result<Detection> {
        let p = Path::new(path);
        if !p.is_dir() {
            return Err(CoreError::NotFound(format!(
                "directory {path} does not exist"
            )));
        }
        Ok(detection::detect(p, &self.tld()))
    }

    // ----------------------------------------------------------------------------
    // DNS / TLS / ports (delegated to NetProvider)
    // ----------------------------------------------------------------------------

    /// `GET /dns`.
    pub async fn dns_status(&self) -> Result<DnsStatus> {
        self.inner.net.dns_status(&self.tld()).await
    }

    /// `POST /dns/configure` → job (may need elevation inside bz-net).
    pub fn configure_dns(&self, tld: Option<String>) -> JobAccepted {
        let e = self.clone();
        let id = self.inner.jobs.spawn("Configure DNS", move |h| async move {
            if let Some(t) = tld.filter(|t| *t != e.tld()) {
                e.patch_config(&serde_json::json!({ "tld": t })).await?;
            }
            let tld = e.tld();
            h.progress(0.2, Some(&format!("Routing .{tld} to 127.0.0.1")));
            e.inner.net.configure_dns(&tld).await?;
            Ok(Some(format!(".{tld} now resolves locally")))
        });
        JobAccepted { job_id: id }
    }

    /// `GET /tls`.
    pub async fn tls_status(&self) -> Result<TlsStatus> {
        self.inner.net.ca_status().await
    }

    /// `POST /tls/trust` → job.
    pub fn trust_ca(&self) -> JobAccepted {
        let e = self.clone();
        let id = self
            .inner
            .jobs
            .spawn("Trust local CA", move |_h| async move {
                e.inner.net.trust_ca().await?;
                Ok(Some("Local CA trusted".into()))
            });
        JobAccepted { job_id: id }
    }

    /// `GET /ports/{port}` (`ours` set when the pid belongs to a supervised process).
    pub async fn port_owner(&self, port: u16) -> Result<PortOwner> {
        let mut o = self.inner.net.port_owner(port).await?;
        let pids = self.inner.supervisor.pids();
        o.ours = o.pid.is_some_and(|p| pids.contains(&p))
            || (o.pid.is_none()
                && self.our_ports().contains(&port)
                && ports::is_listening(port).await);
        Ok(o)
    }

    fn our_ports(&self) -> Vec<u16> {
        let mut v = vec![];
        if self.caddy_active() {
            let (h, s) = self.web_ports();
            v.extend([h, s]);
            v.extend(self.caddy_admin().tcp_port());
        }
        for k in self.service_kinds() {
            if self.inner.supervisor.is_active(&k.process_name()) {
                v.extend(self.service_port(&k));
            }
        }
        v.extend(self.inner.app_ports.lock().expect("apps").values().copied());
        v
    }

    // ----------------------------------------------------------------------------
    // jobs / config
    // ----------------------------------------------------------------------------

    /// `GET /jobs`.
    pub fn jobs(&self) -> Vec<Job> {
        self.inner.jobs.list()
    }

    /// `GET /jobs/{id}`.
    pub fn job(&self, id: &str) -> Result<Job> {
        self.inner
            .jobs
            .get(id)
            .ok_or_else(|| CoreError::NotFound(format!("job `{id}`")))
    }

    /// `GET /config`.
    pub fn config_json(&self) -> serde_json::Value {
        self.config().to_json()
    }

    fn save_config(&self, cfg: Config) -> Result<()> {
        cfg.validate()?;
        cfg.save(&self.inner.paths.config())?;
        *self.inner.config.write().expect("config") = cfg;
        Ok(())
    }

    /// `PATCH /config` (JSON merge patch). Re-applies Caddy / DNS when relevant keys change.
    pub async fn patch_config(&self, patch: &serde_json::Value) -> Result<serde_json::Value> {
        let old = self.config();
        let new = old.patched(patch)?;
        self.save_config(new.clone())?;
        if old.tld != new.tld {
            // re-normalise site domains onto the new TLD
            let (o, n) = (format!(".{}", old.tld), format!(".{}", new.tld));
            self.inner.state.update(|st| {
                for s in &mut st.sites {
                    let re = |d: &str| match d.strip_suffix(&o) {
                        Some(b) => format!("{b}{n}"),
                        None => d.to_string(),
                    };
                    s.domain = re(&s.domain);
                    s.aliases = s.aliases.iter().map(|a| re(a)).collect();
                }
                Ok(())
            })?;
        }
        if (old.tld != new.tld || old.dns_port != new.dns_port)
            && self
                .inner
                .net
                .dns_status(&old.tld)
                .await
                .map(|d| d.running)
                .unwrap_or(false)
        {
            self.inner.net.start_dns(new.dns_port, &new.tld).await?;
        }
        if old.tld != new.tld
            || old.http_port != new.http_port
            || old.https_port != new.https_port
            || old.lan_access != new.lan_access
        {
            if self.caddy_active()
                && (old.http_port != new.http_port || old.https_port != new.https_port)
            {
                // ports are bound at start: restart caddy
                self.stop_service(&ServiceKind::Caddy).await?;
                self.start_service(&ServiceKind::Caddy).await?;
            } else {
                self.apply_caddy_logged().await;
            }
        }
        Ok(new.to_json())
    }

    // ----------------------------------------------------------------------------
    // doctor
    // ----------------------------------------------------------------------------

    /// `GET /doctor` — core checks (home, PATH, caddy, web ports, runtimes, sites,
    /// TLS trust and DNS through the NetProvider).
    pub async fn doctor(&self) -> Vec<DoctorCheck> {
        let mut out = vec![];
        let check =
            |id: &str, title: &str, status: CheckStatus, detail: String, fix: Option<String>| {
                DoctorCheck {
                    id: id.into(),
                    title: title.into(),
                    status,
                    detail,
                    fix,
                }
            };
        let home = self.inner.paths.home();
        let probe = self.inner.paths.tmp().join(".doctor-probe");
        out.push(match std::fs::write(&probe, b"ok") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                check(
                    "home",
                    "BOZKURT home is writable",
                    CheckStatus::Ok,
                    home.display().to_string(),
                    None,
                )
            }
            Err(e) => check(
                "home",
                "BOZKURT home is writable",
                CheckStatus::Fail,
                format!("{}: {e}", home.display()),
                Some(format!(
                    "Fix permissions of {} or set BOZKURT_HOME",
                    home.display()
                )),
            ),
        });
        let bin = self.inner.paths.bin();
        let on_path = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d == bin))
            .unwrap_or(false);
        out.push(if on_path {
            check(
                "path",
                "Shims are on PATH",
                CheckStatus::Ok,
                bin.display().to_string(),
                None,
            )
        } else {
            let fix = if self.inner.platform.is_windows() {
                format!("setx PATH \"{};%PATH%\"", bin.display())
            } else {
                format!(
                    "Add to your shell profile: export PATH=\"{}:$PATH\"",
                    bin.display()
                )
            };
            check(
                "path",
                "Shims are on PATH",
                CheckStatus::Warn,
                format!("{} is not on PATH", bin.display()),
                Some(fix),
            )
        });
        let caddy = self.service_from_info(&ServiceKind::Caddy, None).await;
        out.push(match caddy.status {
            ServiceStatus::NotInstalled => check(
                "caddy",
                "Web server (Caddy)",
                CheckStatus::Warn,
                "Caddy is not installed".into(),
                Some("bz service install caddy".into()),
            ),
            ServiceStatus::Crashed => check(
                "caddy",
                "Web server (Caddy)",
                CheckStatus::Fail,
                caddy.message.unwrap_or_else(|| "Caddy crashed".into()),
                Some("bz service logs caddy".into()),
            ),
            s => check(
                "caddy",
                "Web server (Caddy)",
                CheckStatus::Ok,
                format!(
                    "{} {}",
                    caddy.version.unwrap_or_default(),
                    serde_json::to_value(s)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                )
                .trim()
                .to_string(),
                None,
            ),
        });
        let cfg = self.config();
        for (id, title, port, fallback) in [
            ("port_http", "HTTP port", cfg.http_port, 8080u16),
            ("port_https", "HTTPS port", cfg.https_port, 8443u16),
        ] {
            let ours = self.caddy_active();
            let free = ports::can_bind_any(port);
            out.push(if ours || free {
                check(
                    id,
                    title,
                    CheckStatus::Ok,
                    format!("port {port} available"),
                    None,
                )
            } else {
                let owner = self
                    .inner
                    .net
                    .port_owner(port)
                    .await
                    .ok()
                    .and_then(|o| {
                        o.process.map(|p| {
                            format!(
                                " by {p}{}",
                                o.pid.map(|x| format!(" (pid {x})")).unwrap_or_default()
                            )
                        })
                    })
                    .unwrap_or_default();
                check(
                    id,
                    title,
                    CheckStatus::Warn,
                    format!(
                        "port {port} is taken{owner} or needs privileges; {fallback} will be used"
                    ),
                    Some(format!(
                        "Free port {port}, or set {} = {fallback} in config.toml",
                        if port == cfg.http_port {
                            "http_port"
                        } else {
                            "https_port"
                        }
                    )),
                )
            });
        }
        let rts = self.runtimes();
        let installed: Vec<String> = rts
            .iter()
            .filter(|r| !r.installed.is_empty())
            .map(|r| {
                format!(
                    "{} {}",
                    r.kind,
                    r.active.clone().unwrap_or_else(|| r.installed[0].clone())
                )
            })
            .collect();
        out.push(if installed.is_empty() {
            check(
                "runtimes",
                "Runtimes",
                CheckStatus::Warn,
                "no runtime installed".into(),
                Some("bz runtime install php 8.4  (or node lts)".into()),
            )
        } else {
            check(
                "runtimes",
                "Runtimes",
                CheckStatus::Ok,
                installed.join(", "),
                None,
            )
        });
        let missing: Vec<String> = self
            .sites()
            .into_iter()
            .filter(|s| !Path::new(&s.path).is_dir())
            .map(|s| format!("{} ({})", s.domain, s.path))
            .collect();
        out.push(if missing.is_empty() {
            check(
                "sites",
                "Site directories exist",
                CheckStatus::Ok,
                format!("{} site(s)", self.inner.state.read(|s| s.sites.len())),
                None,
            )
        } else {
            check(
                "sites",
                "Site directories exist",
                CheckStatus::Fail,
                format!("missing: {}", missing.join(", ")),
                Some("Remove or re-point these sites (bz site rm / bz site set path)".into()),
            )
        });
        out.push(match self.inner.net.ca_status().await {
            Ok(t) if t.ca_trusted => {
                check("tls", "Local CA trusted", CheckStatus::Ok, t.ca_path, None)
            }
            Ok(t) => check(
                "tls",
                "Local CA trusted",
                CheckStatus::Warn,
                format!("{} is not trusted", t.ca_path),
                Some("bz tls trust".into()),
            ),
            Err(e) => check(
                "tls",
                "Local CA trusted",
                CheckStatus::Warn,
                e.to_string(),
                Some("bz tls trust".into()),
            ),
        });
        out.push(match self.inner.net.dns_status(&cfg.tld).await {
            Ok(d) if d.configured && d.running => check(
                "dns",
                "Local DNS",
                CheckStatus::Ok,
                format!(".{} → 127.0.0.1 (port {})", d.tld, d.port),
                None,
            ),
            Ok(d) if !d.configured => check(
                "dns",
                "Local DNS",
                CheckStatus::Warn,
                format!(".{} is not routed to the local resolver", d.tld),
                Some("bz dns configure".into()),
            ),
            Ok(d) => check(
                "dns",
                "Local DNS",
                CheckStatus::Warn,
                format!("responder on port {} is not running", d.port),
                Some("bz service start dns".into()),
            ),
            Err(e) => check(
                "dns",
                "Local DNS",
                CheckStatus::Warn,
                e.to_string(),
                Some("bz dns configure".into()),
            ),
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::NoopNetProvider;

    async fn engine() -> (tempfile::TempDir, Engine) {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let e = Engine::new(&home, Arc::new(NoopNetProvider::new(&home)))
            .await
            .unwrap();
        (t, e)
    }

    #[tokio::test]
    async fn fresh_home_overview_and_config() {
        let (_t, e) = engine().await;
        assert!(e.paths().config().exists());
        assert!(e
            .paths()
            .bin()
            .join(if cfg!(windows) { "node.cmd" } else { "node" })
            .exists());
        let o = e.overview().await.unwrap();
        assert_eq!(o.tld, "test");
        assert_eq!(o.sites_count, 0);
        assert_eq!(o.runtimes.len(), 5);
        let names: Vec<_> = o.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["caddy", "dns", "mariadb", "postgres", "redis", "mailpit"]
        );
        let caddy = &o.services[0];
        assert_eq!(caddy.status, ServiceStatus::NotInstalled);
        assert!(caddy.autostart);
        let v = e
            .patch_config(&serde_json::json!({"dns_port": 5391}))
            .await
            .unwrap();
        assert_eq!(v["dns_port"], 5391);
        assert!(e
            .patch_config(&serde_json::json!({"tld": "dev"}))
            .await
            .is_err());
        let doc = e.doctor().await;
        assert!(doc
            .iter()
            .any(|c| c.id == "caddy" && c.status == CheckStatus::Warn));
        assert!(doc
            .iter()
            .any(|c| c.id == "home" && c.status == CheckStatus::Ok));
        assert_eq!(e.health().version, VERSION);
    }

    #[tokio::test]
    async fn site_lifecycle_writes_caddyfile() {
        let (t, e) = engine().await;
        let proj = t.path().join("shop");
        std::fs::create_dir_all(proj.join("dist")).unwrap();
        std::fs::write(proj.join("dist/index.html"), "hi").unwrap();
        let mut rx = e.subscribe();
        let s = e
            .create_site(&CreateSiteRequest {
                path: proj.to_string_lossy().into_owned(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(s.domain, "shop.test");
        assert_eq!(s.kind, SiteKind::Static);
        assert_eq!(s.root.as_deref(), Some("dist"));
        assert!(matches!(
            rx.recv().await.unwrap(),
            Event::SiteChanged { .. }
        ));
        let cf = std::fs::read_to_string(e.paths().caddyfile()).unwrap();
        assert!(cf.contains("shop.test {"));
        assert!(cf.contains("tls internal")); // Noop provider creates no cert files
        let s2 = e.site_action(&s.id, SiteAction::Unsecure).await.unwrap();
        assert!(!s2.https);
        assert!(std::fs::read_to_string(e.paths().caddyfile())
            .unwrap()
            .contains("http://shop.test {"));
        assert!(e.site_action(&s.id, SiteAction::Start).await.is_err());
        // tld change renames domains
        e.patch_config(&serde_json::json!({"tld": "localhost"}))
            .await
            .unwrap();
        assert_eq!(e.site(&s.id).unwrap().domain, "shop.localhost");
        e.delete_site(&s.id).await.unwrap();
        assert!(e.sites().is_empty());
        assert!(e.site(&s.id).is_err());
    }

    #[tokio::test]
    async fn app_env_allows_the_site_hosts_for_vite() {
        let (t, e) = engine().await;
        let proj = t.path().join("shop");
        std::fs::create_dir_all(&proj).unwrap();
        let mut s = e
            .create_site(&CreateSiteRequest {
                path: proj.to_string_lossy().into_owned(),
                kind: Some(SiteKind::App),
                command: Some("vite".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        s.aliases = vec!["*.shop.test".into(), "admin.test".into()];
        s.env
            .insert(VITE_ALLOWED_HOSTS_ENV.into(), "my.host".into());
        let env = e.app_env(&s, 4100);
        assert_eq!(env["PORT"], "4100");
        assert_eq!(
            env[VITE_ALLOWED_HOSTS_ENV],
            "my.host,.shop.test,.admin.test"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_site_runs_with_assigned_port() {
        let (t, e) = engine().await;
        let proj = t.path().join("api");
        std::fs::create_dir_all(&proj).unwrap();
        let s = e
            .create_site(&CreateSiteRequest {
                path: proj.to_string_lossy().into_owned(),
                kind: Some(SiteKind::App),
                command: Some("echo listening on $PORT; exec sleep 30".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(s.app_status, Some(ServiceStatus::Stopped));
        let r = e.site_action(&s.id, SiteAction::Start).await.unwrap();
        let port = r.app_port.expect("port assigned");
        assert!(port >= APP_PORT_BASE);
        assert_eq!(r.app_status, Some(ServiceStatus::Running));
        for _ in 0..50 {
            if !e.site_logs(&s.id, 10).unwrap().lines.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            e.site_logs(&s.id, 10).unwrap().lines,
            vec![format!("listening on {port}")]
        );
        let cf = std::fs::read_to_string(e.paths().caddyfile()).unwrap();
        assert!(cf.contains(&format!("reverse_proxy localhost:{port}")));
        let stopped = e.site_action(&s.id, SiteAction::Stop).await.unwrap();
        assert_eq!(stopped.app_status, Some(ServiceStatus::Stopped));
        assert_eq!(stopped.app_port, None);
    }

    #[tokio::test]
    async fn service_names_and_external_detection() {
        let (_t, e) = engine().await;
        assert!(e.service("nginx").await.is_err());
        // occupy a port and point redis at it → external
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let s = e
            .patch_service(
                "redis",
                &ServicePatch {
                    port: Some(port),
                    autostart: Some(true),
                },
            )
            .await
            .unwrap();
        assert_eq!(s.status, ServiceStatus::External);
        assert!(s.autostart);
        assert!(e.config().autostart.contains(&"redis".to_string()));
        // starting an external service is a no-op
        let s = e
            .service_action("redis", ServiceAction::Start)
            .await
            .unwrap();
        assert_eq!(s.status, ServiceStatus::External);
        drop(l);
        let s = e.service("redis").await.unwrap();
        assert_eq!(s.status, ServiceStatus::NotInstalled);
        // dns via provider
        let d = e.service_action("dns", ServiceAction::Start).await.unwrap();
        assert_eq!(d.status, ServiceStatus::Running);
        assert!(e.dns_status().await.unwrap().running);
        let d = e.service_action("dns", ServiceAction::Stop).await.unwrap();
        assert_eq!(d.status, ServiceStatus::Stopped);
        let all = e.all_services_action(ServiceAction::Stop).await.unwrap();
        assert_eq!(all.len(), 6);
    }

    #[tokio::test]
    async fn jobs_are_listed() {
        let (_t, e) = engine().await;
        let j = e.trust_ca();
        let done = e
            .jobs_manager()
            .wait(&j.job_id, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(done.status, JobStatus::Done);
        assert_eq!(e.jobs().len(), 1);
        assert_eq!(e.job(&j.job_id).unwrap().title, "Trust local CA");
        assert!(e.job("nope").is_err());
    }
}
