//! Matching a statement's new transactions against the ledger where exact matching falls
//! short: spotting ones already recorded (from an overlapping statement or added by hand), and
//! pairing up transfers between the user's own accounts whose two sides are described
//! differently.
//!
//! Exact duplicates and exact transfer pairs are found as the import is recorded (see
//! `batches`). Everything here needs the "matching" task's model, straight after the import if
//! it runs automatically or later from the import's review page, and acts only on answers at
//! or above `MIN_PROBABILITY`; if the model fails, the import goes ahead without it (the error
//! is in the AI request logs).

use std::collections::HashSet;

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use super::batches::{TRANSFERS_PENDING_PATH, UNCATEGORISED_PATH, normalise};
use crate::Money;
use crate::ai::config::{self, Task};
use crate::ai::http::Transport;
use crate::ai::matching::{ask_pairs, best};
use crate::ai::providers::{self, ResolveError, Resolved};
use crate::lookups::CategorisationSource;

const MATCH_DAYS: i64 = 3;

const DUPLICATE_QUESTION: &str = "Are {a} and {b} the same bank transaction recorded twice, for \
     example once from a CSV export and once from a PDF statement, or once added by hand? \
     Descriptions may be formatted differently. Two separate payments to the same place for \
     the same amount are not duplicates.";

const TRANSFER_QUESTION: &str = "Are {a} and {b} the two sides of one transfer of money between \
     the user's own accounts, such as paying off a credit card from a current account or moving \
     money into savings? Each shows the account it is on and the amount from that account's \
     point of view.";

/// A transaction an import created: its date, description, the amount on the statement's
/// account, and the posting on the other side.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MatchRow {
    pub id: i64,
    pub date: NaiveDate,
    pub description: String,
    pub amount: Money,
    pub other_posting: i64,
    pub other_account: i64,
}

/// The import's transactions not yet flagged as duplicates or paired as transfers, with one
/// posting on the statement's account and one other, oldest first.
pub async fn open_rows(
    db: &SqlitePool,
    batch_id: i64,
    account_id: i64,
) -> Result<Vec<MatchRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT t.id, t.date, t.description, bank.amount, other.id AS other_posting,
                other.account_id AS other_account
         FROM transactions t
         JOIN postings bank ON bank.transaction_id = t.id AND bank.account_id = ?1
         JOIN postings other ON other.transaction_id = t.id AND other.id != bank.id
         WHERE t.import_batch_id = ?2 AND t.duplicate_of_id IS NULL
           AND t.transfer_peer_id IS NULL
           AND (SELECT COUNT(*) FROM postings p WHERE p.transaction_id = t.id) = 2
         ORDER BY t.date, t.id",
    )
    .bind(account_id)
    .bind(batch_id)
    .fetch_all(db)
    .await
}

/// Where a transaction came from, for the model and the review page.
pub fn describe_source(file_name: Option<&str>, imported: bool) -> String {
    if imported {
        format!("imported from {}", file_name.unwrap_or("a statement"))
    } else {
        "added by hand".into()
    }
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: i64,
    date: NaiveDate,
    description: String,
    import_batch_id: Option<i64>,
    file_name: Option<String>,
}

/// Points each row that the model is confident is already on the same account (from another
/// import, or added by hand) at that transaction, for the user to review. Rows with an
/// identical transaction there were handled when the import was recorded, and aren't asked
/// about.
async fn model_duplicates(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    batch_id: i64,
    account_id: i64,
    rows: &[MatchRow],
) -> Result<(), sqlx::Error> {
    let mut ask: Vec<(i64, Candidate)> = Vec::new();
    let mut pairs = Vec::new();
    for row in rows {
        let found: Vec<Candidate> = sqlx::query_as(
            "SELECT t.id, t.date, t.description, t.import_batch_id, b.file_name
             FROM transactions t
             JOIN postings p ON p.transaction_id = t.id
             LEFT JOIN import_batches b ON b.id = t.import_batch_id
             WHERE p.account_id = ? AND p.amount = ? AND t.id != ?
               AND (t.import_batch_id IS NULL OR t.import_batch_id != ?)
               AND t.date BETWEEN ? AND ?
             ORDER BY t.date, t.id",
        )
        .bind(account_id)
        .bind(row.amount)
        .bind(row.id)
        .bind(batch_id)
        .bind(row.date - Duration::days(MATCH_DAYS))
        .bind(row.date + Duration::days(MATCH_DAYS))
        .fetch_all(db)
        .await?;
        let wanted = normalise(&row.description);
        let identical = found
            .iter()
            .any(|c| c.date == row.date && normalise(&c.description) == wanted);
        if identical {
            continue;
        }
        for candidate in found {
            pairs.push((
                json!({"date": row.date, "description": row.description, "source": "this import"}),
                json!({
                    "date": candidate.date,
                    "description": candidate.description,
                    "source": describe_source(
                        candidate.file_name.as_deref(),
                        candidate.import_batch_id.is_some(),
                    ),
                }),
            ));
            ask.push((row.id, candidate));
        }
    }
    if ask.is_empty() {
        return Ok(());
    }
    let Ok(probabilities) = ask_pairs(
        db,
        http,
        model,
        "duplicate_match",
        DUPLICATE_QUESTION,
        &pairs,
    )
    .await
    else {
        return Ok(());
    };
    let keyed: Vec<(i64, i64)> = ask.iter().map(|(row, c)| (*row, c.id)).collect();
    let chosen = best(&keyed, &probabilities);
    let mut claimed: HashSet<i64> = HashSet::new();
    for row in rows {
        if let Some(original) = chosen.get(&row.id)
            && claimed.insert(*original)
        {
            sqlx::query("UPDATE transactions SET duplicate_of_id = ? WHERE id = ?")
                .bind(original)
                .bind(row.id)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct TransferCandidate {
    id: i64,
    date: NaiveDate,
    description: String,
    amount: Money,
    account: String,
    other_posting: i64,
}

/// Moves a transaction's other side to Transfers:Pending, as linked transfers are.
async fn park(db: &mut SqliteConnection, posting: i64, pending: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = NULL WHERE id = ?",
    )
    .bind(pending)
    .bind(CategorisationSource::Transfer as i64)
    .bind(posting)
    .execute(db)
    .await?;
    Ok(())
}

async fn link(db: &mut SqliteConnection, a: i64, b: i64) -> Result<(), sqlx::Error> {
    for (id, peer) in [(a, b), (b, a)] {
        sqlx::query("UPDATE transactions SET transfer_peer_id = ? WHERE id = ?")
            .bind(peer)
            .bind(id)
            .execute(&mut *db)
            .await?;
    }
    Ok(())
}

/// Links rows still uncategorised to an uncategorised transaction on another of the user's
/// accounts for the opposite amount, when the model is confident they're the two sides of one
/// transfer. Both are then routed via Transfers:Pending.
async fn model_transfers(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    account_id: i64,
    rows: &[MatchRow],
    pending: i64,
    uncategorised: i64,
) -> Result<(), sqlx::Error> {
    let open = [uncategorised, pending];
    let account_path: String = sqlx::query_scalar("SELECT full_path FROM accounts WHERE id = ?")
        .bind(account_id)
        .fetch_one(db)
        .await?;
    let mut ask: Vec<(i64, TransferCandidate)> = Vec::new();
    let mut pairs = Vec::new();
    for row in rows.iter().filter(|r| open.contains(&r.other_account)) {
        let found: Vec<TransferCandidate> = sqlx::query_as(
            "SELECT t.id, t.date, t.description, bank.amount, a.full_path AS account,
                    other.id AS other_posting
             FROM transactions t
             JOIN postings bank ON bank.transaction_id = t.id
             JOIN accounts a ON a.id = bank.account_id
             JOIN postings other ON other.transaction_id = t.id AND other.id != bank.id
             WHERE t.id != ? AND t.transfer_peer_id IS NULL AND t.duplicate_of_id IS NULL
               AND t.date BETWEEN ? AND ?
               AND bank.amount = ? AND bank.account_id NOT IN (?, ?)
               AND a.type IN ('asset', 'liability')
               AND other.account_id IN (?, ?)
             ORDER BY t.date, t.id",
        )
        .bind(row.id)
        .bind(row.date - Duration::days(MATCH_DAYS))
        .bind(row.date + Duration::days(MATCH_DAYS))
        .bind(-row.amount)
        .bind(account_id)
        .bind(pending)
        .bind(uncategorised)
        .bind(pending)
        .fetch_all(db)
        .await?;
        for candidate in found {
            pairs.push((
                json!({
                    "account": account_path,
                    "date": row.date,
                    "description": row.description,
                    "amount": row.amount.to_string(),
                }),
                json!({
                    "account": candidate.account,
                    "date": candidate.date,
                    "description": candidate.description,
                    "amount": candidate.amount.to_string(),
                }),
            ));
            ask.push((row.id, candidate));
        }
    }
    if ask.is_empty() {
        return Ok(());
    }
    let Ok(probabilities) =
        ask_pairs(db, http, model, "transfer_match", TRANSFER_QUESTION, &pairs).await
    else {
        return Ok(());
    };
    let keyed: Vec<(i64, TransferCandidate)> = ask.clone();
    let chosen = best(&keyed, &probabilities);
    let mut tx = db.begin().await?;
    for row in rows {
        let Some(candidate) = chosen.get(&row.id) else {
            continue;
        };
        let peer: Option<i64> =
            sqlx::query_scalar("SELECT transfer_peer_id FROM transactions WHERE id = ?")
                .bind(candidate.id)
                .fetch_one(&mut *tx)
                .await?;
        if peer.is_some() {
            // Claimed by an earlier row.
            continue;
        }
        park(&mut tx, row.other_posting, pending).await?;
        park(&mut tx, candidate.other_posting, pending).await?;
        link(&mut tx, row.id, candidate.id).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn account_id(db: &SqlitePool, path: &str) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM accounts WHERE full_path = ?")
        .bind(path)
        .fetch_optional(db)
        .await
}

/// Asks the model about the import's duplicates and transfers, and notes that it has.
async fn run(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    batch_id: i64,
    account_id_: i64,
) -> Result<(), sqlx::Error> {
    let rows = open_rows(db, batch_id, account_id_).await?;
    model_duplicates(db, http, model, batch_id, account_id_, &rows).await?;
    let pending = account_id(db, TRANSFERS_PENDING_PATH).await?;
    let uncategorised = account_id(db, UNCATEGORISED_PATH).await?;
    if let (Some(pending), Some(uncategorised)) = (pending, uncategorised) {
        let rows = open_rows(db, batch_id, account_id_).await?;
        model_transfers(db, http, model, account_id_, &rows, pending, uncategorised).await?;
    }
    sqlx::query("UPDATE import_batches SET matched_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(batch_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Matches a new import with the model, if matching runs automatically and is set up.
pub async fn after_import(
    db: &SqlitePool,
    http: &dyn Transport,
    batch_id: i64,
    account_id: i64,
) -> Result<(), sqlx::Error> {
    if !config::runs_automatically(db, Task::Matching)
        .await
        .unwrap_or(false)
    {
        return Ok(());
    }
    let Ok(model) = providers::resolve_task(db, Task::Matching).await else {
        return Ok(());
    };
    run(db, http, &model, batch_id, account_id).await
}

#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    #[error("Import batch not found")]
    BatchNotFound,
    #[error("Transaction not found")]
    NotFound,
    #[error("Peer transaction not found")]
    PeerNotFound,
    #[error("Peer transaction is already linked to a transfer")]
    PeerTaken,
    #[error("Assets:Transfers:Pending account not found. Load UK defaults first.")]
    NoPendingAccount,
    #[error("Cannot find category posting")]
    NoCategoryPosting,
    /// What to set up for the model.
    #[error("{0}")]
    Model(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Matched {
    pub duplicates: usize,
    pub transfers: usize,
    pub taps: usize,
}

/// Asks the model about an import's transactions, for when matching runs on demand rather
/// than straight after each import. Exact duplicates were flagged on import, so they aren't
/// flagged again.
pub async fn match_batch(
    db: &SqlitePool,
    http: &dyn Transport,
    batch_id: i64,
) -> Result<Matched, MatchError> {
    let account: Option<Option<i64>> =
        sqlx::query_scalar("SELECT account_id FROM import_batches WHERE id = ?")
            .bind(batch_id)
            .fetch_optional(db)
            .await?;
    let Some(account) = account else {
        return Err(MatchError::BatchNotFound);
    };
    let model = match providers::resolve_task(db, Task::Matching).await {
        Ok(model) => model,
        Err(ResolveError::Provider(e)) => return Err(MatchError::Model(e.message)),
        Err(ResolveError::Setting(e)) => return Err(MatchError::Model(e.to_string())),
    };
    let Some(account) = account else {
        return Ok(Matched {
            duplicates: 0,
            transfers: 0,
            taps: 0,
        });
    };
    let before: HashSet<i64> = open_rows(db, batch_id, account)
        .await?
        .iter()
        .map(|r| r.id)
        .collect();
    run(db, http, &model, batch_id, account).await?;
    let taps = crate::taps::reconcile(db, http, true).await?;
    let (duplicates, transfers) = if before.is_empty() {
        (0, 0)
    } else {
        let flags: Vec<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT id, duplicate_of_id, transfer_peer_id FROM transactions
             WHERE import_batch_id = ?",
        )
        .bind(batch_id)
        .fetch_all(db)
        .await?;
        let mine = flags.iter().filter(|(id, ..)| before.contains(id));
        let (mut duplicates, mut transfers) = (0, 0);
        for (_, duplicate, peer) in mine {
            duplicates += usize::from(duplicate.is_some());
            transfers += usize::from(peer.is_some());
        }
        (duplicates, transfers)
    };
    Ok(Matched {
        duplicates,
        transfers,
        taps,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct TransferSuggestion {
    pub transaction_id: i64,
    pub date: NaiveDate,
    pub description: String,
    pub amount: Money,
    pub account_full_path: Option<String>,
    pub days_apart: i64,
}

/// A transaction's postings, and the account of the import it came from.
async fn postings_of(
    db: &mut SqliteConnection,
    id: i64,
) -> Result<Option<(Option<i64>, Vec<(i64, i64, Money)>)>, sqlx::Error> {
    let batch_account: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT b.account_id FROM transactions t
         LEFT JOIN import_batches b ON b.id = t.import_batch_id WHERE t.id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let Some(batch_account) = batch_account else {
        return Ok(None);
    };
    let postings: Vec<(i64, i64, Money)> = sqlx::query_as(
        "SELECT id, account_id, amount FROM postings WHERE transaction_id = ? ORDER BY id",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    Ok(Some((batch_account, postings)))
}

/// Transactions that could be the other side of a transfer: the opposite amount on another
/// of the user's accounts, within three days, nearest first.
pub async fn transfer_suggestions(
    db: &SqlitePool,
    id: i64,
) -> Result<Vec<TransferSuggestion>, MatchError> {
    let mut conn = db.acquire().await?;
    let (batch_account, postings) = postings_of(&mut conn, id)
        .await?
        .ok_or(MatchError::NotFound)?;
    let bank = postings
        .iter()
        .find(|(_, account, _)| Some(*account) == batch_account)
        .or_else(|| postings.first());
    let Some((_, _, amount)) = bank else {
        return Ok(Vec::new());
    };
    let date: NaiveDate = sqlx::query_scalar("SELECT date FROM transactions WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let mut suggestions: Vec<TransferSuggestion> = sqlx::query_as(
        "SELECT t.id AS transaction_id, t.date, t.description, bank.amount,
                a.full_path AS account_full_path,
                CAST(abs(julianday(t.date) - julianday(?1)) AS INTEGER) AS days_apart
         FROM transactions t
         JOIN postings bank ON bank.transaction_id = t.id
         JOIN accounts a ON a.id = bank.account_id
         WHERE t.id != ?2 AND t.transfer_peer_id IS NULL AND t.date BETWEEN ?3 AND ?4
           AND bank.amount = ?5 AND a.type IN ('asset', 'liability')
           AND (?6 IS NULL OR bank.account_id != ?6)
         ORDER BY t.id LIMIT 10",
    )
    .bind(date)
    .bind(id)
    .bind(date - Duration::days(MATCH_DAYS))
    .bind(date + Duration::days(MATCH_DAYS))
    .bind(-*amount)
    .bind(batch_account)
    .fetch_all(&mut *conn)
    .await?;
    suggestions.sort_by_key(|s| s.days_apart);
    Ok(suggestions)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MarkAsTransfer {
    #[serde(default)]
    pub peer_transaction_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkedAsTransfer {
    pub status: &'static str,
    pub transaction_id: i64,
    pub peer_transaction_id: Option<i64>,
    pub parked: bool,
}

/// The posting that isn't on the account of the import a transaction came from, or its
/// second posting.
fn category_posting(batch_account: Option<i64>, postings: &[(i64, i64, Money)]) -> Option<i64> {
    batch_account
        .and_then(|account| postings.iter().find(|(_, a, _)| *a != account))
        .or_else(|| postings.get(1))
        .map(|(id, ..)| *id)
}

/// Marks a transaction as a transfer, parking it in Assets:Transfers:Pending. With a peer,
/// links the two; without, it waits there for the other side to be imported.
pub async fn mark_as_transfer(
    db: &SqlitePool,
    id: i64,
    request: MarkAsTransfer,
) -> Result<MarkedAsTransfer, MatchError> {
    let mut tx = db.begin().await?;
    let (batch_account, postings) = postings_of(&mut tx, id)
        .await?
        .ok_or(MatchError::NotFound)?;
    let pending = sqlx::query_scalar("SELECT id FROM accounts WHERE full_path = ?")
        .bind(TRANSFERS_PENDING_PATH)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(MatchError::NoPendingAccount)?;
    let category =
        category_posting(batch_account, &postings).ok_or(MatchError::NoCategoryPosting)?;
    park(&mut tx, category, pending).await?;

    if let Some(peer) = request.peer_transaction_id {
        let (peer_account, peer_postings) = postings_of(&mut tx, peer)
            .await?
            .ok_or(MatchError::PeerNotFound)?;
        let linked: Option<i64> =
            sqlx::query_scalar("SELECT transfer_peer_id FROM transactions WHERE id = ?")
                .bind(peer)
                .fetch_one(&mut *tx)
                .await?;
        if linked.is_some() {
            return Err(MatchError::PeerTaken);
        }
        if let Some(posting) = category_posting(peer_account, &peer_postings) {
            park(&mut tx, posting, pending).await?;
        }
        link(&mut tx, id, peer).await?;
    }
    tx.commit().await?;
    Ok(MarkedAsTransfer {
        status: "ok",
        transaction_id: id,
        peer_transaction_id: request.peer_transaction_id,
        parked: request.peer_transaction_id.is_none(),
    })
}

/// What's waiting in Assets:Transfers:Pending, and how many transactions there have no other
/// side yet.
pub async fn pending_balance(db: &SqlitePool) -> Result<Value, sqlx::Error> {
    let Some(pending) = account_id(db, TRANSFERS_PENDING_PATH).await? else {
        return Ok(json!({"balance": "0", "unlinked_count": 0}));
    };
    let (balance, unlinked): (Money, i64) = sqlx::query_as(
        "SELECT
            (SELECT COALESCE(SUM(amount), 0) FROM postings WHERE account_id = ?1),
            (SELECT COUNT(DISTINCT t.id) FROM transactions t
             JOIN postings p ON p.transaction_id = t.id
             WHERE p.account_id = ?1 AND t.transfer_peer_id IS NULL)",
    )
    .bind(pending)
    .fetch_one(db)
    .await?;
    Ok(json!({"balance": balance.to_string(), "unlinked_count": unlinked}))
}
