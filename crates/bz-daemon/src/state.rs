//! Shared daemon state and the remote listener lifecycle.

use std::net::SocketAddr;
use std::sync::Arc;

use bz_core::Engine;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::netprovider::BzNetProvider;
use crate::remote::{MdnsAdvert, RemoteAuth};
use crate::share::Shares;

/// Which listener a request arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 127.0.0.1:<api_port>: Host/Origin checks, per-install token for non-browser clients.
    Local,
    /// 0.0.0.0:<remote.port>, bearer token.
    Remote,
}

/// A running remote listener.
pub struct RemoteListener {
    pub addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _mdns: Option<MdnsAdvert>,
}

impl RemoteListener {
    pub fn new(
        addr: SocketAddr,
        shutdown: oneshot::Sender<()>,
        task: JoinHandle<()>,
        mdns: Option<MdnsAdvert>,
    ) -> RemoteListener {
        RemoteListener {
            addr,
            shutdown: Some(shutdown),
            task: Some(task),
            _mdns: mdns,
        }
    }

    /// Stop accepting and wait (briefly) for the server task.
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.task.take() {
            if tokio::time::timeout(std::time::Duration::from_secs(3), t)
                .await
                .is_err()
            {
                tracing::debug!("remote listener did not stop in time");
            }
        }
    }
}

/// Everything handlers need.
pub struct AppState {
    pub engine: Engine,
    /// Per-install local API token (`$BOZKURT_HOME/api-token`).
    pub api_token: String,
    /// Port of the local listener (for the Host check).
    pub local_port: u16,
    /// The concrete bz-net provider (None when running with the no-op provider).
    pub bznet: Option<Arc<BzNetProvider>>,
    pub remote_auth: RemoteAuth,
    pub remote: Mutex<Option<RemoteListener>>,
    pub shares: Shares,
    /// Whether to advertise the remote listener over mDNS.
    pub mdns: bool,
    /// Set by `POST /daemon/shutdown`; `bozkurtd` waits on it next to SIGTERM/Ctrl-C.
    pub shutdown_request: tokio::sync::Notify,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("engine", &self.engine)
            .finish()
    }
}

/// Handle type used by the router.
pub type Shared = Arc<AppState>;
