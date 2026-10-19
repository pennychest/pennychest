//! Linking card taps to the statement lines they turn into once a statement is imported.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{Duration, NaiveDate};
use regex::Regex;
use serde_json::json;
use sqlx::SqlitePool;

use super::ai::apply_category;
use crate::Money;
use crate::ai::config::{self, Task};
use crate::ai::http::Transport;
use crate::ai::matching::{MIN_PROBABILITY, ask_pairs};
use crate::ai::providers::{self, Resolved};

// Statements usually show a payment on the tap date or a few days after it (the posting
// date). Allow a day earlier for timezone and midnight edge cases.
pub const MATCH_DAYS_BEFORE: i64 = 1;
pub const MATCH_DAYS_AFTER: i64 = 7;

pub const TAP_QUESTION: &str = "Are {a} (a card payment the user's phone recorded at the till) \
                                and {b} (a line on their card statement) the same payment? \
                                Statements often show a payment processor prefix such as SQ * \
                                or SUMUP *, a reference number, a location, or the company's \
                                legal name instead of the shop's name.";

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new("[a-z0-9]+").expect("a valid pattern"));

fn words(text: &str) -> HashSet<String> {
    WORD.find_iter(&text.to_lowercase())
        .map(|w| w.as_str().to_owned())
        .filter(|w| w.len() >= 3)
        .collect()
}

#[derive(sqlx::FromRow)]
struct OpenTap {
    id: i64,
    tapped_on: NaiveDate,
    merchant: String,
    amount: Money,
    currency: String,
    card_name: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Line {
    id: i64,
    date: NaiveDate,
    description: String,
}

/// The candidate the matching model is confident is this tap, if any.
async fn ask_model(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    tap: &OpenTap,
    candidates: &[Line],
) -> Result<Option<i64>, providers::ProviderError> {
    let tap_side = json!({
        "merchant": tap.merchant,
        "date": tap.tapped_on.to_string(),
        "amount": tap.amount.to_string(),
    });
    let pairs: Vec<_> = candidates
        .iter()
        .map(|line| {
            let statement = json!({"description": line.description, "date": line.date.to_string()});
            (tap_side.clone(), statement)
        })
        .collect();
    let probabilities = ask_pairs(db, http, model, "tap_match", TAP_QUESTION, &pairs).await?;
    // The first of the most likely.
    let mut best: Option<(i64, f64)> = None;
    for (line, probability) in candidates.iter().zip(probabilities) {
        if best.is_none_or(|(_, p)| probability > p) {
            best = Some((line.id, probability));
        }
    }
    Ok(best
        .filter(|(_, p)| *p >= MIN_PROBABILITY)
        .map(|(id, _)| id))
}

/// Links unmatched taps to imported transactions, returning how many it linked.
///
/// A tap matches a transaction on its card's paired account whose posting there is the
/// negated tap amount, dated within the match window. When several transactions qualify, the
/// one whose description shares a word with the merchant wins. If that doesn't settle it and
/// a matching model is set up, the model picks, and the tap is left unmatched unless it's
/// confident; with `use_model` off such taps are left for a later reconcile that can ask it.
/// Otherwise the transaction closest to the tap date wins.
pub async fn reconcile(
    db: &SqlitePool,
    http: &dyn Transport,
    use_model: bool,
) -> Result<usize, sqlx::Error> {
    let cards: HashMap<String, (i64, String)> = sqlx::query_as::<_, (String, i64, String)>(
        "SELECT w.card_name, a.id, a.currency FROM wallet_cards w
         JOIN accounts a ON a.id = w.account_id",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|(card, account, currency)| (card, (account, currency)))
    .collect();
    let taps: Vec<OpenTap> = sqlx::query_as(
        "SELECT t.id, t.tapped_on, t.merchant, t.amount, t.currency, t.card_name
         FROM card_taps t JOIN wallet_cards w ON w.card_name = t.card_name
         WHERE t.transaction_id IS NULL AND NOT t.dismissed
         ORDER BY t.tapped_on, t.id",
    )
    .fetch_all(db)
    .await?;
    if taps.is_empty() {
        return Ok(0);
    }
    let mut claimed: HashSet<i64> =
        sqlx::query_scalar("SELECT transaction_id FROM card_taps WHERE transaction_id IS NOT NULL")
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();

    let mut model = None;
    if matches!(
        config::task_model(db, Task::Matching).await,
        Ok((Some(_), Some(_)))
    ) {
        model = providers::resolve_task(db, Task::Matching).await.ok();
    }
    let mut linked = 0;
    for tap in taps {
        let (account, currency) = &cards[&tap.card_name];
        if tap.currency != *currency {
            continue;
        }
        let candidates: Vec<Line> = sqlx::query_as(
            "SELECT DISTINCT t.id, t.date, t.description FROM transactions t
             JOIN postings p ON p.transaction_id = t.id
             WHERE p.account_id = ? AND p.amount = ? AND t.date BETWEEN ? AND ?
             ORDER BY t.id",
        )
        .bind(account)
        .bind(-tap.amount)
        .bind(tap.tapped_on - Duration::days(MATCH_DAYS_BEFORE))
        .bind(tap.tapped_on + Duration::days(MATCH_DAYS_AFTER))
        .fetch_all(db)
        .await?;
        let candidates: Vec<Line> = candidates
            .into_iter()
            .filter(|t| !claimed.contains(&t.id))
            .collect();
        if candidates.is_empty() {
            continue;
        }

        let merchant = words(&tap.merchant);
        let sharing: Vec<Line> = candidates
            .iter()
            .filter(|t| !merchant.is_disjoint(&words(&t.description)))
            .cloned()
            .collect();
        let pool = if sharing.is_empty() {
            &candidates
        } else {
            &sharing
        };
        let mut best = (sharing.len() == 1).then(|| sharing[0].id);
        if best.is_none()
            && candidates.len() > 1
            && let Some(ready) = &model
        {
            if !use_model {
                continue;
            }
            match ask_model(db, http, ready, &tap, pool).await {
                Ok(Some(id)) => best = Some(id),
                Ok(None) => continue,
                // Logged; the rest fall back to dates.
                Err(_) => model = None,
            }
        }
        let best = best.unwrap_or_else(|| {
            pool.iter()
                .min_by_key(|t| ((t.date - tap.tapped_on).num_days().abs(), t.id))
                .expect("there are candidates")
                .id
        });
        sqlx::query("UPDATE card_taps SET transaction_id = ? WHERE id = ?")
            .bind(best)
            .bind(tap.id)
            .execute(db)
            .await?;
        claimed.insert(best);
        // Imports leave unknown merchants uncategorised; the tap's AI category fills the gap.
        apply_category(db, tap.id).await?;
        linked += 1;
    }
    Ok(linked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_lowercase_and_at_least_three_letters() {
        let found = words("SQ *BTC Ltd 0231");
        assert_eq!(
            found,
            HashSet::from(["btc".into(), "ltd".into(), "0231".into()])
        );
    }
}
