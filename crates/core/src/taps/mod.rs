//! Card taps: payments the phone's wallet shortcut reports as they happen, before they appear
//! on a statement. Taps live outside the ledger; once a statement with the payment is
//! imported, the tap is reconciled by pointing it at the imported transaction.

pub mod ai;
pub mod parse;
pub mod reconcile;

use std::collections::{BTreeMap, HashMap};

use chrono::{NaiveDate, NaiveDateTime};
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

pub use reconcile::reconcile;

use crate::Money;
use crate::ai::http::Transport;
use crate::auth::random_token;
use crate::learned::Learned;
use crate::rules::RuleSet;
use crate::settings::{Setting, SettingError};

// The tap token is stored readable (not hashed) so Settings can show it again when adding
// another phone. It only allows recording taps, and reading it requires signing in.
const TAP_TOKEN: Setting<String> = Setting::new("tap_token");

pub async fn token(db: &SqlitePool) -> Result<Option<String>, SettingError> {
    TAP_TOKEN.get(db).await
}

/// A new token, replacing any there was.
pub async fn new_token(db: &SqlitePool) -> Result<String, SettingError> {
    let token = random_token(32);
    TAP_TOKEN.set(db, &token).await?;
    Ok(token)
}

pub async fn clear_token(db: &SqlitePool) -> Result<(), SettingError> {
    TAP_TOKEN.clear(db).await
}

/// Whether `given` is the tap token: `None` if there isn't one, so taps are off.
pub async fn token_matches(db: &SqlitePool, given: &str) -> Result<Option<bool>, SettingError> {
    Ok(token(db)
        .await?
        .map(|expected| bool::from(given.as_bytes().ct_eq(expected.as_bytes()))))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Unmatched,
    Reconciled,
    Dismissed,
}

/// A tap as the Card taps page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tap {
    pub id: i64,
    pub tapped_on: NaiveDate,
    pub merchant: String,
    /// Positive for a purchase, negative for a refund.
    pub amount: Money,
    pub currency: String,
    pub card_name: Option<String>,
    pub status: Status,
    /// The account the tap's card is paired with.
    pub account_id: Option<i64>,
    pub account_full_path: Option<String>,
    pub suggested_account_full_path: Option<String>,
    /// "rule", "learned" (from the user's own history) or "ai", in that order of preference.
    pub suggestion_source: Option<&'static str>,
    pub suggestion_confidence: Option<f64>,
    pub transaction_id: Option<i64>,
    pub transaction_description: Option<String>,
    pub transaction_date: Option<NaiveDate>,
    pub created_at: NaiveDateTime,
}

#[derive(sqlx::FromRow)]
struct TapRow {
    id: i64,
    tapped_on: NaiveDate,
    merchant: String,
    amount: Money,
    currency: String,
    card_name: Option<String>,
    transaction_id: Option<i64>,
    dismissed: bool,
    created_at: NaiveDateTime,
    ai_confidence: Option<f64>,
    ai_path: Option<String>,
    account_id: Option<i64>,
    account_full_path: Option<String>,
    transaction_description: Option<String>,
    transaction_date: Option<NaiveDate>,
}

const SELECT: &str = "SELECT t.id, t.tapped_on, t.merchant, t.amount, t.currency, t.card_name,
           t.transaction_id, t.dismissed, t.created_at, t.ai_confidence, ai.full_path AS ai_path,
           card.id AS account_id, card.full_path AS account_full_path,
           tx.description AS transaction_description, tx.date AS transaction_date
    FROM card_taps t
    LEFT JOIN accounts ai ON ai.id = t.ai_account_id
    LEFT JOIN wallet_cards w ON w.card_name = t.card_name
    LEFT JOIN accounts card ON card.id = w.account_id
    LEFT JOIN transactions tx ON tx.id = t.transaction_id";

/// Taps as the page shows them, with each one's suggested category: the user's own rules win,
/// then what they've taught it, then the AI's suggestion.
async fn shown(
    db: &SqlitePool,
    learned: &Learned,
    rows: Vec<TapRow>,
) -> Result<Vec<Tap>, sqlx::Error> {
    let mut conn = db.acquire().await?;
    let rules = RuleSet::load(&mut conn).await?;
    drop(conn);
    let paths: HashMap<i64, String> = sqlx::query_as("SELECT id, full_path FROM accounts")
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();
    let by_rule: Vec<Option<String>> = rows
        .iter()
        .map(|r| {
            rules
                .first_match(&r.merchant)
                .map(|rule| paths.get(&rule.target_account_id).cloned())
                .map(Option::unwrap_or_default)
        })
        .collect();
    // What the user's own history says, for taps no rule covers.
    let unruled: Vec<usize> = (0..rows.len()).filter(|i| by_rule[*i].is_none()).collect();
    let merchants: Vec<String> = unruled.iter().map(|i| rows[*i].merchant.clone()).collect();
    let suggested = learned.suggest(db, &merchants).await.unwrap_or_default();
    let mut by_history: HashMap<usize, (String, f64)> = HashMap::new();
    for (i, suggestion) in unruled.into_iter().zip(suggested) {
        if let Some((account, p)) = suggestion
            && let Some(path) = paths.get(&account)
        {
            by_history.insert(i, (path.clone(), p));
        }
    }

    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let (suggestion, source, confidence) = if let Some(path) = &by_rule[i] {
                (Some(path.clone()), Some("rule"), None)
            } else if let Some((path, p)) = by_history.remove(&i) {
                (Some(path), Some("learned"), Some(p))
            } else if let Some(path) = r.ai_path {
                (Some(path), Some("ai"), r.ai_confidence)
            } else {
                (None, None, None)
            };
            let suggestion = suggestion.filter(|s| !s.is_empty());
            let status = if r.dismissed {
                Status::Dismissed
            } else if r.transaction_id.is_some() {
                Status::Reconciled
            } else {
                Status::Unmatched
            };
            Tap {
                id: r.id,
                tapped_on: r.tapped_on,
                merchant: r.merchant,
                amount: r.amount,
                currency: r.currency,
                card_name: r.card_name,
                status,
                account_id: r.account_id,
                account_full_path: r.account_full_path,
                suggestion_source: suggestion.as_ref().and(source),
                suggested_account_full_path: suggestion,
                suggestion_confidence: confidence,
                transaction_id: r.transaction_id,
                transaction_description: r.transaction_description,
                transaction_date: r.transaction_date,
                created_at: r.created_at,
            }
        })
        .collect())
}

/// One tap as the page shows it.
pub async fn get(db: &SqlitePool, learned: &Learned, id: i64) -> Result<Option<Tap>, sqlx::Error> {
    let rows: Vec<TapRow> = sqlx::query_as(&format!("{SELECT} WHERE t.id = ?"))
        .bind(id)
        .fetch_all(db)
        .await?;
    Ok(shown(db, learned, rows).await?.pop())
}

/// Which taps to list: `None` for all of them.
pub async fn list(
    db: &SqlitePool,
    learned: &Learned,
    status: Option<Status>,
) -> Result<Vec<Tap>, sqlx::Error> {
    let filter = match status {
        Some(Status::Unmatched) => "WHERE NOT t.dismissed AND t.transaction_id IS NULL",
        Some(Status::Reconciled) => "WHERE NOT t.dismissed AND t.transaction_id IS NOT NULL",
        Some(Status::Dismissed) => "WHERE t.dismissed",
        None => "",
    };
    let rows: Vec<TapRow> = sqlx::query_as(&format!(
        "{SELECT} {filter} ORDER BY t.tapped_on DESC, t.id DESC"
    ))
    .fetch_all(db)
    .await?;
    shown(db, learned, rows).await
}

/// What the shortcut sends. Fields are loosely typed because it sends whatever the wallet
/// trigger provides.
#[derive(Debug, Clone, Default)]
pub struct Reported {
    pub merchant: Option<String>,
    pub amount: Value,
    pub date: Value,
    pub card: Option<String>,
    pub currency: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TapError {
    #[error("Tap not found")]
    NotFound,
    /// The shortcut sent something that can't be read.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Records a tap the shortcut reported, linking it straight away if the statement is already
/// in. Its id.
pub async fn record(
    db: &SqlitePool,
    http: &dyn Transport,
    reported: Reported,
    today: NaiveDate,
) -> Result<i64, TapError> {
    let merchant = reported.merchant.as_deref().unwrap_or_default().trim();
    if merchant.is_empty() {
        return Err(TapError::Invalid("merchant is required".into()));
    }
    let (amount, detected) = parse::amount(&reported.amount).map_err(TapError::Invalid)?;
    let tapped_on = parse::date(&reported.date, today).map_err(TapError::Invalid)?;
    if amount.is_zero() {
        return Err(TapError::Invalid("amount must not be zero".into()));
    }
    let amount = Money::from_decimal(amount.round_dp(4))
        .map_err(|_| TapError::Invalid(format!("could not parse amount {amount}")))?;
    let currency = reported
        .currency
        .filter(|c| !c.is_empty())
        .or(detected)
        .unwrap_or_else(|| "GBP".into())
        .trim()
        .to_uppercase();
    let card = reported
        .card
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO card_taps (tapped_on, merchant, amount, currency, card_name)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(tapped_on)
    .bind(merchant)
    .bind(amount)
    .bind(currency)
    .bind(card)
    .fetch_one(db)
    .await?;
    // The statement may already have been imported if the tap is sent late.
    reconcile(db, http, false).await?;
    Ok(id)
}

async fn exists(db: &SqlitePool, id: i64) -> Result<(), TapError> {
    let found: Option<i64> = sqlx::query_scalar("SELECT id FROM card_taps WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?;
    found.map(|_| ()).ok_or(TapError::NotFound)
}

/// Not spending to look for on a statement, e.g. a payment that was cancelled.
pub async fn dismiss(db: &SqlitePool, id: i64) -> Result<(), TapError> {
    exists(db, id).await?;
    sqlx::query("UPDATE card_taps SET dismissed = 1, transaction_id = NULL WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn restore(db: &SqlitePool, http: &dyn Transport, id: i64) -> Result<(), TapError> {
    exists(db, id).await?;
    sqlx::query("UPDATE card_taps SET dismissed = 0 WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    reconcile(db, http, true).await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: i64) -> Result<(), TapError> {
    exists(db, id).await?;
    sqlx::query("DELETE FROM card_taps WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// A card name the wallet reported, and the account it's paired with.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    pub card_name: String,
    pub account_id: Option<i64>,
    pub account_full_path: Option<String>,
    pub tap_count: i64,
}

/// Every card name seen in a tap or paired, with its pairing.
pub async fn cards(db: &SqlitePool) -> Result<Vec<Card>, sqlx::Error> {
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT card_name, COUNT(*) FROM card_taps WHERE card_name IS NOT NULL
         GROUP BY card_name",
    )
    .fetch_all(db)
    .await?;
    let paired: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT w.card_name, a.id, a.full_path FROM wallet_cards w
         JOIN accounts a ON a.id = w.account_id",
    )
    .fetch_all(db)
    .await?;
    let mut cards: BTreeMap<(String, String), Card> = BTreeMap::new();
    let card = |name: &str| -> Card {
        Card {
            card_name: name.to_owned(),
            account_id: None,
            account_full_path: None,
            tap_count: 0,
        }
    };
    for (name, count) in counts {
        let entry = card(&name);
        cards
            .entry((name.to_lowercase(), name))
            .or_insert(entry)
            .tap_count = count;
    }
    for (name, account, path) in paired {
        let entry = card(&name);
        let found = cards.entry((name.to_lowercase(), name)).or_insert(entry);
        found.account_id = Some(account);
        found.account_full_path = Some(path);
    }
    Ok(cards.into_values().collect())
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("card_name is required")]
    NoName,
    #[error("Account not found")]
    AccountNotFound,
    #[error("Cards can only be paired with asset or liability accounts")]
    NotACardAccount,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Pairs a wallet card with an asset or liability account, or with `None`, unpairs it. Taps
/// linked under the old pairing are linked again.
pub async fn pair(
    db: &SqlitePool,
    http: &dyn Transport,
    card_name: &str,
    account_id: Option<i64>,
) -> Result<Card, PairError> {
    let card_name = card_name.trim();
    if card_name.is_empty() {
        return Err(PairError::NoName);
    }
    let mut account_full_path = None;
    if let Some(id) = account_id {
        let account: Option<(String, String)> =
            sqlx::query_as("SELECT full_path, type FROM accounts WHERE id = ?")
                .bind(id)
                .fetch_optional(db)
                .await?;
        let (path, kind) = account.ok_or(PairError::AccountNotFound)?;
        if !["asset", "liability"].contains(&kind.as_str()) {
            return Err(PairError::NotACardAccount);
        }
        account_full_path = Some(path);
    }
    let mut tx = db.begin().await?;
    match account_id {
        Some(id) => sqlx::query(
            "INSERT INTO wallet_cards (card_name, account_id) VALUES (?, ?)
             ON CONFLICT (card_name) DO UPDATE SET account_id = excluded.account_id",
        )
        .bind(card_name)
        .bind(id),
        None => sqlx::query("DELETE FROM wallet_cards WHERE card_name = ?").bind(card_name),
    }
    .execute(&mut *tx)
    .await?;
    // Links made under the old pairing may no longer be valid.
    sqlx::query("UPDATE card_taps SET transaction_id = NULL WHERE card_name = ?")
        .bind(card_name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    reconcile(db, http, true).await?;
    let tap_count = sqlx::query_scalar("SELECT COUNT(*) FROM card_taps WHERE card_name = ?")
        .bind(card_name)
        .fetch_one(db)
        .await?;
    Ok(Card {
        card_name: card_name.to_owned(),
        account_id,
        account_full_path,
        tap_count,
    })
}
