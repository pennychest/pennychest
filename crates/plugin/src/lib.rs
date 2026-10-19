//! The protocol PennyChest uses to talk to plugins, which run as separate programs and speak
//! JSON-RPC over stdin and stdout, and an SDK for writing plugins in Rust.
//!
//! - [`protocol`]: the messages, as `docs/plugins/protocol.md` describes them.
//! - [`manifest`]: `pennychest-plugin.toml`, which says how to run a plugin.
//! - [`Plugin`], [`Importer`] and [`Exporter`]: the SDK. A plugin is a `main` that adds its
//!   importers and exporters to a [`Plugin`] and calls [`Plugin::serve`].
//! - [`client`]: PennyChest's side, which the plugin host uses.
//!
//! The SDK uses only the standard library's stdin and stdout, so a plugin built with it runs
//! natively or compiled to WASI (`wasm32-wasip1`).

pub mod client;
pub mod manifest;
pub mod protocol;
mod sdk;

pub use chrono::NaiveDate;
pub use protocol::{
    ExporterInfo, ImporterInfo, Ledger, LedgerAccount, LedgerBudget, LedgerPosting,
    LedgerTransaction, Row, StatementInfo,
};
pub use rust_decimal::Decimal;
pub use sdk::{Exporter, Importer, Plugin};
