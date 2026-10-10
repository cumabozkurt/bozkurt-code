//! Daemon assembly: engine + provider, local listener, remote listener, autostart,
//! graceful shutdown.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use bz_core::config::Config;
use bz_core::{Engine, NetProvider, NoopNetProvider, Paths};
use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;

use crate::api;
use crate::error::ApiErr;
use crate::netprovider::BzNetProvider;
use crate::remote::{MdnsAdvert, RemoteAuth};
use crate::share::Shares;
use crate::state::{AppState, RemoteListener, Scope, Shared};

/// Which networking implementation to plug into the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetChoice {
    /// bz-net (local CA, DNS responder, OS routing).
    Real,
    /// No-op provider (tests, headless).
    Noop,
}

/// Startup options.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    pub home: PathBuf,
    /// Overrides `api_port` from config.toml (0 = pick a free port).
    pub api_port: Option<u16>,
    pub net: NetChoice,
    /// Start `config.autostart` services and apply the Caddy config.
    pub autostart: bool,
    /// Advertise the remote listener via mDNS.
    pub mdns: bool,
}

impl DaemonOptions {
    pub fn new(home: impl Into<PathBuf>) -> DaemonOptions {
        DaemonOptions {
            home: home.into(),
            api_port: None,
            net: NetChoice::Real,
            autostart: true,
            mdns: true,
        }
    }
}

/// A running daemon.
pub struct Daemon {
    pub state: Shared,
    /// Address of the local API listener.
    pub local_addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    server: Option<JoinHandle<()>>,
    autostart: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("local_addr", &self.local_addr)
            .finish()
    }
}

impl Daemon {
    /// Open the home, bind the API and start serving.
    pub async fn start(opts: DaemonOptions) -> anyhow::Result<Daemon> {
        let paths = Paths::new(&opts.home);
        paths
            .ensure()
            .with_context(|| format!("creating {}", opts.home.display()))?;
        let pre = Config::load(&paths.config()).context("reading config.toml")?;
        let (net, bznet): (Arc<dyn NetProvider>, Option<Arc<BzNetProvider>>) = match opts.net {
            NetChoice::Real => {
                let p = BzNetProvider::shared(&opts.home, pre.dns_port);
                (p.clone(), Some(p))
            }
            NetChoice::Noop => (Arc::new(NoopNetProvider::new(&opts.home)), None),
        };
        let engine = Engine::new(&opts.home, net)
            .await
            .context("opening the BOZKURT home")?;
        if let Some(p) = &bznet {
            match p.ensure_ca().await {
                Ok(ca) => tracing::info!("local CA: {}", ca.display()),
                Err(e) => tracing::warn!("could not create the local CA: {e}"),
            }
        }
        let cfg = engine.config();
        let port = opts.api_port.unwrap_or(cfg.api_port);
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .with_context(|| {
                format!("binding 127.0.0.1:{port} (is another bozkurtd running? use --port)")
            })?;
        let local_addr = listener.local_addr()?;
        // we own the home now: kill what a crashed predecessor left running (P1-7)
        let reaped = engine.reap_orphans().await;
        if !reaped.is_empty() {
            tracing::warn!("reaped orphaned processes: {}", reaped.join(", "));
        }
        let api_token = bz_core::apitoken::load_or_create(&opts.home).with_context(|| {
            format!("creating {}", bz_core::apitoken::path(&opts.home).display())
        })?;
        let state: Shared = Arc::new(AppState {
            api_token,
            local_port: local_addr.port(),
            remote_auth: RemoteAuth::open(opts.home.join("remote.json")),
            engine: engine.clone(),
            bznet,
            remote: Mutex::new(None),
            shares: Shares::default(),
            mdns: opts.mdns,
            shutdown_request: tokio::sync::Notify::new(),
        });
        let (tx, mut rx) = watch::channel(false);
        let app = api::router(state.clone(), Scope::Local);
        let server = tokio::spawn(async move {
            let svc = app.into_make_service_with_connect_info::<SocketAddr>();
            let r = axum::serve(listener, svc)
                .with_graceful_shutdown(async move {
                    let _ = rx.wait_for(|v| *v).await;
                })
                .await;
            if let Err(e) = r {
                tracing::error!("api server: {e}");
            }
        });
        tracing::info!(
            "bozkurtd {} listening on http://{local_addr}/api/v1",
            bz_core::engine::VERSION
        );
        let autostart = opts.autostart.then(|| {
            let st = state.clone();
            tokio::spawn(async move {
                let started = st.engine.start_autostart().await;
                for s in &started {
                    tracing::info!("autostart {}: {:?}", s.name, s.status);
                }
                if let Err(e) = st.engine.apply_caddy().await {
                    tracing::warn!("applying caddy config: {e}");
                }
            })
        });
        if cfg.remote.enabled {
            if let Err(e) = reconcile_remote(&state).await {
                tracing::warn!("remote listener: {}", e.message);
            }
        }
        Ok(Daemon {
            state,
            local_addr,
            shutdown: tx,
            server: Some(server),
            autostart,
        })
    }

    /// Address of the remote listener, when running.
    pub async fn remote_addr(&self) -> Option<SocketAddr> {
        self.state.remote.lock().await.as_ref().map(|r| r.addr)
    }

    /// Resolves when a client asked for a graceful stop (`POST /api/v1/daemon/shutdown`).
    pub async fn shutdown_requested(&self) {
        self.state.shutdown_request.notified().await
    }

    /// The per-install local API token.
    pub fn api_token(&self) -> &str {
        &self.state.api_token
    }

    /// The engine.
    pub fn engine(&self) -> &Engine {
        &self.state.engine
    }

    /// Stop listeners, then every supervised process and the DNS responder.
    pub async fn shutdown(mut self) {
        tracing::info!("shutting down");
        if let Some(a) = self.autostart.take() {
            a.abort();
        }
        if let Some(r) = self.state.remote.lock().await.take() {
            r.stop().await;
        }
        let _ = self.shutdown.send(true);
        if let Some(s) = self.server.take() {
            if tokio::time::timeout(std::time::Duration::from_secs(5), s)
                .await
                .is_err()
            {
                tracing::warn!("api server did not stop within 5s");
            }
        }
        self.state.engine.shutdown().await;
        tracing::info!("stopped");
    }
}

/// Make the remote listener match `config.remote` (start/stop/rebind).
pub async fn reconcile_remote(st: &Shared) -> Result<(), ApiErr> {
    reconcile_remote_to(st, &st.engine.config().remote).await
}

/// Bring the remote listener in line with `cfg` (which need not be saved yet): callers
/// bind first and persist `remote.enabled = true` only once the bind worked.
pub async fn reconcile_remote_to(
    st: &Shared,
    cfg: &bz_core::config::RemoteConfig,
) -> Result<(), ApiErr> {
    let mut g = st.remote.lock().await;
    let wanted = cfg.enabled.then_some(cfg.port);
    let same = match (&*g, wanted) {
        (Some(r), Some(p)) => p == 0 || r.addr.port() == p,
        (None, None) => true,
        _ => false,
    };
    if same {
        return Ok(());
    }
    if let Some(r) = g.take() {
        r.stop().await;
        tracing::info!("remote listener stopped");
    }
    let Some(port) = wanted else { return Ok(()) };
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
        .await
        .map_err(|e| {
            ApiErr::new(
                axum::http::StatusCode::CONFLICT,
                "conflict",
                format!("cannot listen on 0.0.0.0:{port}: {e}"),
            )
        })?;
    let addr = listener
        .local_addr()
        .map_err(|e| ApiErr::internal(e.to_string()))?;
    let app = api::router(st.clone(), Scope::Remote);
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let svc = app.into_make_service_with_connect_info::<SocketAddr>();
        let r = axum::serve(listener, svc)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
        if let Err(e) = r {
            tracing::error!("remote server: {e}");
        }
    });
    let mdns = if st.mdns {
        match MdnsAdvert::start(addr.port()) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!("mDNS advertisement failed: {e}");
                None
            }
        }
    } else {
        None
    };
    tracing::info!("remote listener on {addr}");
    *g = Some(RemoteListener::new(addr, tx, task, mdns));
    Ok(())
}
