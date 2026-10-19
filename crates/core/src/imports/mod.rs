//! Importing statements: the importer interface, the built-in CSV importer, and saved CSV
//! column templates.
//!
//! Importers turn a statement file into rows. The CSV importer is built in; plugins
//! provide others behind the same `Importer` trait.

pub mod batches;
pub mod csv;
pub mod matching;
pub mod templates;

use std::sync::Arc;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;

use crate::Money;

/// One transaction read from a statement.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedRow {
    pub date: NaiveDate,
    pub description: String,
    /// Signed from the account's side: negative for money out.
    pub amount: Money,
    /// The row as the statement had it, kept with the import as the record of what the bank
    /// reported.
    pub raw_content: String,
    pub line_number: Option<i64>,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// What an importer can read from a statement before importing it, so the import page can
/// suggest the account and an opening balance.
#[derive(Debug, Clone, PartialEq)]
pub struct StatementInfo {
    /// "current_account", "savings" or "credit_card"
    pub statement_type: String,
    /// e.g. "12-34-56 12345678" or "****1234"
    pub bank_identifier: String,
    /// e.g. "HSBC Current Account Statement"
    pub label: String,
    /// A name for a new account, e.g. "HSBC Advance"
    pub name_hint: Option<String>,
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
    /// Signed for posting: positive for money held in an asset account, negative for a
    /// credit card balance owed. Kept as the statement wrote it (e.g. "-10.00").
    pub opening_balance: Option<Decimal>,
    pub closing_balance: Option<Decimal>,
}

pub trait Importer: Send + Sync {
    fn name(&self) -> &str;
    /// Short name for the format, e.g. "HSBC PDF statements".
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn file_types(&self) -> Vec<String>;

    /// Whether this importer recognises the file. By default, by its extension.
    fn detect(&self, _content: &[u8], filename: &str) -> bool {
        handles_extension(&self.file_types(), filename)
    }

    /// The statement's rows, or why the file can't be read.
    fn parse(&self, content: &[u8]) -> Result<Vec<ImportedRow>, String>;

    /// The account and summary details, for formats that carry them.
    fn statement_info(&self, _content: &[u8]) -> Option<StatementInfo> {
        None
    }
}

fn extension(filename: &str) -> Option<String> {
    filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
}

fn handles_extension(file_types: &[String], filename: &str) -> bool {
    extension(filename).is_some_and(|ext| file_types.contains(&ext))
}

/// An importer as `/api/imports/importers` lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImporterInfo {
    pub name: String,
    pub label: String,
    pub description: String,
    pub file_types: Vec<String>,
}

/// The importers available: the built-in CSV importer and any from plugins.
#[derive(Clone)]
pub struct Importers(Vec<Arc<dyn Importer>>);

impl Default for Importers {
    fn default() -> Self {
        Self(vec![Arc::new(csv::CsvImporter)])
    }
}

impl Importers {
    pub fn new(importers: Vec<Arc<dyn Importer>>) -> Self {
        Self(importers)
    }

    /// Every importer, in order.
    pub fn all(&self) -> Vec<Arc<dyn Importer>> {
        self.0.clone()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Importer>> {
        self.0.iter().find(|i| i.name() == name).cloned()
    }

    pub fn list(&self) -> Vec<ImporterInfo> {
        self.0
            .iter()
            .map(|i| ImporterInfo {
                name: i.name().to_owned(),
                label: if i.label().is_empty() {
                    i.name().to_owned()
                } else {
                    i.label().to_owned()
                },
                description: i.description().to_owned(),
                file_types: i.file_types(),
            })
            .collect()
    }

    /// The importer for a file: the first that recognises it, else the first that handles its
    /// extension (so its `parse` can say what's wrong). The generic CSV importer is tried
    /// last, so a bank's own importer for CSV files wins over it.
    pub fn find(&self, content: &[u8], filename: &str) -> Option<Arc<dyn Importer>> {
        let mut ordered: Vec<&Arc<dyn Importer>> = self.0.iter().collect();
        ordered.sort_by_key(|i| i.name() == csv::NAME);
        ordered
            .iter()
            .find(|i| i.detect(content, filename))
            .or_else(|| {
                ordered
                    .iter()
                    .find(|i| handles_extension(&i.file_types(), filename))
            })
            .map(|i| Arc::clone(i))
    }

    /// Every file extension some importer handles, e.g. ".csv", sorted.
    pub fn supported_extensions(&self) -> Vec<String> {
        let mut extensions: Vec<String> = self
            .0
            .iter()
            .flat_map(|i| i.file_types())
            .map(|ext| format!(".{ext}"))
            .collect();
        extensions.sort();
        extensions.dedup();
        extensions
    }
}

/// Whether a filename looks like a CSV file the CSV importer could take with a mapping.
pub fn is_csv(filename: &str) -> bool {
    matches!(extension(filename).as_deref(), Some("csv" | "txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    impl Importer for Fake {
        fn name(&self) -> &str {
            "fakebank"
        }
        fn label(&self) -> &str {
            "Fake Bank statements"
        }
        fn description(&self) -> &str {
            ""
        }
        fn file_types(&self) -> Vec<String> {
            vec!["fake".into(), "csv".into()]
        }
        fn detect(&self, content: &[u8], _filename: &str) -> bool {
            content.starts_with(b"FAKEBANK")
        }
        fn parse(&self, _content: &[u8]) -> Result<Vec<ImportedRow>, String> {
            Ok(vec![])
        }
    }

    fn importers() -> Importers {
        Importers::new(vec![Arc::new(csv::CsvImporter), Arc::new(Fake)])
    }

    #[test]
    fn finds_by_content_then_extension_with_csv_last() {
        let importers = importers();
        let csv = b"Date,Description,Amount\n03/07/2026,TESCO,-4.00\n";
        assert_eq!(importers.find(csv, "july.csv").unwrap().name(), "csv");
        assert_eq!(
            importers.find(b"FAKEBANK", "july.csv").unwrap().name(),
            "fakebank"
        );
        assert_eq!(
            importers.find(b"junk", "other.fake").unwrap().name(),
            "fakebank"
        );
        // An unrecognised CSV goes to the first importer handling .csv, the plugin here.
        assert_eq!(
            importers.find(b"junk", "other.csv").unwrap().name(),
            "fakebank"
        );
        assert!(importers.find(b"\xff\xd8", "photo.jpg").is_none());
    }

    #[test]
    fn lists_extensions() {
        assert_eq!(
            importers().supported_extensions(),
            [".csv", ".fake", ".txt"]
        );
    }
}
