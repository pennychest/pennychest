//! The messages PennyChest and a plugin exchange. `docs/plugins/protocol.md` describes the
//! protocol for plugin authors in any language; these are its types.
//!
//! Every message is one line of JSON-RPC 2.0. PennyChest sends requests on the plugin's
//! stdin, one at a time, and the plugin answers each on stdout before the next is sent.
//! Files travel as base64, dates as `YYYY-MM-DD` and amounts as decimal strings, never as
//! JSON numbers, so nothing is lost to floating point.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The protocol version this crate speaks. A plugin reports the version it speaks in its
/// handshake; PennyChest refuses one it doesn't know.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest message either side may send, in bytes: a file of about 48 MiB once it's
/// base64.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// How long PennyChest waits for an answer, in seconds, before it stops the plugin.
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 10;
pub const CALL_TIMEOUT_SECS: u64 = 120;

/// The methods PennyChest calls.
pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const DETECT: &str = "importer/detect";
    pub const PARSE: &str = "importer/parse";
    pub const STATEMENT_INFO: &str = "importer/statement_info";
    pub const EXPORT: &str = "exporter/export";
    pub const SHUTDOWN: &str = "shutdown";
}

/// JSON-RPC error codes. The standard ones, and one for a problem to show the user.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The plugin understood the request but can't do it, e.g. a statement it can't read.
    /// Its message is shown to the user as it is, so it should say what's wrong.
    pub const FAILED: i64 = -32000;
}

/// Bytes as base64 in JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Base64(pub Vec<u8>);

impl Serialize for Base64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Base64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text.as_bytes())
            .map(Base64)
            .map_err(serde::de::Error::custom)
    }
}

// The handshake

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeParams {
    /// The newest version PennyChest speaks.
    pub protocol_version: u32,
}

/// What the plugin answers `initialize` with: who it is and what it provides. A plugin can
/// provide several importers and exporters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handshake {
    pub protocol_version: u32,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub importers: Vec<ImporterInfo>,
    #[serde(default)]
    pub exporters: Vec<ExporterInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImporterInfo {
    /// Unique among everything installed, e.g. "hsbc".
    pub name: String,
    /// Short name for the format, shown on the import page, e.g. "HSBC PDF statements".
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Extensions it handles, without the dot, e.g. `["pdf"]`.
    pub file_types: Vec<String>,
    /// Whether it implements `importer/detect`. If not, PennyChest picks it by extension.
    #[serde(default)]
    pub detects: bool,
    /// Whether it implements `importer/statement_info`.
    #[serde(default)]
    pub statement_info: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExporterInfo {
    pub name: String,
    /// Shown on the export page, e.g. "Beancount ledger".
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// The downloaded file is named `pennychest-<date>.<file_extension>`.
    pub file_extension: String,
    #[serde(default = "octet_stream")]
    pub media_type: String,
}

fn octet_stream() -> String {
    "application/octet-stream".into()
}

// Importing

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectParams {
    pub importer: String,
    pub file: Base64,
    pub filename: String,
}

/// For `importer/parse` and `importer/statement_info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileParams {
    pub importer: String,
    pub file: Base64,
}

/// One transaction read from a statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub date: NaiveDate,
    pub description: String,
    /// Signed from the account's side: negative for money out. At most four decimal places.
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    /// The row as the statement had it, kept as the record of what the bank reported.
    pub raw_content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_number: Option<i64>,
    /// Anything else worth keeping, stored with the import.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parsed {
    pub rows: Vec<Row>,
}

/// What a statement says about its account, so the import page can suggest the account and
/// an opening balance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementInfo {
    /// "current_account", "savings" or "credit_card".
    pub statement_type: String,
    /// e.g. "12-34-56 12345678" or "****1234".
    pub bank_identifier: String,
    /// e.g. "HSBC Current Account Statement".
    pub label: String,
    #[serde(default)]
    pub name_hint: Option<String>,
    #[serde(default)]
    pub period_start: Option<NaiveDate>,
    #[serde(default)]
    pub period_end: Option<NaiveDate>,
    /// Signed for posting: positive for money held in an asset account, negative for a
    /// credit card balance owed.
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub opening_balance: Option<Decimal>,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub closing_balance: Option<Decimal>,
}

// Exporting

/// Everything an exporter can see: a read-only snapshot, so exporters don't depend on how
/// PennyChest stores things.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    pub today: NaiveDate,
    /// Parents before their children.
    pub accounts: Vec<LedgerAccount>,
    /// Oldest first.
    pub transactions: Vec<LedgerTransaction>,
    pub budgets: Vec<LedgerBudget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerAccount {
    pub id: i64,
    pub name: String,
    /// e.g. "Expenses:Food:Groceries".
    pub full_path: String,
    /// "asset", "liability", "income", "expense" or "equity".
    #[serde(rename = "type")]
    pub kind: String,
    pub currency: String,
    pub parent_id: Option<i64>,
    pub bank_identifier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerTransaction {
    pub id: i64,
    pub date: NaiveDate,
    pub description: String,
    /// "confirmed", or "pending" while an import is still to be reviewed.
    pub status: String,
    pub currency: String,
    /// In the order they were made; they sum to zero.
    pub postings: Vec<LedgerPosting>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerPosting {
    pub id: i64,
    pub account_id: i64,
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerBudget {
    /// Covers this account and everything below it.
    pub account_id: i64,
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    /// "monthly" or "annual".
    pub period: String,
    pub created_on: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportParams {
    pub exporter: String,
    pub ledger: Ledger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exported {
    pub file: Base64,
}

// JSON-RPC framing

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn files_are_base64_and_amounts_decimal_strings() {
        let params = FileParams {
            importer: "x".into(),
            file: Base64(b"Date,Amount\n".to_vec()),
        };
        assert_eq!(
            serde_json::to_value(&params).unwrap(),
            json!({"importer": "x", "file": "RGF0ZSxBbW91bnQK"})
        );
        let row: Row = serde_json::from_value(json!({
            "date": "2026-09-20",
            "description": "TESCO",
            "amount": "-12.50",
            "raw_content": "20/09/2026,TESCO,-12.50",
        }))
        .unwrap();
        assert_eq!(row.amount, "-12.50".parse::<Decimal>().unwrap());
        assert_eq!(serde_json::to_value(&row).unwrap()["amount"], "-12.50");
        // A number would lose precision in some languages, so it's refused.
        let number = json!({"date": "2026-09-20", "description": "x", "amount": 1.1,
                            "raw_content": ""});
        assert!(serde_json::from_value::<Row>(number).is_err());
    }
}
