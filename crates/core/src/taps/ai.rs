//! Suggesting a category for each card tap with the model the "taps" task uses, and giving
//! the statement line a tap is matched to that category.
//!
//! This runs after the tap has been saved and the phone has had its reply, so a slow or
//! failing model never delays or breaks a tap.

use std::time::Instant;

use sqlx::SqlitePool;

use crate::Money;
use crate::ai::categorise::categorise_transactions;
use crate::ai::config::Task;
use crate::ai::http::Transport;
use crate::ai::log_call;
use crate::ai::providers::{self, ResolveError};
use crate::imports::batches::UNCATEGORISED_PATH;
use crate::learned::Learned;
use crate::lookups::CategorisationSource;
use crate::rules::RuleSet;

/// Gives the statement transaction a tap is matched to the tap's AI category, but only if the
/// import left it uncategorised. Whether anything changed.
pub async fn apply_category(db: &SqlitePool, tap_id: i64) -> Result<bool, sqlx::Error> {
    let tap: Option<(Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT transaction_id, ai_account_id FROM card_taps WHERE id = ?")
            .bind(tap_id)
            .fetch_optional(db)
            .await?;
    let Some((Some(transaction), Some(category))) = tap else {
        return Ok(false);
    };
    let uncategorised: Vec<i64> = sqlx::query_scalar(
        "SELECT p.id FROM postings p JOIN accounts a ON a.id = p.account_id
         WHERE p.transaction_id = ? AND a.full_path = ? AND p.categorised_by_id = ?",
    )
    .bind(transaction)
    .bind(UNCATEGORISED_PATH)
    .bind(CategorisationSource::ImportDefault as i64)
    .fetch_all(db)
    .await?;
    let [posting] = uncategorised[..] else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = NULL WHERE id = ?",
    )
    .bind(category)
    .bind(CategorisationSource::Ai as i64)
    .bind(posting)
    .execute(db)
    .await?;
    Ok(true)
}

#[derive(Debug, thiserror::Error)]
pub enum CategoriseError {
    /// What to set up, or what went wrong with the model, which is in the AI logs.
    #[error("{0}")]
    Model(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Asks the model for a tap's spending category and remembers it on the tap. Skipped if one of
/// the user's rules covers the merchant, or their own history already knows it.
pub async fn categorise(
    db: &SqlitePool,
    http: &dyn Transport,
    learned: &Learned,
    tap_id: i64,
) -> Result<(), CategoriseError> {
    let tap: Option<(String, Money, String, Option<i64>)> = sqlx::query_as(
        "SELECT merchant, amount, currency, ai_account_id FROM card_taps WHERE id = ?",
    )
    .bind(tap_id)
    .fetch_optional(db)
    .await?;
    let Some((merchant, amount, currency, None)) = tap else {
        return Ok(());
    };
    let mut conn = db.acquire().await?;
    if RuleSet::load(&mut conn)
        .await?
        .first_match(&merchant)
        .is_some()
    {
        return Ok(());
    }
    drop(conn);
    let known = learned
        .suggest(db, std::slice::from_ref(&merchant))
        .await
        .map_err(|e| CategoriseError::Model(e.to_string()))?;
    if known.first().is_some_and(Option::is_some) {
        return Ok(());
    }
    let model = match providers::resolve_task(db, Task::Taps).await {
        Ok(model) => model,
        Err(ResolveError::Provider(e)) => return Err(CategoriseError::Model(e.message)),
        Err(ResolveError::Setting(e)) => return Err(CategoriseError::Model(e.to_string())),
    };
    let categories: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, full_path FROM accounts
         WHERE type = 'expense' AND instr(full_path, ':') > 0 AND full_path != ?",
    )
    .bind(UNCATEGORISED_PATH)
    .fetch_all(db)
    .await?;
    if categories.is_empty() {
        return Ok(());
    }
    let paths: Vec<String> = categories.iter().map(|(_, p)| p.clone()).collect();
    let refund = if amount.is_negative() { ", refund" } else { "" };
    let description = format!(
        "{merchant} ({currency} {}{refund})",
        amount.abs().two_places()
    );

    let started = Instant::now();
    let label = model.label();
    let answered = categorise_transactions(http, &model, &[(tap_id, description)], &paths).await;
    let results = match answered {
        Ok((results, request, response)) => {
            log_call(
                db,
                &label,
                "tap_categorise",
                started,
                Some(&request),
                Some(&response),
                None,
            )
            .await;
            results
        }
        Err((e, request, response)) => {
            let (request, response) = (
                Some(&request).filter(|r| !r.is_null()),
                Some(&response).filter(|r| !r.is_null()),
            );
            log_call(
                db,
                &label,
                "tap_categorise",
                started,
                request,
                response,
                Some(&e.message),
            )
            .await;
            return Err(CategoriseError::Model(e.message));
        }
    };
    let Some(chosen) = results.first() else {
        return Ok(());
    };
    let account = categories
        .iter()
        .find(|(_, path)| *path == chosen.account_full_path)
        .map(|(id, _)| *id);
    sqlx::query("UPDATE card_taps SET ai_account_id = ?, ai_confidence = ? WHERE id = ?")
        .bind(account)
        .bind(chosen.confidence)
        .bind(tap_id)
        .execute(db)
        .await?;
    apply_category(db, tap_id).await?;
    Ok(())
}
