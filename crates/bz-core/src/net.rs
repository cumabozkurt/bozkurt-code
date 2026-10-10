//! The [`NetProvider`] seam between bz-core and bz-net.
//!
//! bz-core never depends on bz-net. TLS (local CA, leaf certificates, trust stores),
//! local DNS (responder + OS resolver routing) and port inspection are reached through
//! this trait; the daemon wires in the real bz-net implementation, tests use
//! [`NoopNetProvider`].

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::error::Result;
use crate::models::{DnsStatus, PortOwner, TlsStatus};

/// Networking services provided by `bz-net` (or a test double).
///
/// All methods are async and must be safe to call concurrently. Methods that need
/// elevation (`trust_ca` on some systems, `configure_dns`) are only invoked by the engine
/// from inside a job, after the caller confirmed the action.
#[async_trait]
pub trait NetProvider: Send + Sync + 'static {
    /// Ensure a valid leaf certificate for `domain` (SAN: `domain`, plus `*.domain`
    /// when `wildcard`) exists, issuing or renewing it from the local CA as needed.
    /// Returns `(cert_pem_path, key_pem_path)`, conventionally
    /// `$BOZKURT_HOME/certs/<domain>.pem` and `<domain>-key.pem`.
    async fn ensure_cert(&self, domain: &str, wildcard: bool) -> Result<(PathBuf, PathBuf)>;

    /// Local CA status (`GET /tls`).
    async fn ca_status(&self) -> Result<TlsStatus>;

    /// Trust the local CA in the OS / browser stores (`POST /tls/trust`).
    async fn trust_ca(&self) -> Result<()>;

    /// DNS responder + OS routing status for `tld` (`GET /dns`).
    async fn dns_status(&self, tld: &str) -> Result<DnsStatus>;

    /// Route `*.<tld>` to the local responder at OS level (resolver file / NRPT /
    /// systemd-resolved). May require elevation; backs up files it changes.
    async fn configure_dns(&self, tld: &str) -> Result<()>;

    /// Start the in-process DNS responder on `127.0.0.1:<port>` answering `*.<tld>`.
    /// Calling it while running restarts it with the new parameters.
    async fn start_dns(&self, port: u16, tld: &str) -> Result<()>;

    /// Stop the DNS responder (default: no-op).
    async fn stop_dns(&self) -> Result<()> {
        Ok(())
    }

    /// Who is listening on TCP `port` (pid/process when determinable). `ours` is filled
    /// in by the engine, implementations may leave it `false`.
    async fn port_owner(&self, port: u16) -> Result<PortOwner>;
}

/// A do-nothing provider for tests and for running the engine without networking.
/// Certificates are reported at their conventional paths but never created (so
/// generated Caddyfiles fall back to `tls internal` unless the files exist); DNS
/// start/stop is tracked in memory; `port_owner` only probes whether the port accepts
/// connections.
#[derive(Debug)]
pub struct NoopNetProvider {
    certs_dir: PathBuf,
    ca_path: PathBuf,
    dns: Mutex<Option<(u16, String)>>,
}

impl NoopNetProvider {
    /// `home` is the BOZKURT home (used only to build conventional paths).
    pub fn new(home: impl Into<PathBuf>) -> NoopNetProvider {
        let home = home.into();
        NoopNetProvider {
            certs_dir: home.join("certs"),
            ca_path: home.join("ca").join("rootCA.pem"),
            dns: Mutex::new(None),
        }
    }
}

#[async_trait]
impl NetProvider for NoopNetProvider {
    async fn ensure_cert(&self, domain: &str, _wildcard: bool) -> Result<(PathBuf, PathBuf)> {
        Ok((
            self.certs_dir.join(format!("{domain}.pem")),
            self.certs_dir.join(format!("{domain}-key.pem")),
        ))
    }

    async fn ca_status(&self) -> Result<TlsStatus> {
        Ok(TlsStatus {
            ca_path: self.ca_path.to_string_lossy().into_owned(),
            ca_trusted: false,
            ca_expires: None,
        })
    }

    async fn trust_ca(&self) -> Result<()> {
        Ok(())
    }

    async fn dns_status(&self, tld: &str) -> Result<DnsStatus> {
        let g = self.dns.lock().expect("dns");
        Ok(DnsStatus {
            tld: tld.to_string(),
            port: g.as_ref().map(|d| d.0).unwrap_or(5390),
            configured: false,
            running: g.is_some(),
        })
    }

    async fn configure_dns(&self, _tld: &str) -> Result<()> {
        Ok(())
    }

    async fn start_dns(&self, port: u16, tld: &str) -> Result<()> {
        *self.dns.lock().expect("dns") = Some((port, tld.to_string()));
        Ok(())
    }

    async fn stop_dns(&self) -> Result<()> {
        *self.dns.lock().expect("dns") = None;
        Ok(())
    }

    async fn port_owner(&self, port: u16) -> Result<PortOwner> {
        let _busy = crate::ports::is_listening(port).await;
        Ok(PortOwner {
            port,
            pid: None,
            process: None,
            ours: false,
        })
    }
}
