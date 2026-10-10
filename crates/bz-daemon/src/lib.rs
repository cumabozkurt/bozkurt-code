//! # bz-daemon — `bozkurtd`
//!
//! The BOZKURT CODE daemon: an axum HTTP API (`docs/API.md`, `/api/v1`) plus the
//! `/events` WebSocket over the [`bz_core::Engine`], with networking provided by
//! [`netprovider::BzNetProvider`] (bz-net). The desktop app, `bz` CLI, MCP server and
//! mobile companion all talk to it.
//!
//! * [`server`] — assembling and running the daemon ([`Daemon::start`])
//! * [`api`] — router, handlers, CORS, remote auth, events
//! * [`netprovider`] — `NetProvider` on bz-net
//! * [`remote`] — pairing codes, device tokens, mDNS
//! * [`databases`], [`import`], [`share`] — databases, migration scan, public tunnels
//! * [`instance`] — single-instance pid file, logging

pub mod api;
pub mod databases;
pub mod error;
pub mod import;
pub mod instance;
pub mod netprovider;
pub mod remote;
pub mod server;
pub mod share;
pub mod state;

pub use server::{Daemon, DaemonOptions, NetChoice};
