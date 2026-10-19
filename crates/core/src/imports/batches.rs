//! Importing a statement as a batch of pending transactions, and reviewing, confirming,
//! re-categorising and deleting what was imported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, NaiveDate};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};

use super::csv::{self, Columns};
use super::templates::{self, TemplateError};
use super::{ImportedRow, Importer, Importers};
use crate::Money;
use crate::accounts::{self, AccountType};
use crate::actions::Source;
use crate::ai::categorise;
use crate::ai::config::{self, Task};
use crate::ai::http::Transport;
use crate::learned::Learned;
use crate::ledger::{self, Balanced, LedgerError, NewPosting, NewTransaction, Status};
use crate::lookups::CategorisationSource;
use crate::rules::RuleSet;

pub const UNCATEGORISED_PATH: &str = "Expenses:Uncategorised";
pub const TRANSFERS_PENDING_PATH: &str = "Assets:Transfers:Pending";
/// How far apart, in days, the dates of a duplicate or the two sides of a transfer can be.
const MATCH_DAYS: i64 = 3;

/// A statement to import.
pub struct Upload {
    pub account_id: i64,
    pub importer_name: String,
    pub filename: String,
    pub content: Vec<u8>,
    /// CSV only: which column holds each field, for layouts the importer doesn't recognise.
    pub csv_columns: Option<Columns>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportOutcome {
    pub batch_id: i64,
    pub file_name: String,
    pub transaction_count: usize,
    /// Categorised by rules.
    pub categorised_count: usize,
    pub uncategorised_count: usize,
    /// Categorised after the rules: from the user's history or by the model.
    pub ai_categorised_count: usize,
    /// Categorised from the user's own history.
    pub learned_count: usize,
    pub ai_error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("Account not found")]
    AccountNotFound,
    #[error("Importer '{0}' isn't installed")]
    ImporterMissing(String),
    #[error("This file has already been imported (batch #{0})")]
    AlreadyImported(i64),
    #[error("{0}")]
    Unreadable(String),
    #[error("No transactions found in file")]
    Empty,
    #[error("Import batch not found")]
    BatchNotFound,
    #[error("Transaction not found")]
    TransactionNotFound,
    #[error("Target account not found")]
    TargetNotFound,
    #[error("Cannot find posting to categorise")]
    NothingToCategorise,
    #[error("File not found")]
    FileNotFound,
    #[error("File no longer available on disk")]
    FileGone,
    #[error("couldn't save the uploaded file: {0}")]
    Save(#[from] std::io::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<TemplateError> for ImportError {
    fn from(err: TemplateError) -> Self {
        match err {
            TemplateError::Database(err) => Self::Database(err),
            other => Self::Unreadable(other.to_string()),
        }
    }
}

impl From<LedgerError> for ImportError {
    fn from(err: LedgerError) -> Self {
        match err {
            LedgerError::Database(err) => Self::Database(err),
            other => Self::Unreadable(other.to_string()),
        }
    }
}

/// The filename to store an upload under: its last path component only, so a name like
/// `../../x` can't write outside the upload directory.
fn safe_name(filename: &str) -> String {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    if name.is_empty() || name == "." || name == ".." {
        "statement".into()
    } else {
        name
    }
}

/// Imports a statement: one pending transaction per row, between the statement's account and
/// the category a rule picks (or Expenses:Uncategorised), with transfers between the user's
/// own accounts paired up and exact duplicates of transactions already there flagged.
pub async fn import(
    db: &SqlitePool,
    upload_dir: &Path,
    importers: &Importers,
    learned: &Learned,
    http: &dyn Transport,
    upload: Upload,
) -> Result<ImportOutcome, ImportError> {
    let mut conn = db.acquire().await?;
    let account = accounts::get(&mut conn, upload.account_id)
        .await?
        .ok_or(ImportError::AccountNotFound)?;
    let importer = importers
        .get(&upload.importer_name)
        .ok_or_else(|| ImportError::ImporterMissing(upload.importer_name.clone()))?;

    let hash = hex::encode(Sha256::digest(&upload.content));
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM import_batches WHERE file_hash = ?")
            .bind(&hash)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(id) = existing {
        return Err(ImportError::AlreadyImported(id));
    }
    drop(conn);

    let content = Arc::new(upload.content);
    let (rows, info) = parse(importer, Arc::clone(&content), upload.csv_columns).await?;
    if rows.is_empty() {
        return Err(ImportError::Empty);
    }

    tokio::fs::create_dir_all(upload_dir).await?;
    let file_path = upload_dir.join(format!("{hash}_{}", safe_name(&upload.filename)));
    tokio::fs::write(&file_path, content.as_slice()).await?;

    let result = record(
        db,
        &account,
        &upload.importer_name,
        &upload.filename,
        &hash,
        &file_path,
        rows,
        info,
    )
    .await;
    let mut outcome = match result {
        Ok(outcome) => outcome,
        Err(err) => {
            // Nothing points at the file if the import didn't go in.
            let _ = tokio::fs::remove_file(&file_path).await;
            return Err(err);
        }
    };
    // The model looks for duplicates and transfers exact matching missed, if matching runs
    // automatically; then categorising sees only what's left.
    if let Err(err) = super::matching::after_import(db, http, outcome.batch_id, account.id).await {
        tracing::warn!("Matching an import failed: {err}");
    }
    // And links any card taps the statement now covers.
    let match_with_model = config::runs_automatically(db, Task::Matching)
        .await
        .unwrap_or(false);
    if let Err(err) = crate::taps::reconcile(db, http, match_with_model).await {
        tracing::warn!("Reconciling card taps after an import failed: {err}");
    }
    // Then categorise what rules didn't: from the user's own history first, then with the
    // model if categorising runs automatically. The import stands whatever happens here; the
    // model's problem is reported alongside it and kept in the AI logs.
    let use_ai = config::runs_automatically(db, Task::Categorise)
        .await
        .unwrap_or(false);
    match categorise::uncategorised(
        db,
        http,
        learned,
        Some(outcome.batch_id),
        Source::Import,
        use_ai,
    )
    .await
    {
        Ok(done) => {
            outcome.learned_count = done.learned;
            outcome.ai_categorised_count = done.total();
            outcome.uncategorised_count -= done.total();
            outcome.ai_error = done.error;
        }
        Err(err) => tracing::warn!("Categorising an import failed: {err}"),
    }
    Ok(outcome)
}

/// Reads the rows (and the statement's summary) off the async runtime, since a plugin's
/// importer can take a while.
async fn parse(
    importer: Arc<dyn Importer>,
    content: Arc<Vec<u8>>,
    csv_columns: Option<Columns>,
) -> Result<(Vec<ImportedRow>, Option<super::StatementInfo>), ImportError> {
    let is_csv = importer.name() == csv::NAME;
    tokio::task::spawn_blocking(move || {
        let rows = match csv_columns {
            Some(columns) if is_csv => {
                let (headers, _) = csv::read(&content).map_err(ImportError::Unreadable)?;
                templates::check_columns(&columns, headers.len())?;
                csv::parse_mapped(&content, &columns)
            }
            _ => importer.parse(&content),
        }
        .map_err(ImportError::Unreadable)?;
        Ok((rows, importer.statement_info(&content)))
    })
    .await
    .map_err(|e| ImportError::Unreadable(e.to_string()))?
}

/// The account for uncategorised transactions, made under Expenses if it's missing.
async fn uncategorised(conn: &mut SqliteConnection) -> Result<i64, sqlx::Error> {
    if let Some(account) = accounts::find_by_path(conn, UNCATEGORISED_PATH).await? {
        return Ok(account.id);
    }
    let parent = accounts::find_by_path(conn, "Expenses")
        .await?
        .map(|a| a.id);
    sqlx::query_scalar(
        "INSERT INTO accounts (name, full_path, parent_id, type) VALUES (?, ?, ?, 'expense')
         RETURNING id",
    )
    .bind("Uncategorised")
    .bind(UNCATEGORISED_PATH)
    .bind(parent)
    .fetch_one(conn)
    .await
}

/// A transaction just imported: its id, date, description, and the statement-side amount.
struct NewRow {
    id: i64,
    date: NaiveDate,
    description: String,
    amount: Money,
    /// The category side: routed via Transfers:Pending, or still uncategorised.
    needs_transfer_check: bool,
}

#[allow(clippy::too_many_arguments)]
async fn record(
    db: &SqlitePool,
    account: &accounts::Account,
    importer_name: &str,
    filename: &str,
    hash: &str,
    file_path: &Path,
    rows: Vec<ImportedRow>,
    info: Option<super::StatementInfo>,
) -> Result<ImportOutcome, ImportError> {
    let mut tx = db.begin().await?;
    let uncategorised = uncategorised(&mut tx).await?;
    let pending = accounts::find_by_path(&mut tx, TRANSFERS_PENDING_PATH)
        .await?
        .map(|a| a.id);
    let rules = RuleSet::load(&mut tx).await?;
    let kinds: HashMap<i64, AccountType> = sqlx::query_as("SELECT id, type FROM accounts")
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();

    let balance = |b: Option<rust_decimal::Decimal>| {
        b.and_then(|b| Money::from_decimal(b.round_dp(crate::money::SCALE)).ok())
    };
    let batch_id: i64 = sqlx::query_scalar(
        "INSERT INTO import_batches (importer_name, source_type_id, file_path, file_hash, file_name,
            account_id, period_start, period_end, opening_balance, closing_balance)
         VALUES (?, 1, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(importer_name)
    .bind(file_path.display().to_string())
    .bind(hash)
    .bind(filename)
    .bind(account.id)
    .bind(info.as_ref().and_then(|i| i.period_start))
    .bind(info.as_ref().and_then(|i| i.period_end))
    .bind(balance(info.as_ref().and_then(|i| i.opening_balance)))
    .bind(balance(info.as_ref().and_then(|i| i.closing_balance)))
    .fetch_one(&mut *tx)
    .await?;

    let mut categorised = 0;
    let mut created = Vec::with_capacity(rows.len());
    let transaction_count = rows.len();
    for row in rows {
        let rule = rules.first_match(&row.description);
        let (target, source, rule_id, transfer_rule) = match rule {
            Some(rule) => {
                categorised += 1;
                let target_kind = kinds.get(&rule.target_account_id);
                // A rule pointing at another of the user's own accounts is a transfer: it goes
                // via Transfers:Pending so the other side can be paired with it.
                let is_transfer = matches!(
                    target_kind,
                    Some(AccountType::Asset | AccountType::Liability)
                ) && rule.target_account_id != account.id
                    && Some(rule.target_account_id) != pending;
                match pending.filter(|_| is_transfer) {
                    Some(pending) => (pending, CategorisationSource::Transfer, Some(rule.id), true),
                    None => (
                        rule.target_account_id,
                        CategorisationSource::Rule,
                        Some(rule.id),
                        false,
                    ),
                }
            }
            None => (
                uncategorised,
                CategorisationSource::ImportDefault,
                None,
                false,
            ),
        };
        let postings = Balanced::new(vec![
            NewPosting {
                account_id: account.id,
                amount: row.amount,
                categorised_by: CategorisationSource::ImportDefault,
                rule_id: None,
            },
            NewPosting {
                account_id: target,
                amount: -row.amount,
                categorised_by: source,
                rule_id,
            },
        ])?;
        let id = ledger::insert(
            &mut tx,
            &NewTransaction {
                date: row.date,
                description: row.description.clone(),
                status: Status::Pending,
                currency: account.currency.clone(),
                import_batch_id: Some(batch_id),
                import_metadata: (!row.metadata.is_empty())
                    .then(|| serde_json::Value::Object(row.metadata.clone())),
                postings,
            },
        )
        .await?;
        sqlx::query(
            "INSERT INTO raw_import_rows (batch_id, raw_content, line_number, transaction_id)
             VALUES (?, ?, ?, ?)",
        )
        .bind(batch_id)
        .bind(&row.raw_content)
        .bind(row.line_number)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        created.push(NewRow {
            id,
            date: row.date,
            description: row.description,
            amount: row.amount,
            needs_transfer_check: rule.is_none() || transfer_rule,
        });
    }

    if let Some(pending) = pending {
        pair_transfers(&mut tx, account.id, pending, &created).await?;
    }
    flag_exact_duplicates(&mut tx, batch_id, account.id, &created).await?;
    tx.commit().await?;

    Ok(ImportOutcome {
        batch_id,
        file_name: filename.to_owned(),
        transaction_count,
        categorised_count: categorised,
        uncategorised_count: transaction_count - categorised,
        ai_categorised_count: 0,
        learned_count: 0,
        ai_error: None,
    })
}

/// Pairs each new transaction that's uncategorised or a transfer with the other side already
/// waiting in Transfers:Pending: a transaction on another account, within three days, for the
/// opposite amount, not yet paired.
async fn pair_transfers(
    tx: &mut SqliteConnection,
    account_id: i64,
    pending: i64,
    rows: &[NewRow],
) -> Result<(), sqlx::Error> {
    for row in rows.iter().filter(|r| r.needs_transfer_check) {
        let already: Option<i64> =
            sqlx::query_scalar("SELECT transfer_peer_id FROM transactions WHERE id = ?")
                .bind(row.id)
                .fetch_one(&mut *tx)
                .await?;
        if already.is_some() {
            continue;
        }
        let candidate: Option<i64> = sqlx::query_scalar(
            "SELECT t.id FROM transactions t
             JOIN postings waiting ON waiting.transaction_id = t.id AND waiting.account_id = ?
             JOIN postings bank ON bank.transaction_id = t.id AND bank.id != waiting.id
                               AND bank.amount = ? AND bank.account_id != ?
             WHERE t.id != ? AND t.transfer_peer_id IS NULL AND t.date BETWEEN ? AND ?
             ORDER BY t.id LIMIT 1",
        )
        .bind(pending)
        .bind(-row.amount)
        .bind(account_id)
        .bind(row.id)
        .bind(row.date - Duration::days(MATCH_DAYS))
        .bind(row.date + Duration::days(MATCH_DAYS))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(candidate) = candidate else {
            continue;
        };
        sqlx::query(
            "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = NULL
             WHERE transaction_id = ? AND account_id != ?",
        )
        .bind(pending)
        .bind(CategorisationSource::Transfer as i64)
        .bind(row.id)
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
        for (id, peer) in [(row.id, candidate), (candidate, row.id)] {
            sqlx::query("UPDATE transactions SET transfer_peer_id = ? WHERE id = ?")
                .bind(peer)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    Ok(())
}

/// Lower-case letters and digits, one space between runs: how descriptions are compared.
pub(crate) fn normalise(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Points each new transaction that's identical to one already on the same account (same
/// date, amount and description, from another import or added by hand) at it, for the user to
/// review. Matching ones that differ is left to the model (see `matching`).
async fn flag_exact_duplicates(
    tx: &mut SqliteConnection,
    batch_id: i64,
    account_id: i64,
    rows: &[NewRow],
) -> Result<(), sqlx::Error> {
    let mut claimed: Vec<i64> = Vec::new();
    for row in rows {
        let candidates: Vec<(i64, String)> = sqlx::query_as(
            "SELECT t.id, t.description FROM transactions t
             JOIN postings p ON p.transaction_id = t.id
             WHERE p.account_id = ? AND p.amount = ? AND t.id != ? AND t.date = ?
               AND (t.import_batch_id IS NULL OR t.import_batch_id != ?)
             ORDER BY t.date, t.id",
        )
        .bind(account_id)
        .bind(row.amount)
        .bind(row.id)
        .bind(row.date)
        .bind(batch_id)
        .fetch_all(&mut *tx)
        .await?;
        let wanted = normalise(&row.description);
        let same = candidates
            .into_iter()
            .find(|(id, description)| normalise(description) == wanted && !claimed.contains(id));
        if let Some((original, _)) = same {
            sqlx::query("UPDATE transactions SET duplicate_of_id = ? WHERE id = ?")
                .bind(original)
                .bind(row.id)
                .execute(&mut *tx)
                .await?;
            claimed.push(original);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchSummary {
    pub id: i64,
    pub importer_name: String,
    pub file_name: Option<String>,
    pub account_id: Option<i64>,
    pub account_path: Option<String>,
    pub account_name: Option<String>,
    /// The last four characters of the account's bank identifier, if it has one.
    pub account_number_hint: Option<String>,
    #[serde(serialize_with = "iso_timestamp")]
    pub imported_at: String,
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
    pub transaction_count: i64,
    pub confirmed_count: i64,
    pub pending_count: i64,
}

fn iso_timestamp<S: serde::Serializer>(stored: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&stored.replacen(' ', "T", 1))
}

fn optional_iso_timestamp<S: serde::Serializer>(
    stored: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match stored {
        Some(stored) => iso_timestamp(stored, serializer),
        None => serializer.serialize_none(),
    }
}

/// Every import, newest first.
pub async fn list(db: &SqlitePool) -> Result<Vec<BatchSummary>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        i64,
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<NaiveDate>,
        Option<NaiveDate>,
        i64,
        i64,
        i64,
    )> = sqlx::query_as(
        "SELECT b.id, b.importer_name, b.file_name, b.account_id, a.full_path, a.name,
                a.bank_identifier, b.imported_at, b.period_start, b.period_end,
                (SELECT COUNT(*) FROM transactions t WHERE t.import_batch_id = b.id),
                (SELECT COUNT(*) FROM transactions t
                 WHERE t.import_batch_id = b.id AND t.status = 'confirmed'),
                (SELECT COUNT(*) FROM transactions t
                 WHERE t.import_batch_id = b.id AND t.status = 'pending')
         FROM import_batches b LEFT JOIN accounts a ON a.id = b.account_id
         ORDER BY b.imported_at DESC, b.id DESC",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let identifier = r.6.as_deref().map(str::trim).unwrap_or_default();
            let hint: String = {
                let chars: Vec<char> = identifier.chars().collect();
                chars[chars.len().saturating_sub(4)..].iter().collect()
            };
            BatchSummary {
                id: r.0,
                importer_name: r.1,
                file_name: r.2,
                account_id: r.3,
                account_path: r.4,
                account_name: r.5,
                account_number_hint: (!hint.is_empty()).then_some(hint),
                imported_at: r.7,
                period_start: r.8,
                period_end: r.9,
                transaction_count: r.10,
                confirmed_count: r.11,
                pending_count: r.12,
            }
        })
        .collect())
}

/// The transaction already in the ledger that an imported one looks like.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateOf {
    pub transaction_id: i64,
    pub date: String,
    pub description: String,
    /// "imported from <file>" or "added by hand"
    pub source: String,
}

/// An imported transaction as the review pages show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewTransaction {
    pub id: i64,
    pub date: String,
    pub description: String,
    /// On the statement's side.
    pub amount: String,
    pub status: Status,
    pub account_full_path: Option<String>,
    pub target_account_full_path: Option<String>,
    pub categorised_by: Option<String>,
    pub rule_id: Option<i64>,
    pub rule_pattern: Option<String>,
    pub raw_content: Option<String>,
    pub line_number: Option<i64>,
    pub transfer_peer_id: Option<i64>,
    pub import_batch_id: Option<i64>,
    pub source_file_name: Option<String>,
    pub duplicate_of: Option<DuplicateOf>,
    /// 0 (normal) to 1 (very unusual), once the insights model has judged it.
    pub unusual_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Batch {
    pub id: i64,
    pub importer_name: String,
    pub source_type_id: i64,
    pub file_path: Option<String>,
    pub file_hash: Option<String>,
    pub file_name: Option<String>,
    pub account_id: Option<i64>,
    #[serde(serialize_with = "iso_timestamp")]
    pub imported_at: String,
    pub transaction_count: usize,
    pub categorised_count: usize,
    /// When the matching model last looked for duplicates and transfers in it.
    #[serde(serialize_with = "optional_iso_timestamp")]
    pub matched_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchReview {
    pub batch: Batch,
    pub transactions: Vec<ReviewTransaction>,
}

#[derive(sqlx::FromRow)]
struct BatchRow {
    id: i64,
    importer_name: String,
    source_type_id: i64,
    file_path: Option<String>,
    file_hash: Option<String>,
    file_name: Option<String>,
    account_id: Option<i64>,
    imported_at: String,
    period_start: Option<NaiveDate>,
    period_end: Option<NaiveDate>,
    matched_at: Option<String>,
}

const BATCH_COLUMNS: &str = "id, importer_name, source_type_id, file_path, file_hash, file_name,
    account_id, imported_at, period_start, period_end, matched_at";

async fn batch_row(conn: &mut SqliteConnection, id: i64) -> Result<Option<BatchRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {BATCH_COLUMNS} FROM import_batches WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await
}

#[derive(sqlx::FromRow)]
struct PostingRow {
    transaction_id: i64,
    account_id: i64,
    amount: Money,
    source: Option<String>,
    rule_id: Option<i64>,
    rule_pattern: Option<String>,
    full_path: Option<String>,
}

#[derive(sqlx::FromRow)]
struct TransactionRow {
    id: i64,
    date: NaiveDate,
    description: String,
    status: Status,
    transfer_peer_id: Option<i64>,
    import_batch_id: Option<i64>,
    unusual_score: Option<f64>,
    raw_content: Option<String>,
    line_number: Option<i64>,
    duplicate_id: Option<i64>,
    duplicate_date: Option<NaiveDate>,
    duplicate_description: Option<String>,
    duplicate_batch: Option<i64>,
    duplicate_file: Option<String>,
}

/// A batch's transactions, newest first, as the review pages show them.
async fn review_transactions(
    conn: &mut SqliteConnection,
    batch: &BatchRow,
) -> Result<Vec<ReviewTransaction>, sqlx::Error> {
    let rows: Vec<TransactionRow> = sqlx::query_as(
        "SELECT t.id, t.date, t.description, t.status, t.transfer_peer_id, t.import_batch_id,
                t.unusual_score,
                (SELECT r.raw_content FROM raw_import_rows r WHERE r.transaction_id = t.id
                 ORDER BY r.id LIMIT 1) AS raw_content,
                (SELECT r.line_number FROM raw_import_rows r WHERE r.transaction_id = t.id
                 ORDER BY r.id LIMIT 1) AS line_number,
                d.id AS duplicate_id, d.date AS duplicate_date,
                d.description AS duplicate_description, d.import_batch_id AS duplicate_batch,
                db.file_name AS duplicate_file
         FROM transactions t
         LEFT JOIN transactions d ON d.id = t.duplicate_of_id
         LEFT JOIN import_batches db ON db.id = d.import_batch_id
         WHERE t.import_batch_id = ?
         ORDER BY t.date DESC, t.id DESC",
    )
    .bind(batch.id)
    .fetch_all(&mut *conn)
    .await?;
    let postings: Vec<PostingRow> = sqlx::query_as(
        "SELECT p.transaction_id, p.account_id, p.amount, c.name AS source, p.rule_id,
                r.pattern AS rule_pattern, a.full_path
         FROM postings p
         JOIN transactions t ON t.id = p.transaction_id
         LEFT JOIN accounts a ON a.id = p.account_id
         LEFT JOIN categorisation_sources c ON c.id = p.categorised_by_id
         LEFT JOIN rules r ON r.id = p.rule_id
         WHERE t.import_batch_id = ?
         ORDER BY p.id",
    )
    .bind(batch.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut by_transaction: HashMap<i64, Vec<PostingRow>> = HashMap::new();
    for posting in postings {
        by_transaction
            .entry(posting.transaction_id)
            .or_default()
            .push(posting);
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            let postings = by_transaction.remove(&row.id).unwrap_or_default();
            // The statement's side is the posting on its account; the other is the category.
            let mut bank = None;
            let mut category = None;
            for (i, posting) in postings.iter().enumerate() {
                if batch.account_id == Some(posting.account_id) {
                    bank = Some(i);
                } else {
                    category = Some(i);
                }
            }
            if bank.is_none() && category.is_none() && postings.len() >= 2 {
                (bank, category) = (Some(0), Some(1));
            }
            let bank = bank.map(|i| &postings[i]);
            let category = category.map(|i| &postings[i]);
            let duplicate_of = row.duplicate_id.map(|id| DuplicateOf {
                transaction_id: id,
                date: row
                    .duplicate_date
                    .map(|d| d.to_string())
                    .unwrap_or_default(),
                description: row.duplicate_description.clone().unwrap_or_default(),
                source: match row.duplicate_batch {
                    None => "added by hand".into(),
                    Some(_) => format!(
                        "imported from {}",
                        row.duplicate_file.as_deref().unwrap_or("a statement")
                    ),
                },
            });
            ReviewTransaction {
                id: row.id,
                date: row.date.to_string(),
                description: row.description,
                amount: bank.map_or_else(|| "0".into(), |p| p.amount.to_string()),
                status: row.status,
                account_full_path: bank.and_then(|p| p.full_path.clone()),
                target_account_full_path: category.and_then(|p| p.full_path.clone()),
                categorised_by: category.and_then(|p| p.source.clone()),
                rule_id: category.and_then(|p| p.rule_id),
                rule_pattern: category.and_then(|p| p.rule_pattern.clone()),
                raw_content: row.raw_content,
                line_number: row.line_number,
                transfer_peer_id: row.transfer_peer_id,
                import_batch_id: row.import_batch_id,
                source_file_name: batch.file_name.clone(),
                duplicate_of,
                unusual_score: row.unusual_score,
            }
        })
        .collect())
}

/// A batch with all its transactions, for review.
pub async fn review(db: &SqlitePool, id: i64) -> Result<BatchReview, ImportError> {
    let mut conn = db.acquire().await?;
    let batch = batch_row(&mut conn, id)
        .await?
        .ok_or(ImportError::BatchNotFound)?;
    let transactions = review_transactions(&mut conn, &batch).await?;
    Ok(BatchReview {
        batch: Batch {
            id: batch.id,
            importer_name: batch.importer_name.clone(),
            source_type_id: batch.source_type_id,
            file_path: batch.file_path.clone(),
            file_hash: batch.file_hash.clone(),
            file_name: batch.file_name.clone(),
            account_id: batch.account_id,
            imported_at: batch.imported_at.clone(),
            transaction_count: transactions.len(),
            categorised_count: transactions
                .iter()
                .filter(|t| t.categorised_by.as_deref() == Some("rule"))
                .count(),
            matched_at: batch.matched_at.clone(),
        },
        transactions,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountBatch {
    pub id: i64,
    pub file_name: Option<String>,
    #[serde(serialize_with = "iso_timestamp")]
    pub imported_at: String,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub transaction_count: usize,
    pub pending_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountReview {
    pub account_id: i64,
    pub account_path: Option<String>,
    pub batches: Vec<AccountBatch>,
    pub transactions: Vec<ReviewTransaction>,
}

/// Every import into an account, in statement order, with their transactions.
pub async fn account_review(
    db: &SqlitePool,
    account_id: i64,
) -> Result<AccountReview, ImportError> {
    let mut conn = db.acquire().await?;
    let account = accounts::get(&mut conn, account_id)
        .await?
        .ok_or(ImportError::AccountNotFound)?;
    let batches: Vec<BatchRow> = sqlx::query_as(&format!(
        "SELECT {BATCH_COLUMNS} FROM import_batches WHERE account_id = ?
         ORDER BY period_start IS NULL, period_start, imported_at, id"
    ))
    .bind(account_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut summaries = Vec::new();
    let mut transactions = Vec::new();
    for batch in &batches {
        let rows = review_transactions(&mut conn, batch).await?;
        summaries.push(AccountBatch {
            id: batch.id,
            file_name: batch.file_name.clone(),
            imported_at: batch.imported_at.clone(),
            period_start: batch.period_start.map(|d| d.to_string()),
            period_end: batch.period_end.map(|d| d.to_string()),
            transaction_count: rows.len(),
            pending_count: rows.iter().filter(|t| t.status == Status::Pending).count(),
        });
        transactions.extend(rows);
    }
    Ok(AccountReview {
        account_id,
        account_path: Some(account.full_path),
        batches: summaries,
        transactions,
    })
}

/// Confirms a batch's pending transactions. Returns how many were.
pub async fn confirm_batch(db: &SqlitePool, id: i64) -> Result<u64, ImportError> {
    let mut conn = db.acquire().await?;
    batch_row(&mut conn, id)
        .await?
        .ok_or(ImportError::BatchNotFound)?;
    Ok(sqlx::query(
        "UPDATE transactions SET status = 'confirmed'
         WHERE import_batch_id = ? AND status = 'pending'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// Confirms the pending transactions of every import into an account.
pub async fn confirm_account(db: &SqlitePool, account_id: i64) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE transactions SET status = 'confirmed'
         WHERE status = 'pending'
           AND import_batch_id IN (SELECT id FROM import_batches WHERE account_id = ?)",
    )
    .bind(account_id)
    .execute(db)
    .await?
    .rows_affected())
}

/// Re-applies rules to a batch's transactions. Ones categorised by hand or paired as
/// transfers are left alone. Returns how many changed.
pub async fn reapply_rules(db: &SqlitePool, id: i64) -> Result<u64, ImportError> {
    let mut tx = db.begin().await?;
    let batch = batch_row(&mut tx, id)
        .await?
        .ok_or(ImportError::BatchNotFound)?;
    let rules = RuleSet::load(&mut tx).await?;
    let uncategorised = uncategorised(&mut tx).await?;
    let Some(account_id) = batch.account_id else {
        tx.commit().await?;
        return Ok(0);
    };
    // Each transaction's first posting not on the statement's account.
    let categories: Vec<(i64, String, i64, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT t.id, t.description, p.id, p.account_id, p.categorised_by_id, p.rule_id
         FROM transactions t
         JOIN postings p ON p.id = (SELECT id FROM postings
                                    WHERE transaction_id = t.id AND account_id != ?
                                    ORDER BY id LIMIT 1)
         WHERE t.import_batch_id = ?",
    )
    .bind(account_id)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;

    let mut updated = 0;
    for (_, description, posting_id, current, source, current_rule) in categories {
        if source == CategorisationSource::Manual as i64
            || source == CategorisationSource::Transfer as i64
        {
            continue;
        }
        let change = match rules.first_match(&description) {
            Some(rule) if current != rule.target_account_id || current_rule != Some(rule.id) => {
                Some((
                    rule.target_account_id,
                    CategorisationSource::Rule,
                    Some(rule.id),
                ))
            }
            Some(_) => None,
            None if source != CategorisationSource::ImportDefault as i64 => {
                Some((uncategorised, CategorisationSource::ImportDefault, None))
            }
            None => None,
        };
        if let Some((account, source, rule_id)) = change {
            sqlx::query(
                "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = ? WHERE id = ?",
            )
            .bind(account)
            .bind(source as i64)
            .bind(rule_id)
            .bind(posting_id)
            .execute(&mut *tx)
            .await?;
            updated += 1;
        }
    }
    tx.commit().await?;
    Ok(updated)
}

/// Files a transaction (imported or added by hand) under a category by hand: its one income
/// or expense posting, or else the side that isn't the statement's account.
pub async fn categorise(
    db: &SqlitePool,
    transaction_id: i64,
    target_account_id: i64,
) -> Result<(), ImportError> {
    let mut tx = db.begin().await?;
    let batch_account: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT b.account_id FROM transactions t
         LEFT JOIN import_batches b ON b.id = t.import_batch_id WHERE t.id = ?",
    )
    .bind(transaction_id)
    .fetch_optional(&mut *tx)
    .await?;
    let batch_account = batch_account.ok_or(ImportError::TransactionNotFound)?;
    if accounts::get(&mut tx, target_account_id).await?.is_none() {
        return Err(ImportError::TargetNotFound);
    }
    let postings: Vec<(i64, i64, AccountType)> = sqlx::query_as(
        "SELECT p.id, p.account_id, a.type FROM postings p JOIN accounts a ON a.id = p.account_id
         WHERE p.transaction_id = ? ORDER BY p.id",
    )
    .bind(transaction_id)
    .fetch_all(&mut *tx)
    .await?;
    let categories: Vec<i64> = postings
        .iter()
        .filter(|(_, _, kind)| matches!(kind, AccountType::Income | AccountType::Expense))
        .map(|(id, _, _)| *id)
        .collect();
    let posting = if let [only] = categories[..] {
        Some(only)
    } else {
        let first_account = postings.first().map(|(_, account, _)| *account);
        postings
            .iter()
            .find(|(_, account, _)| match batch_account {
                Some(batch_account) => *account != batch_account,
                None => Some(*account) != first_account,
            })
            .or_else(|| postings.get(1))
            .map(|(id, _, _)| *id)
    };
    let posting = posting.ok_or(ImportError::NothingToCategorise)?;
    sqlx::query(
        "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = NULL WHERE id = ?",
    )
    .bind(target_account_id)
    .bind(CategorisationSource::Manual as i64)
    .bind(posting)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Keeps a transaction that was flagged as a possible duplicate.
pub async fn not_duplicate(db: &SqlitePool, transaction_id: i64) -> Result<(), ImportError> {
    let result = sqlx::query("UPDATE transactions SET duplicate_of_id = NULL WHERE id = ?")
        .bind(transaction_id)
        .execute(db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ImportError::TransactionNotFound);
    }
    Ok(())
}

/// Deletes an import: its transactions, its statement rows, and the stored file.
pub async fn delete(db: &SqlitePool, id: i64) -> Result<(), ImportError> {
    let mut tx = db.begin().await?;
    let batch = batch_row(&mut tx, id)
        .await?
        .ok_or(ImportError::BatchNotFound)?;
    sqlx::query("DELETE FROM transactions WHERE import_batch_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM import_batches WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if let Some(path) = batch.file_path {
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(())
}

/// The statement file an import came from: its path on disk and its name.
pub async fn stored_file(db: &SqlitePool, id: i64) -> Result<(PathBuf, String), ImportError> {
    let mut conn = db.acquire().await?;
    let batch = batch_row(&mut conn, id)
        .await?
        .ok_or(ImportError::FileNotFound)?;
    let path = PathBuf::from(batch.file_path.ok_or(ImportError::FileNotFound)?);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(ImportError::FileGone);
    }
    let name = batch
        .file_name
        .unwrap_or_else(|| format!("statement-{id}.pdf"));
    Ok((path, name))
}

/// The "Expenses" account of a new chart, if the caller needs one for tests or seeding.
#[cfg(test)]
async fn account(pool: &SqlitePool, path: &str, kind: AccountType) -> i64 {
    accounts::create(
        pool,
        accounts::NewAccount {
            name: path.rsplit(':').next().unwrap().into(),
            full_path: path.into(),
            parent_id: None,
            kind,
            currency: "GBP".into(),
            bank_identifier: None,
            icon: None,
        },
    )
    .await
    .unwrap()
    .id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::lookups::MatchType;
    use crate::rules::{self, NewRule};

    const STATEMENT: &str = "Date,Description,Amount\n\
                             20/09/2026,TESCO STORES,-12.50\n\
                             21/09/2026,SALARY,1500\n\
                             22/09/2026,TRANSFER TO SAVINGS,-100\n";

    struct Setup {
        pool: SqlitePool,
        dir: tempfile::TempDir,
        current: i64,
        savings: i64,
        groceries: i64,
    }

    async fn setup() -> Setup {
        let pool = db::open_in_memory().await.unwrap();
        accounts::load_seed_accounts(&pool).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let id = |a: Option<accounts::Account>| a.unwrap().id;
        let current = id(accounts::find_by_path(&mut conn, "Assets:Bank:Current")
            .await
            .unwrap());
        let savings = id(accounts::find_by_path(&mut conn, "Assets:Bank:Savings")
            .await
            .unwrap());
        let groceries = id(accounts::find_by_path(&mut conn, "Expenses:Food:Groceries")
            .await
            .unwrap());
        drop(conn);
        for (pattern, target) in [("TESCO", groceries), ("TRANSFER TO SAVINGS", savings)] {
            rules::create(
                &pool,
                NewRule {
                    pattern: pattern.into(),
                    match_type_id: MatchType::Substring as i64,
                    target_account_id: target,
                    priority: 0,
                    source: "manual".into(),
                    description: None,
                },
            )
            .await
            .unwrap();
        }
        Setup {
            pool,
            dir: tempfile::tempdir().unwrap(),
            current,
            savings,
            groceries,
        }
    }

    fn upload(account_id: i64, filename: &str, content: &str) -> Upload {
        Upload {
            account_id,
            importer_name: "csv".into(),
            filename: filename.into(),
            content: content.as_bytes().to_vec(),
            csv_columns: None,
        }
    }

    #[tokio::test]
    async fn imports_categorises_and_routes_transfers() {
        let s = setup().await;
        let outcome = import(
            &s.pool,
            s.dir.path(),
            &Importers::default(),
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "sept.csv", STATEMENT),
        )
        .await
        .unwrap();
        assert_eq!(outcome.transaction_count, 3);
        assert_eq!(outcome.categorised_count, 2);
        assert_eq!(outcome.uncategorised_count, 1);

        let review = review(&s.pool, outcome.batch_id).await.unwrap();
        let by_description: HashMap<&str, &ReviewTransaction> = review
            .transactions
            .iter()
            .map(|t| (t.description.as_str(), t))
            .collect();
        let tesco = by_description["TESCO STORES"];
        assert_eq!(tesco.amount, "-12.5000");
        assert_eq!(
            tesco.target_account_full_path.as_deref(),
            Some("Expenses:Food:Groceries")
        );
        assert_eq!(tesco.categorised_by.as_deref(), Some("rule"));
        assert_eq!(
            tesco.raw_content.as_deref(),
            Some("20/09/2026,TESCO STORES,-12.50")
        );
        assert_eq!(
            by_description["SALARY"].target_account_full_path.as_deref(),
            Some(UNCATEGORISED_PATH)
        );
        let transfer = by_description["TRANSFER TO SAVINGS"];
        assert_eq!(
            transfer.target_account_full_path.as_deref(),
            Some(TRANSFERS_PENDING_PATH)
        );
        assert_eq!(transfer.categorised_by.as_deref(), Some("transfer"));
        assert_eq!(
            review.batch.categorised_count, 1,
            "only rule-categorised ones count"
        );
        // Newest first
        assert_eq!(review.transactions[0].description, "TRANSFER TO SAVINGS");

        let stored = std::fs::read_dir(s.dir.path()).unwrap().count();
        assert_eq!(stored, 1);
    }

    #[tokio::test]
    async fn pairs_the_other_side_of_a_transfer_and_flags_duplicates() {
        let s = setup().await;
        let importers = Importers::default();
        import(
            &s.pool,
            s.dir.path(),
            &importers,
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "current.csv", STATEMENT),
        )
        .await
        .unwrap();
        let savings = import(
            &s.pool,
            s.dir.path(),
            &importers,
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(
                s.savings,
                "savings.csv",
                "Date,Description,Amount\n23/09/2026,FROM CURRENT,100\n",
            ),
        )
        .await
        .unwrap();
        let paired = review(&s.pool, savings.batch_id).await.unwrap();
        let side = &paired.transactions[0];
        assert!(side.transfer_peer_id.is_some());
        assert_eq!(
            side.target_account_full_path.as_deref(),
            Some(TRANSFERS_PENDING_PATH)
        );

        // The same lines again, in a file that differs only by a blank line
        let again = import(
            &s.pool,
            s.dir.path(),
            &importers,
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "again.csv", &format!("{STATEMENT}\n")),
        )
        .await
        .unwrap();
        let flagged = review(&s.pool, again.batch_id).await.unwrap();
        assert!(
            flagged
                .transactions
                .iter()
                .all(|t| t.duplicate_of.is_some())
        );
        assert_eq!(
            flagged.transactions[0]
                .duplicate_of
                .as_ref()
                .unwrap()
                .source,
            "imported from current.csv"
        );
    }

    #[tokio::test]
    async fn refuses_the_same_file_twice_and_empty_files() {
        let s = setup().await;
        let importers = Importers::default();
        let first = import(
            &s.pool,
            s.dir.path(),
            &importers,
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "a.csv", STATEMENT),
        )
        .await
        .unwrap();
        let again = import(
            &s.pool,
            s.dir.path(),
            &importers,
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "b.csv", STATEMENT),
        )
        .await;
        assert!(matches!(again, Err(ImportError::AlreadyImported(id)) if id == first.batch_id));
        let empty = import(
            &s.pool,
            s.dir.path(),
            &importers,
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "c.csv", "Date,Description,Amount\n"),
        )
        .await;
        assert!(matches!(empty, Err(ImportError::Empty)));
        let missing = Upload {
            importer_name: "hsbc".into(),
            ..upload(s.current, "d.pdf", "%PDF")
        };
        assert!(matches!(
            import(
                &s.pool,
                s.dir.path(),
                &importers,
                &Learned::default(),
                crate::ai::http::FakeApi::new().as_ref(),
                missing
            )
            .await,
            Err(ImportError::ImporterMissing(_))
        ));
    }

    #[tokio::test]
    async fn stored_files_stay_inside_the_upload_directory() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("..\\..\\x.csv"), "x.csv");
        assert_eq!(safe_name(".."), "statement");
        let s = setup().await;
        import(
            &s.pool,
            s.dir.path(),
            &Importers::default(),
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "../../evil.csv", STATEMENT),
        )
        .await
        .unwrap();
        let names: Vec<String> = std::fs::read_dir(s.dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(names[0].ends_with("_evil.csv"), "{names:?}");
    }

    #[tokio::test]
    async fn reapplies_rules_but_not_over_manual_choices() {
        let s = setup().await;
        let outcome = import(
            &s.pool,
            s.dir.path(),
            &Importers::default(),
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "a.csv", STATEMENT),
        )
        .await
        .unwrap();
        let salary_rule = rules::create(
            &s.pool,
            NewRule {
                pattern: "SALARY".into(),
                match_type_id: 1,
                target_account_id: account(&s.pool, "Income:Wages", AccountType::Income).await,
                priority: 0,
                source: "manual".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        let tesco = review(&s.pool, outcome.batch_id)
            .await
            .unwrap()
            .transactions
            .into_iter()
            .find(|t| t.description == "TESCO STORES")
            .unwrap();
        let takeaway = account(&s.pool, "Expenses:Takeaway", AccountType::Expense).await;
        categorise(&s.pool, tesco.id, takeaway).await.unwrap();

        assert_eq!(reapply_rules(&s.pool, outcome.batch_id).await.unwrap(), 1);
        let after = review(&s.pool, outcome.batch_id).await.unwrap();
        let find = |d: &str| {
            after
                .transactions
                .iter()
                .find(|t| t.description == d)
                .unwrap()
                .clone()
        };
        assert_eq!(find("SALARY").rule_id, Some(salary_rule.id));
        assert_eq!(
            find("TESCO STORES").target_account_full_path.as_deref(),
            Some("Expenses:Takeaway")
        );
        assert_eq!(
            find("TESCO STORES").categorised_by.as_deref(),
            Some("manual")
        );
        let _ = s.groceries;
    }

    #[tokio::test]
    async fn confirms_lists_and_deletes() {
        let s = setup().await;
        let outcome = import(
            &s.pool,
            s.dir.path(),
            &Importers::default(),
            &Learned::default(),
            crate::ai::http::FakeApi::new().as_ref(),
            upload(s.current, "a.csv", STATEMENT),
        )
        .await
        .unwrap();
        let listed = list(&s.pool).await.unwrap();
        assert_eq!(listed[0].pending_count, 3);
        assert_eq!(listed[0].account_name.as_deref(), Some("Current Account"));
        assert_eq!(confirm_batch(&s.pool, outcome.batch_id).await.unwrap(), 3);
        assert_eq!(confirm_account(&s.pool, s.current).await.unwrap(), 0);
        let by_account = account_review(&s.pool, s.current).await.unwrap();
        assert_eq!(by_account.batches[0].pending_count, 0);
        assert_eq!(by_account.transactions.len(), 3);

        delete(&s.pool, outcome.batch_id).await.unwrap();
        assert!(list(&s.pool).await.unwrap().is_empty());
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions")
            .fetch_one(&s.pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
        assert_eq!(
            std::fs::read_dir(s.dir.path()).unwrap().count(),
            0,
            "the stored file goes too"
        );
        assert!(matches!(
            delete(&s.pool, outcome.batch_id).await,
            Err(ImportError::BatchNotFound)
        ));
    }

    #[test]
    fn normalises_descriptions() {
        assert_eq!(normalise("TESCO  Stores*2041!"), "tesco stores 2041");
    }
}
