//! Transactions and their postings: the double-entry ledger.
//!
//! Every transaction's postings sum to zero. That's guaranteed by construction: a transaction
//! can only be written from `Balanced` postings, and `Balanced::new` is the only way to make
//! them.

use std::collections::HashMap;
use std::fmt;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize, Serializer};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection, SqlitePool};

use crate::Money;
use crate::lookups::CategorisationSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum Status {
    Pending,
    Confirmed,
}

impl Status {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "confirmed" => Some(Self::Confirmed),
            _ => None,
        }
    }
}

/// One side of a transaction, before it's written.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPosting {
    pub account_id: i64,
    pub amount: Money,
    pub categorised_by: CategorisationSource,
    pub rule_id: Option<i64>,
}

/// Postings that sum to zero, at least two of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Balanced(Vec<NewPosting>);

impl Balanced {
    pub fn new(postings: Vec<NewPosting>) -> Result<Self, LedgerError> {
        let total: Money = postings.iter().map(|p| p.amount).sum();
        if !total.is_zero() {
            return Err(LedgerError::Unbalanced(total));
        }
        if postings.len() < 2 {
            return Err(LedgerError::TooFewPostings);
        }
        Ok(Self(postings))
    }

    pub fn postings(&self) -> &[NewPosting] {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct NewTransaction {
    pub date: NaiveDate,
    pub description: String,
    pub status: Status,
    pub currency: String,
    pub import_batch_id: Option<i64>,
    pub import_metadata: Option<serde_json::Value>,
    pub postings: Balanced,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transaction {
    pub id: i64,
    pub date: NaiveDate,
    pub description: String,
    pub status: Status,
    pub currency: String,
    pub import_batch_id: Option<i64>,
    #[serde(serialize_with = "iso_timestamp")]
    pub created_at: String,
    pub postings: Vec<Posting>,
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Posting {
    pub id: i64,
    pub transaction_id: i64,
    pub account_id: i64,
    pub amount: Money,
    pub categorised_by_id: i64,
    pub rule_id: Option<i64>,
    pub account_full_path: Option<String>,
}

/// SQLite's `YYYY-MM-DD HH:MM:SS` as the API writes times: `YYYY-MM-DDTHH:MM:SS`.
fn iso_timestamp<S: Serializer>(stored: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&stored.replacen(' ', "T", 1))
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("Transaction not found")]
    NotFound,
    #[error("Postings must sum to zero, got {}", .0.to_decimal().normalize())]
    Unbalanced(Money),
    #[error("Transaction must have at least 2 postings")]
    TooFewPostings,
    #[error("Account {0} not found")]
    AccountNotFound(i64),
    #[error("Rule {0} not found")]
    RuleNotFound(i64),
    #[error("Category not found")]
    CategoryNotFound,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Writes a transaction and its postings. The caller commits.
pub async fn insert(db: &mut SqliteConnection, new: &NewTransaction) -> Result<i64, LedgerError> {
    for posting in new.postings.postings() {
        let account: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = ?)")
                .bind(posting.account_id)
                .fetch_one(&mut *db)
                .await?;
        if !account {
            return Err(LedgerError::AccountNotFound(posting.account_id));
        }
        if let Some(rule_id) = posting.rule_id {
            let rule: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rules WHERE id = ?)")
                .bind(rule_id)
                .fetch_one(&mut *db)
                .await?;
            if !rule {
                return Err(LedgerError::RuleNotFound(rule_id));
            }
        }
    }

    let metadata = new.import_metadata.as_ref().map(ToString::to_string);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (date, description, status, currency, import_batch_id,
                                   import_metadata)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(new.date)
    .bind(&new.description)
    .bind(new.status)
    .bind(&new.currency)
    .bind(new.import_batch_id)
    .bind(metadata)
    .fetch_one(&mut *db)
    .await?;
    for posting in new.postings.postings() {
        sqlx::query(
            "INSERT INTO postings (transaction_id, account_id, amount, categorised_by_id, rule_id)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(posting.account_id)
        .bind(posting.amount)
        .bind(posting.categorised_by as i64)
        .bind(posting.rule_id)
        .execute(&mut *db)
        .await?;
    }
    Ok(id)
}

pub async fn create(db: &SqlitePool, new: &NewTransaction) -> Result<Transaction, LedgerError> {
    let mut tx = db.begin().await?;
    let id = insert(&mut tx, new).await?;
    tx.commit().await?;
    get(db, id).await?.ok_or(LedgerError::NotFound)
}

#[derive(sqlx::FromRow)]
struct TransactionRow {
    id: i64,
    date: NaiveDate,
    description: String,
    status: Status,
    currency: String,
    import_batch_id: Option<i64>,
    created_at: String,
}

const TRANSACTION_COLUMNS: &str =
    "t.id, t.date, t.description, t.status, t.currency, t.import_batch_id, t.created_at";

pub async fn get(db: &SqlitePool, id: i64) -> Result<Option<Transaction>, sqlx::Error> {
    let row: Option<TransactionRow> = sqlx::query_as(&format!(
        "SELECT {TRANSACTION_COLUMNS} FROM transactions t WHERE t.id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(with_postings(db, vec![row]).await?.pop())
}

/// Which transactions to list or count. Ids of 0 are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filters {
    pub status: Option<String>,
    /// Transactions with a posting on this account.
    pub account_id: Option<i64>,
    /// Transactions in this category or any beneath it (Food includes Food:Groceries).
    pub category_id: Option<i64>,
    pub batch_id: Option<i64>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    /// Text the description contains, ignoring case.
    pub search: Option<String>,
}

async fn push_filters<'a>(
    db: &SqlitePool,
    query: &mut QueryBuilder<'a, Sqlite>,
    filters: &'a Filters,
) -> Result<(), LedgerError> {
    query.push(" WHERE 1 = 1");
    if let Some(status) = filters.status.as_deref().filter(|s| !s.is_empty()) {
        query.push(" AND t.status = ").push_bind(status);
    }
    if let Some(from) = filters.date_from {
        query.push(" AND t.date >= ").push_bind(from);
    }
    if let Some(to) = filters.date_to {
        query.push(" AND t.date <= ").push_bind(to);
    }
    if let Some(search) = filters.search.as_deref().filter(|s| !s.is_empty()) {
        query
            .push(" AND lower(t.description) LIKE '%' || lower(")
            .push_bind(search)
            .push(") || '%'");
    }
    if let Some(account_id) = filters.account_id.filter(|id| *id != 0) {
        query
            .push(" AND t.id IN (SELECT transaction_id FROM postings WHERE account_id = ")
            .push_bind(account_id)
            .push(")");
    }
    if let Some(batch_id) = filters.batch_id.filter(|id| *id != 0) {
        query.push(" AND t.import_batch_id = ").push_bind(batch_id);
    }
    if let Some(category_id) = filters.category_id.filter(|id| *id != 0) {
        let path: Option<String> =
            sqlx::query_scalar("SELECT full_path FROM accounts WHERE id = ?")
                .bind(category_id)
                .fetch_optional(db)
                .await?;
        let path = path.ok_or(LedgerError::CategoryNotFound)?;
        let prefix = format!("{path}:");
        let prefix_len = i64::try_from(prefix.chars().count()).unwrap_or(i64::MAX);
        query
            .push(
                " AND t.id IN (SELECT p.transaction_id FROM postings p
                   JOIN accounts a ON a.id = p.account_id WHERE a.full_path = ",
            )
            .push_bind(path)
            .push(" OR substr(a.full_path, 1, ")
            .push_bind(prefix_len)
            .push(") = ")
            .push_bind(prefix)
            .push(")");
    }
    Ok(())
}

/// The newest first, a page at a time (pages count from 1).
pub async fn list(
    db: &SqlitePool,
    filters: &Filters,
    page: u32,
    per_page: u32,
) -> Result<Vec<Transaction>, LedgerError> {
    let mut query = QueryBuilder::new(format!("SELECT {TRANSACTION_COLUMNS} FROM transactions t"));
    push_filters(db, &mut query, filters).await?;
    query
        .push(" ORDER BY t.date DESC, t.id DESC LIMIT ")
        .push_bind(i64::from(per_page))
        .push(" OFFSET ")
        .push_bind(i64::from(page.saturating_sub(1)) * i64::from(per_page));
    let rows: Vec<TransactionRow> = query.build_query_as().fetch_all(db).await?;
    Ok(with_postings(db, rows).await?)
}

pub async fn count(db: &SqlitePool, filters: &Filters) -> Result<i64, LedgerError> {
    let mut query = QueryBuilder::new("SELECT COUNT(*) FROM transactions t");
    push_filters(db, &mut query, filters).await?;
    Ok(query.build_query_scalar().fetch_one(db).await?)
}

async fn with_postings(
    db: &SqlitePool,
    rows: Vec<TransactionRow>,
) -> Result<Vec<Transaction>, sqlx::Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = QueryBuilder::new(
        "SELECT p.id, p.transaction_id, p.account_id, p.amount, p.categorised_by_id, p.rule_id,
                a.full_path AS account_full_path
         FROM postings p LEFT JOIN accounts a ON a.id = p.account_id
         WHERE p.transaction_id IN (",
    );
    let mut ids = query.separated(", ");
    for row in &rows {
        ids.push_bind(row.id);
    }
    query.push(") ORDER BY p.id");
    let postings: Vec<Posting> = query.build_query_as().fetch_all(db).await?;
    let mut by_transaction: HashMap<i64, Vec<Posting>> = HashMap::new();
    for posting in postings {
        by_transaction
            .entry(posting.transaction_id)
            .or_default()
            .push(posting);
    }
    Ok(rows
        .into_iter()
        .map(|row| Transaction {
            postings: by_transaction.remove(&row.id).unwrap_or_default(),
            id: row.id,
            date: row.date,
            description: row.description,
            status: row.status,
            currency: row.currency,
            import_batch_id: row.import_batch_id,
            created_at: row.created_at,
        })
        .collect())
}

/// Changes to a transaction's own details. Fields left out stay as they are.
#[derive(Debug, Clone, Default)]
pub struct TransactionUpdate {
    pub date: Option<NaiveDate>,
    pub description: Option<String>,
    pub status: Option<Status>,
}

pub async fn update(
    db: &SqlitePool,
    id: i64,
    changes: TransactionUpdate,
) -> Result<Transaction, LedgerError> {
    let result = sqlx::query(
        "UPDATE transactions SET date = coalesce(?, date), description = coalesce(?, description),
         status = coalesce(?, status) WHERE id = ?",
    )
    .bind(changes.date)
    .bind(changes.description)
    .bind(changes.status)
    .bind(id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(LedgerError::NotFound);
    }
    get(db, id).await?.ok_or(LedgerError::NotFound)
}

/// Deletes a transaction and its postings. The statement rows it was imported from stay with
/// their import as the record of what the bank reported; they, card taps, and transactions
/// that pointed at it as a transfer pair or duplicate stop pointing at it.
pub async fn delete(db: impl sqlx::SqliteExecutor<'_>, id: i64) -> Result<(), LedgerError> {
    let result = sqlx::query("DELETE FROM transactions WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(LedgerError::NotFound);
    }
    Ok(())
}

pub async fn confirm(db: &SqlitePool, id: i64) -> Result<Transaction, LedgerError> {
    update(
        db,
        id,
        TransactionUpdate {
            status: Some(Status::Confirmed),
            ..TransactionUpdate::default()
        },
    )
    .await
}

/// Confirms the pending ones among `ids`. Returns how many were.
pub async fn confirm_many(db: &SqlitePool, ids: &[i64]) -> Result<u64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut query = QueryBuilder::new(
        "UPDATE transactions SET status = 'confirmed' WHERE status = 'pending' AND id IN (",
    );
    let mut list = query.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    query.push(")");
    Ok(query.build().execute(db).await?.rows_affected())
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{self, AccountType, NewAccount};
    use crate::db;

    fn m(s: &str) -> Money {
        s.parse().unwrap()
    }

    fn posting(account_id: i64, amount: &str) -> NewPosting {
        NewPosting {
            account_id,
            amount: m(amount),
            categorised_by: CategorisationSource::Manual,
            rule_id: None,
        }
    }

    async fn setup() -> (SqlitePool, i64, i64) {
        let pool = db::open_in_memory().await.unwrap();
        let mut ids = Vec::new();
        for (path, kind) in [
            ("Assets:Current", AccountType::Asset),
            ("Expenses:Food", AccountType::Expense),
        ] {
            let account = accounts::create(
                &pool,
                NewAccount {
                    name: path.into(),
                    full_path: path.into(),
                    parent_id: None,
                    kind,
                    currency: "GBP".into(),
                    bank_identifier: None,
                    icon: None,
                },
            )
            .await
            .unwrap();
            ids.push(account.id);
        }
        (pool, ids[0], ids[1])
    }

    fn spend(bank: i64, food: i64, date: &str, description: &str, amount: &str) -> NewTransaction {
        NewTransaction {
            date: date.parse().unwrap(),
            description: description.into(),
            status: Status::Pending,
            currency: "GBP".into(),
            import_batch_id: None,
            import_metadata: None,
            postings: Balanced::new(vec![
                posting(food, amount),
                posting(bank, &format!("-{amount}")),
            ])
            .unwrap(),
        }
    }

    #[test]
    fn balanced_postings_sum_to_zero_and_number_at_least_two() {
        assert!(Balanced::new(vec![posting(1, "50"), posting(2, "-50")]).is_ok());
        assert!(
            Balanced::new(vec![
                posting(1, "0.1"),
                posting(2, "0.2"),
                posting(3, "-0.3")
            ])
            .is_ok()
        );
        let unbalanced = Balanced::new(vec![posting(1, "50.00"), posting(2, "-30.00")]);
        assert_eq!(
            unbalanced.unwrap_err().to_string(),
            "Postings must sum to zero, got 20"
        );
        assert!(matches!(
            Balanced::new(vec![posting(1, "0")]),
            Err(LedgerError::TooFewPostings)
        ));
        assert!(matches!(
            Balanced::new(vec![]),
            Err(LedgerError::TooFewPostings)
        ));
    }

    #[tokio::test]
    async fn creates_and_reads_back() {
        let (pool, bank, food) = setup().await;
        let created = create(&pool, &spend(bank, food, "2026-01-15", "Tesco", "50"))
            .await
            .unwrap();
        assert_eq!(created.status, Status::Pending);
        assert_eq!(created.postings.len(), 2);
        assert_eq!(
            created.postings[0].account_full_path.as_deref(),
            Some("Expenses:Food")
        );
        assert_eq!(created.postings[1].amount, m("-50"));
        assert_eq!(get(&pool, created.id).await.unwrap(), Some(created));
    }

    #[tokio::test]
    async fn refuses_missing_accounts_and_rules() {
        let (pool, bank, _) = setup().await;
        let mut new = spend(bank, 999, "2026-01-15", "x", "1");
        assert!(matches!(
            create(&pool, &new).await,
            Err(LedgerError::AccountNotFound(999))
        ));
        new.postings = Balanced::new(vec![
            NewPosting {
                rule_id: Some(5),
                ..posting(bank, "1")
            },
            posting(bank, "-1"),
        ])
        .unwrap();
        assert!(matches!(
            create(&pool, &new).await,
            Err(LedgerError::RuleNotFound(5))
        ));
        let none: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(none, 0, "nothing is left half-written");
    }

    #[tokio::test]
    async fn lists_newest_first_with_filters_and_pages() {
        let (pool, bank, food) = setup().await;
        for (date, description) in [
            ("2026-01-10", "Tesco"),
            ("2026-02-10", "TESCO EXPRESS"),
            ("2026-02-11", "Petrol"),
        ] {
            create(&pool, &spend(bank, food, date, description, "5"))
                .await
                .unwrap();
        }
        let all = list(&pool, &Filters::default(), 1, 50).await.unwrap();
        let descriptions: Vec<&str> = all.iter().map(|t| t.description.as_str()).collect();
        assert_eq!(descriptions, ["Petrol", "TESCO EXPRESS", "Tesco"]);

        let tesco = Filters {
            search: Some("tesco".into()),
            ..Filters::default()
        };
        assert_eq!(count(&pool, &tesco).await.unwrap(), 2);
        let february = Filters {
            date_from: "2026-02-01".parse().ok(),
            ..tesco.clone()
        };
        assert_eq!(count(&pool, &february).await.unwrap(), 1);

        let second_page = list(&pool, &Filters::default(), 2, 2).await.unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].description, "Tesco");

        let missing = Filters {
            category_id: Some(999),
            ..Filters::default()
        };
        assert!(matches!(
            count(&pool, &missing).await,
            Err(LedgerError::CategoryNotFound)
        ));
    }

    #[tokio::test]
    async fn updates_confirms_and_deletes() {
        let (pool, bank, food) = setup().await;
        let first = create(&pool, &spend(bank, food, "2026-01-10", "a", "1"))
            .await
            .unwrap();
        let second = create(&pool, &spend(bank, food, "2026-01-11", "b", "2"))
            .await
            .unwrap();

        let renamed = update(
            &pool,
            first.id,
            TransactionUpdate {
                description: Some("A".into()),
                ..TransactionUpdate::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (renamed.description.as_str(), renamed.date),
            ("A", first.date)
        );

        assert_eq!(
            confirm(&pool, first.id).await.unwrap().status,
            Status::Confirmed
        );
        assert_eq!(
            confirm_many(&pool, &[first.id, second.id, 999])
                .await
                .unwrap(),
            1
        );

        delete(&pool, first.id).await.unwrap();
        assert!(matches!(
            delete(&pool, first.id).await,
            Err(LedgerError::NotFound)
        ));
        let postings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM postings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(postings, 2);
    }

    #[test]
    fn writes_times_as_the_api_did() {
        let json = serde_json::to_value(Transaction {
            id: 1,
            date: "2026-01-15".parse().unwrap(),
            description: String::new(),
            status: Status::Pending,
            currency: "GBP".into(),
            import_batch_id: None,
            created_at: "2026-10-14 08:48:10".into(),
            postings: vec![],
        })
        .unwrap();
        assert_eq!(json["created_at"], "2026-10-14T08:48:10");
        assert_eq!(json["date"], "2026-01-15");
        assert_eq!(json["status"], "pending");
    }
}
