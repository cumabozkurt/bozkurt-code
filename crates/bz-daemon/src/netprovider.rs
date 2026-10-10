//! [`BzNetProvider`]: the real [`bz_core::NetProvider`] on top of [`bz_net::NetService`].
//!
//! Blocking bz-net calls (certificate generation, trust-store probes, plan application
//! that may show an elevation dialog) run in `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use bz_core::models::{DnsStatus, PortOwner, TlsStatus};
use bz_core::{async_trait, CoreError, NetProvider, Result};
use bz_net::{DnsHandle, NetError, NetService, Plan};
use tokio::sync::Mutex;

/// Map a bz-net error onto the engine error (and so onto API codes/status).
pub fn net_err(e: NetError) -> CoreError {
    match e {
        NetError::Invalid(m) => CoreError::InvalidInput(m),
        NetError::Unsupported(m) => CoreError::Unsupported(m),
        NetError::Missing(p) => CoreError::NotFound(format!("missing file {}", p.display())),
        NetError::ElevationDenied(m) => {
            CoreError::Conflict(format!("elevation was cancelled or denied: {m}"))
        }
        other => CoreError::Other(anyhow::anyhow!(other.to_string())),
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::result::Result<T, NetError> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CoreError::Other(anyhow::anyhow!("blocking task: {e}")))?
        .map_err(net_err)
}

/// bz-net backed networking for the engine.
#[derive(Debug)]
pub struct BzNetProvider {
    svc: NetService,
    dns: Mutex<Option<DnsHandle>>,
    /// Port the responder runs on (or was last asked to run on).
    dns_port: AtomicU16,
}

impl BzNetProvider {
    /// Provider for the BOZKURT home `home`; `dns_port` is the configured responder port.
    pub fn new(home: &Path, dns_port: u16) -> BzNetProvider {
        BzNetProvider {
            svc: NetService::new(bz_net::NetPaths::from_bozkurt_home(home)),
            dns: Mutex::new(None),
            dns_port: AtomicU16::new(dns_port),
        }
    }

    /// Shared handle (what the engine and the daemon both hold).
    pub fn shared(home: &Path, dns_port: u16) -> Arc<BzNetProvider> {
        Arc::new(BzNetProvider::new(home, dns_port))
    }

    /// The underlying bz-net service.
    pub fn service(&self) -> &NetService {
        &self.svc
    }

    /// Create the local CA if it does not exist yet; returns `rootCA.pem`.
    pub async fn ensure_ca(&self) -> Result<PathBuf> {
        let svc = self.svc.clone();
        blocking(move || svc.ensure_ca().map(|i| i.cert_path)).await
    }

    /// Apply a confirmed plan (blocking, may prompt for elevation).
    pub async fn apply(&self, plan: Plan) -> Result<String> {
        if plan.is_empty() {
            return Ok(String::new());
        }
        let svc = self.svc.clone();
        let r = blocking(move || svc.apply(&plan)).await?;
        Ok(r.output)
    }

    /// Trust the CA in the user stores (+ system store when `system`).
    pub async fn trust(&self, system: bool) -> Result<()> {
        let svc = self.svc.clone();
        let plan = blocking(move || svc.trust_ca(system)).await?;
        self.apply(plan).await.map(|_| ())
    }

    /// Remove the CA from the trust stores.
    pub async fn untrust(&self, system: bool) -> Result<()> {
        let svc = self.svc.clone();
        let plan = blocking(move || svc.untrust_ca(system)).await?;
        self.apply(plan).await.map(|_| ())
    }

    /// Remove the OS routing of `tld`.
    pub async fn unconfigure_dns(&self, tld: &str) -> Result<()> {
        let svc = self.svc.clone();
        let tld = tld.to_string();
        let plan = blocking(move || svc.dns_unconfigure_plan(&tld)).await?;
        self.apply(plan).await.map(|_| ())
    }

    /// Port the responder must use for `tld`: the configured one, except where the OS
    /// routing needs a fixed port (Windows NRPT → 53).
    async fn responder_port(&self, tld: &str, port: u16) -> u16 {
        if !cfg!(windows) {
            return port;
        }
        let svc = self.svc.clone();
        let tld = tld.to_string();
        blocking(move || svc.dns_configure_plan(&tld, port).map(|s| s.responder_port))
            .await
            .unwrap_or(port)
    }

    /// Start (or restart) the responder on exactly `port`, without the Windows NRPT remap.
    async fn start_dns_on(&self, port: u16, tld: &str) -> Result<()> {
        let mut g = self.dns.lock().await;
        if let Some(h) = g.take() {
            h.stop().await;
        }
        let h = self
            .svc
            .start_dns(port, &[tld.to_string()])
            .await
            .map_err(net_err)?;
        tracing::info!("dns responder on {} for .{tld}", h.local_addr());
        self.dns_port.store(h.local_addr().port(), Ordering::SeqCst);
        *g = Some(h);
        Ok(())
    }

    /// Address the DNS responder is bound to, when running.
    pub async fn dns_addr(&self) -> Option<std::net::SocketAddr> {
        let g = self.dns.lock().await;
        g.as_ref()
            .filter(|h| h.is_running())
            .map(|h| h.local_addr())
    }

    /// Is something bound to TCP `port`?
    pub fn port_in_use(&self, port: u16) -> bool {
        !self.svc.is_port_free(port)
    }
}

#[async_trait]
impl NetProvider for BzNetProvider {
    async fn ensure_cert(&self, domain: &str, wildcard: bool) -> Result<(PathBuf, PathBuf)> {
        let svc = self.svc.clone();
        let domain = domain.to_string();
        blocking(move || svc.ensure_cert(&domain, &[], wildcard)).await
    }

    async fn ca_status(&self) -> Result<TlsStatus> {
        let svc = self.svc.clone();
        let s = tokio::task::spawn_blocking(move || svc.ca_status())
            .await
            .map_err(|e| CoreError::Other(anyhow::anyhow!("blocking task: {e}")))?;
        Ok(TlsStatus {
            ca_path: s.ca_path.to_string_lossy().into_owned(),
            ca_trusted: s.ca_trusted,
            ca_expires: s
                .ca_expires
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        })
    }

    async fn trust_ca(&self) -> Result<()> {
        self.trust(false).await
    }

    async fn dns_status(&self, tld: &str) -> Result<DnsStatus> {
        let running = self.dns_addr().await;
        let port = running
            .map(|a| a.port())
            .unwrap_or_else(|| self.dns_port.load(Ordering::SeqCst));
        let svc = self.svc.clone();
        let t = tld.to_string();
        let configured = blocking(move || svc.dns_status(&t, port))
            .await
            .map(|s| s.configured)
            .unwrap_or(false);
        Ok(DnsStatus {
            tld: tld.to_string(),
            port,
            configured,
            running: running.is_some(),
        })
    }

    async fn configure_dns(&self, tld: &str) -> Result<()> {
        let port = self.dns_port.load(Ordering::SeqCst);
        let svc = self.svc.clone();
        let t = tld.to_string();
        let mut setup = blocking(move || svc.dns_configure_plan(&t, port)).await?;
        if setup.responder_port != port {
            // e.g. Windows NRPT: the responder has to answer on :53. This runs before the
            // elevation prompt, so a busy port never leaves an NRPT rule pointing at
            // nothing: fall back to the hosts file instead (P1-14).
            tracing::info!(
                "dns routing via {:?} needs the responder on port {}",
                setup.backend,
                setup.responder_port
            );
            if let Err(e) = self.start_dns(setup.responder_port, tld).await {
                tracing::warn!(
                    "dns responder could not bind port {}: {e}; using the hosts file",
                    setup.responder_port
                );
                let why = format!(
                    "The DNS responder could not listen on 127.0.0.1:{} ({e}); sites are listed in the hosts file instead (no wildcard subdomains).",
                    setup.responder_port
                );
                let svc = self.svc.clone();
                let t = tld.to_string();
                setup =
                    blocking(move || svc.dns_configure_plan_hosts_fallback(&t, port, &why)).await?;
                // keep serving on the configured port for `bz dns status`
                let _ = self.start_dns_on(port, tld).await;
            }
        }
        self.apply(setup.plan).await.map(|_| ())
    }

    async fn start_dns(&self, port: u16, tld: &str) -> Result<()> {
        let port = self.responder_port(tld, port).await;
        self.start_dns_on(port, tld).await
    }

    async fn stop_dns(&self) -> Result<()> {
        if let Some(h) = self.dns.lock().await.take() {
            h.stop().await;
        }
        Ok(())
    }

    async fn port_owner(&self, port: u16) -> Result<PortOwner> {
        let svc = self.svc.clone();
        let o = tokio::task::spawn_blocking(move || svc.port_owner(port))
            .await
            .map_err(|e| CoreError::Other(anyhow::anyhow!("blocking task: {e}")))?;
        Ok(PortOwner {
            port,
            pid: o.pid,
            process: o.process,
            ours: o.ours,
        })
    }
}
