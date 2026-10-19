//! PennyChest's core: the ledger, imports, rules, categorisation, AI and the actions shared by
//! the API, chat and MCP. Nothing here knows about HTTP; the server crate maps it onto routes.

pub mod accounts;
pub mod actions;
pub mod ai;
pub mod auth;
pub mod budgets;
pub mod chat;
pub mod dashboard;
pub mod db;
pub mod export;
pub mod imports;
pub mod insights;
pub mod learned;
pub mod ledger;
pub mod lookups;
pub mod mcp;
pub mod money;
pub mod plugins;
pub mod reports;
pub mod rules;
pub mod settings;
pub mod taps;

pub use money::Money;
