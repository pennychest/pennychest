//! Exporting the user's data: the ledger in the formats of the installed exporters, and a
//! complete copy of the database.
//!
//! Exporters get a [`Ledger`]: a read-only snapshot in the plugin protocol's own types, so they
//! don't depend on how PennyChest stores things, and the plugin host can send it to a plugin
//! as it is. The database backup isn't an exporter: it's a copy of PennyChest itself.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{NaiveDate, NaiveDateTime};
use pennychest_plugin::{Ledger, LedgerAccount, LedgerBudget, LedgerPosting, LedgerTransaction};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::Money;
use crate::auth::random_token;

/// Turns the ledger into a file in another format.
pub trait Exporter: Send + Sync {
    fn name(&self) -> &str;
    /// Shown on the export page, e.g. "Beancount ledger".
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    /// The download is named `pennychest-<date>.<file_extension>`.
    fn file_extension(&self) -> &str;
    fn media_type(&self) -> &str;

    /// The ledger in this exporter's format, or what went wrong, for the user.
    fn export(&self, ledger: &Ledger) -> Result<Vec<u8>, String>;
}

/// An exporter as `/api/export` lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExporterInfo {
    pub name: String,
    pub label: String,
    pub description: String,
    pub file_extension: String,
}

/// The exporters available, all from plugins.
#[derive(Clone, Default)]
pub struct Exporters(Vec<Arc<dyn Exporter>>);

impl Exporters {
    pub fn new(exporters: Vec<Arc<dyn Exporter>>) -> Self {
        Self(exporters)
    }

    /// Every exporter, in order.
    pub fn all(&self) -> Vec<Arc<dyn Exporter>> {
        self.0.clone()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Exporter>> {
        self.0.iter().find(|e| e.name() == name).cloned()
    }

    pub fn list(&self) -> Vec<ExporterInfo> {
        self.0
            .iter()
            .map(|e| ExporterInfo {
                name: e.name().to_owned(),
                label: if e.label().is_empty() {
                    e.name().to_owned()
                } else {
                    e.label().to_owned()
                },
                description: e.description().to_owned(),
                file_extension: e.file_extension().to_owned(),
            })
            .collect()
    }
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    id: i64,
    name: String,
    full_path: String,
    #[sqlx(rename = "type")]
    kind: String,
    currency: String,
    parent_id: Option<i64>,
    bank_identifier: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PostingRow {
    transaction_id: i64,
    id: i64,
    account_id: i64,
    amount: Money,
}

/// Everything an exporter can see, as of `today`: accounts parents first, transactions oldest
/// first with their postings in the order they were made, and budgets.
pub async fn load_ledger(db: &SqlitePool, today: NaiveDate) -> Result<Ledger, sqlx::Error> {
    let accounts: Vec<AccountRow> = sqlx::query_as(
        "SELECT id, name, full_path, type, currency, parent_id, bank_identifier
         FROM accounts ORDER BY full_path",
    )
    .fetch_all(db)
    .await?;
    let transactions: Vec<(i64, NaiveDate, String, String, String)> = sqlx::query_as(
        "SELECT id, date, description, status, currency FROM transactions ORDER BY date, id",
    )
    .fetch_all(db)
    .await?;
    let postings: Vec<PostingRow> = sqlx::query_as(
        "SELECT transaction_id, id, account_id, amount FROM postings ORDER BY transaction_id, id",
    )
    .fetch_all(db)
    .await?;
    let budgets: Vec<(i64, Money, String, NaiveDateTime)> = sqlx::query_as(
        "SELECT b.account_id, b.amount, p.name, b.created_at FROM budgets b
         JOIN budget_periods p ON p.id = b.period_id ORDER BY b.id",
    )
    .fetch_all(db)
    .await?;

    let mut by_transaction: std::collections::HashMap<i64, Vec<LedgerPosting>> =
        std::collections::HashMap::new();
    for p in postings {
        by_transaction
            .entry(p.transaction_id)
            .or_default()
            .push(LedgerPosting {
                id: p.id,
                account_id: p.account_id,
                amount: p.amount.to_decimal(),
            });
    }
    Ok(Ledger {
        today,
        accounts: accounts
            .into_iter()
            .map(|a| LedgerAccount {
                id: a.id,
                name: a.name,
                full_path: a.full_path,
                kind: a.kind,
                currency: a.currency,
                parent_id: a.parent_id,
                bank_identifier: a.bank_identifier,
            })
            .collect(),
        transactions: transactions
            .into_iter()
            .map(
                |(id, date, description, status, currency)| LedgerTransaction {
                    id,
                    date,
                    description,
                    status,
                    currency,
                    postings: by_transaction.remove(&id).unwrap_or_default(),
                },
            )
            .collect(),
        budgets: budgets
            .into_iter()
            .map(|(account_id, amount, period, created_at)| LedgerBudget {
                account_id,
                amount: amount.to_decimal(),
                period,
                created_on: Some(created_at.date()),
            })
            .collect(),
    })
}

/// How big the database is, in bytes.
pub async fn database_size(db: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()")
        .fetch_one(db)
        .await
}

/// A consistent copy of the whole database, safe to take while the app is in use: SQLite
/// writes it from a single read transaction. It opens as a PennyChest database as it is.
pub async fn backup(db: &SqlitePool) -> Result<Vec<u8>, sqlx::Error> {
    let path: PathBuf =
        std::env::temp_dir().join(format!("pennychest-backup-{}.db", random_token(12)));
    // A string literal: VACUUM INTO doesn't take a bound parameter here.
    let literal = path.to_string_lossy().replace('\'', "''");
    let copied = sqlx::query(&format!("VACUUM INTO '{literal}'"))
        .execute(db)
        .await;
    let read = match copied {
        Ok(_) => tokio::fs::read(&path).await.map_err(sqlx::Error::Io),
        Err(e) => Err(e),
    };
    // The copy is only for the download.
    let _ = tokio::fs::remove_file(&path).await;
    read
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn the_backup_is_a_database() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("pennychest.db")).await.unwrap();
        let copy = backup(&pool).await.unwrap();
        assert!(copy.starts_with(b"SQLite format 3\0"));
    }
}
