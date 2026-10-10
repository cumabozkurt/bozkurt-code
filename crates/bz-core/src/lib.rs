//! # bz-core — the BOZKURT CODE engine
//!
//! Library behind `bozkurtd` (and through it the GUI, `bz` CLI, MCP server and mobile
//! companion). It owns everything under `$BOZKURT_HOME`: paths, `config.toml`,
//! `state.json`, live runtime catalogs, installers, runtime switching + shims, services,
//! sites, the process supervisor, project detection, Caddyfile generation, jobs and the
//! event bus. The entry point is [`Engine`]; wire types are in [`models`] and
//! serialize exactly as documented in `docs/API.md`.
//!
//! ## The `NetProvider` seam (TLS, DNS, ports)
//!
//! bz-core does **not** depend on `bz-net`. Everything networking-related is reached
//! through the [`NetProvider`] trait, which the daemon implements with bz-net and hands
//! to the engine as `Arc<dyn NetProvider>`:
//!
//! ```ignore
//! #[async_trait::async_trait]
//! pub trait NetProvider: Send + Sync + 'static {
//!     /// Leaf cert for `domain` (+ `*.domain` if `wildcard`), issued/renewed from the
//!     /// local CA. Returns (cert_pem_path, key_pem_path) — `certs/<domain>.pem`, `-key.pem`.
//!     async fn ensure_cert(&self, domain: &str, wildcard: bool) -> Result<(PathBuf, PathBuf)>;
//!     async fn ca_status(&self) -> Result<TlsStatus>;            // GET /tls
//!     async fn trust_ca(&self) -> Result<()>;                    // POST /tls/trust (job)
//!     async fn dns_status(&self, tld: &str) -> Result<DnsStatus>; // GET /dns
//!     async fn configure_dns(&self, tld: &str) -> Result<()>;    // POST /dns/configure (job)
//!     async fn start_dns(&self, port: u16, tld: &str) -> Result<()>; // service "dns" start
//!     async fn stop_dns(&self) -> Result<()> { Ok(()) }           // service "dns" stop
//!     async fn port_owner(&self, port: u16) -> Result<PortOwner>; // GET /ports/{port}
//! }
//! ```
//!
//! * `ensure_cert` is called for every certificate group of every HTTPS site before the
//!   Caddyfile is written; if the returned files do not exist the site falls back to
//!   Caddy's `tls internal`, so the config always loads.
//! * `port_owner` may leave `ours = false`; the engine sets it from supervised PIDs.
//! * Elevation-requiring calls (`configure_dns`, sometimes `trust_ca`) are only invoked
//!   from jobs the user explicitly started.
//! * [`NoopNetProvider`] is a no-op implementation for tests and headless use.
//!
//! ## Quick start
//!
//! ```no_run
//! # async fn demo() -> bz_core::Result<()> {
//! use std::sync::Arc;
//! use bz_core::{Engine, NoopNetProvider, models::RuntimeKind};
//! let home = bz_core::paths::default_home();
//! let engine = Engine::new(&home, Arc::new(NoopNetProvider::new(&home))).await?;
//! let job = engine.install_runtime(RuntimeKind::Node, "lts");
//! println!("installing in job {}", job.job_id);
//! # Ok(()) }
//! ```
//!
//! ## Modules
//! * [`paths`] — home resolution and layout · [`config`] — `config.toml`, `bozkurt.toml`
//! * [`state`] — atomic `state.json` · [`models`] — API types · [`error`] — error codes
//! * [`events`] / [`jobs`] — event bus and background jobs
//! * [`download`] — verified streaming downloads + extraction · [`catalogs`] — runtime indexes
//! * [`runtimes`] — install/use/uninstall + shims · [`detection`] — framework detection
//! * [`supervisor`] — child processes · [`services`] — caddy/mariadb/postgres/redis/mailpit/php-fpm
//! * [`caddyfile`] — config generation + reload · [`sites`] — site records
//! * [`net`] — the `NetProvider` seam · [`ports`], [`platform`], [`version`], [`fsutil`] — helpers

pub mod apitoken;
pub mod caddyfile;
pub mod catalogs;
pub mod config;
pub mod detection;
pub mod download;
pub mod engine;
pub mod error;
pub mod events;
pub mod fsutil;
pub mod jobs;
pub mod models;
pub mod net;
pub mod paths;
pub mod platform;
pub mod ports;
pub mod procguard;
pub mod runtimes;
pub mod services;
pub mod sites;
pub mod state;
pub mod supervisor;
pub mod version;

pub use engine::Engine;
pub use error::{CoreError, Result};
pub use events::{Event, EventBus};
pub use net::{NetProvider, NoopNetProvider};
pub use paths::Paths;
pub use platform::Platform;

/// Re-exported so implementors of [`NetProvider`] use the same macro version.
pub use async_trait::async_trait;
