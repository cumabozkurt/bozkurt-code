//! # bz-net
//!
//! Networking layer of BOZKURT CODE, independent of `bz-core`:
//!
//! * [`dns`] — in-process DNS responder answering `*.<tld>` with loopback addresses.
//! * [`resolver`] — routing a TLD to that responder through the OS resolver (plans).
//! * [`tls`] — local certificate authority and leaf certificates; [`trust`] — OS/browser trust
//!   stores (plans).
//! * [`ports`] — who holds a port, free-port search.
//! * [`hosts`] — managed block in the hosts file.
//! * [`plan`] / [`elevation`] — every change outside `$BOZKURT_HOME` is a [`Plan`] that the caller
//!   shows to the user, then [`NetService::apply`] performs it (with backups and one elevation prompt).
//!
//! [`NetService`] is the façade `bz-core` plugs in.

pub mod dns;
pub mod elevation;
mod error;
pub mod hosts;
pub mod plan;
pub mod ports;
pub mod resolver;
mod service;
pub mod tls;
pub mod trust;

pub use dns::{DnsConfig, DnsHandle, UnknownPolicy};
pub use elevation::ApplyReport;
pub use error::{NetError, Result};
pub use plan::{Action, Os, Plan, Step};
pub use ports::{PortOwner, Proto};
pub use resolver::{DnsSetup, DnsStatus, ResolverBackend, SystemEnv};
pub use service::{NetPaths, NetService};
pub use tls::{CaInfo, CaStatus, CertInfo, LeafCert, LocalCa, TrustStoreStatus};
pub use trust::{LinuxSystemStore, TrustEnv};
