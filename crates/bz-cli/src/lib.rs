//! # bz-cli — the `bz` command-line client of BOZKURT CODE
//!
//! `bz` is a thin client over the bozkurtd HTTP API (`docs/API.md`, `docs/CLI.md`).
//! The library half is shared with `bozkurt-mcp`: the typed HTTP [`client`], daemon
//! discovery/auto-start in [`daemon`], argument [`resolve`]rs and the MCP client
//! config writer in [`register`].

pub mod cli;
pub mod client;
pub mod commands;
pub mod daemon;
pub mod error;
pub mod events;
pub mod i18n;
pub mod register;
pub mod resolve;
pub mod ui;

pub use client::Client;
pub use error::CliError;
