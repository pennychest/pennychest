//! Actions that change the ledger itself. All need the `transactions` scope.
//!
//! Imported transactions can be changed or deleted like any other; the statement rows they
//! came from stay with their import, so what the bank reported is never lost. The
//! descriptions steer models towards changing an imported transaction's category rather than
//! its bank details.

use chrono::{Local, NaiveDate};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::common::{
    self, Loaded, category_amount, count_of, find_money_account, load_one, unique,
};
use super::{Action, ActionError, Check, Output, Scope, items, length, parse, schema};
use crate::Money;
use crate::accounts;
use crate::ledger::{self, Balanced, NewPosting, NewTransaction, Status};
use crate::lookups::CategorisationSource;

pub fn actions() -> Vec<Action> {
    vec![
        action!(
            add_transaction,
            AddTransactionInput,
            Scope::Transactions,
            "Record a payment or income the user tells you about, such as a cash purchase. It \
             is saved as reviewed. Use add_transfer for money moved between the user's own \
             accounts."
        ),
        action!(
            add_transfer,
            AddTransferInput,
            Scope::Transactions,
            "Record money moved between two of the user's own accounts, such as paying off a \
             credit card or moving money into savings. Transfers aren't spending, so they have \
             no category."
        ),
        action!(
            edit_transaction,
            EditTransactionInput,
            Scope::Transactions,
            "Change a transaction's description, date, amount, category or account. Imported \
             transactions (\"imported\": true) came from the user's bank statement: to fix how \
             they're categorised change only the category, and only change their amount, date, \
             account or description when the user asks for that specifically. Transfers can \
             change amount, date and description but have no category."
        ),
        action!(
            delete_transactions,
            TransactionIdsInput,
            Scope::Transactions,
            "Delete transactions, including transfers. Imported transactions (\"imported\": \
             true) came from the user's bank statement, so only delete one when the user asks \
             for it specifically, for example a duplicate; to fix how one is categorised, \
             change its category instead."
        ),
        action!(
            mark_reviewed,
            TransactionIdsInput,
            Scope::Transactions,
            "Mark imported transactions as reviewed, meaning the user has checked their \
             category."
        ),
    ]
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn no_transaction(id: i64) -> ActionError {
    ActionError::Invalid(format!("There is no transaction {id}."))
}

async fn get(db: &SqlitePool, id: i64) -> Result<Loaded, ActionError> {
    load_one(db, id).await?.ok_or_else(|| no_transaction(id))
}

async fn describe(db: &SqlitePool, id: i64) -> Result<common::Described, ActionError> {
    Ok(get(db, id).await?.describe())
}

/// A posting as a snapshot keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostingSnapshot {
    pub id: i64,
    pub account_id: i64,
    pub amount: Money,
    pub categorised_by_id: i64,
    pub rule_id: Option<i64>,
}

/// Everything needed to put a transaction back exactly as it was, including what pointed at
/// it (statement rows, card taps and a transfer partner).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: i64,
    pub date: NaiveDate,
    pub description: String,
    pub status: String,
    pub currency: String,
    pub import_batch_id: Option<i64>,
    pub import_metadata: Option<Value>,
    pub transfer_peer_id: Option<i64>,
    pub postings: Vec<PostingSnapshot>,
    pub raw_row_ids: Vec<i64>,
    pub tap_ids: Vec<i64>,
    pub peer_of: Vec<i64>,
}

async fn snapshot(db: &SqlitePool, txn: &Loaded) -> Result<Snapshot, sqlx::Error> {
    let id = txn.header.id;
    let (currency, import_metadata, transfer_peer_id): (String, Option<String>, Option<i64>) =
        sqlx::query_as(
            "SELECT currency, import_metadata, transfer_peer_id FROM transactions WHERE id = ?",
        )
        .bind(id)
        .fetch_one(db)
        .await?;
    let ids = |sql: &'static str| async move {
        sqlx::query_scalar::<_, i64>(sql)
            .bind(id)
            .fetch_all(db)
            .await
    };
    Ok(Snapshot {
        id,
        date: txn.header.date,
        description: txn.header.description.clone(),
        status: txn.header.status.clone(),
        currency,
        import_batch_id: txn.header.import_batch_id,
        import_metadata: import_metadata.and_then(|m| serde_json::from_str(&m).ok()),
        transfer_peer_id,
        postings: txn
            .legs
            .iter()
            .map(|l| PostingSnapshot {
                id: l.id,
                account_id: l.account_id,
                amount: l.amount,
                categorised_by_id: l.categorised_by_id,
                rule_id: l.rule_id,
            })
            .collect(),
        raw_row_ids: ids("SELECT id FROM raw_import_rows WHERE transaction_id = ? ORDER BY id")
            .await?,
        tap_ids: ids("SELECT id FROM card_taps WHERE transaction_id = ? ORDER BY id").await?,
        peer_of: ids("SELECT id FROM transactions WHERE transfer_peer_id = ? ORDER BY id").await?,
    })
}

#[derive(Serialize, Deserialize)]
pub struct Added {
    pub transaction_id: i64,
}

async fn add(
    db: &SqlitePool,
    date: NaiveDate,
    description: &str,
    currency: &str,
    legs: [(i64, Money); 2],
) -> Result<i64, ActionError> {
    let postings = legs
        .into_iter()
        .map(|(account_id, amount)| NewPosting {
            account_id,
            amount,
            categorised_by: CategorisationSource::Ai,
            rule_id: None,
        })
        .collect();
    let new = NewTransaction {
        date,
        description: description.trim().to_owned(),
        status: Status::Confirmed,
        currency: currency.to_owned(),
        import_batch_id: None,
        import_metadata: None,
        postings: Balanced::new(postings).map_err(|e| ActionError::Invalid(e.to_string()))?,
    };
    let mut conn = db.acquire().await?;
    ledger::insert(&mut conn, &new).await.map_err(|e| match e {
        ledger::LedgerError::Database(e) => ActionError::Database(e),
        other => ActionError::Invalid(other.to_string()),
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTransactionInput {
    /// e.g. "Coffee at Pret".
    #[schemars(length(min = 1, max = 200))]
    description: String,
    /// How much was spent (for a spending category) or received (for an income category).
    /// Use a negative amount for a refund.
    amount: Money,
    /// Spending or income category (path or name).
    category: String,
    /// The bank account, card or cash account it was paid from or into.
    account: String,
    /// Defaults to today.
    #[serde(default)]
    date: Option<NaiveDate>,
}

impl Check for AddTransactionInput {
    fn check(&self) -> Result<(), String> {
        length("description", &self.description, 1, 200)
    }
}

pub async fn add_transaction(
    db: &SqlitePool,
    input: AddTransactionInput,
) -> Result<Output, ActionError> {
    if input.amount.is_zero() {
        return Err(ActionError::invalid("The amount can't be zero."));
    }
    let category = accounts::find_category(db, &input.category).await?;
    let account = find_money_account(db, &input.account).await?;
    let amount = category_amount(category.kind, input.amount);
    let id = add(
        db,
        input.date.unwrap_or_else(today),
        &input.description,
        &account.currency,
        [(category.id, amount), (account.id, -amount)],
    )
    .await?;
    let added = describe(db, id).await?;
    let summary = format!(
        "Added \"{}\" (£{}, {}) on {}",
        added.description,
        added.amount,
        added.category.as_deref().unwrap_or_default(),
        added.date
    );
    Ok(Output::new(json!({ "added": added })).with_change(summary, Added { transaction_id: id }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTransferInput {
    /// How much was moved.
    amount: Money,
    /// Account the money left (path or name).
    from_account: String,
    /// Account the money arrived in (path or name).
    to_account: String,
    #[serde(default = "transfer")]
    #[schemars(length(min = 1, max = 200))]
    description: String,
    /// Defaults to today.
    #[serde(default)]
    date: Option<NaiveDate>,
}

fn transfer() -> String {
    "Transfer".into()
}

impl Check for AddTransferInput {
    fn check(&self) -> Result<(), String> {
        if !self.amount.is_positive() {
            return Err("amount: must be greater than 0".into());
        }
        length("description", &self.description, 1, 200)
    }
}

pub async fn add_transfer(db: &SqlitePool, input: AddTransferInput) -> Result<Output, ActionError> {
    let source = find_money_account(db, &input.from_account).await?;
    let destination = find_money_account(db, &input.to_account).await?;
    if source.id == destination.id {
        return Err(ActionError::invalid("The two accounts must be different."));
    }
    let id = add(
        db,
        input.date.unwrap_or_else(today),
        &input.description,
        &source.currency,
        [(source.id, -input.amount), (destination.id, input.amount)],
    )
    .await?;
    Ok(
        Output::new(json!({ "added": describe(db, id).await? })).with_change(
            format!(
                "Added a £{} transfer from {} to {}",
                input.amount.two_places(),
                source.full_path,
                destination.full_path
            ),
            Added { transaction_id: id },
        ),
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditTransactionInput {
    transaction_id: i64,
    #[serde(default)]
    #[schemars(length(min = 1, max = 200))]
    description: Option<String>,
    #[serde(default)]
    date: Option<NaiveDate>,
    /// New amount, meaning the same as in add_transaction (or the amount moved, for a
    /// transfer).
    #[serde(default)]
    amount: Option<Money>,
    /// New category (path or name).
    #[serde(default)]
    category: Option<String>,
    /// New bank account or card it was paid from or into.
    #[serde(default)]
    account: Option<String>,
}

impl Check for EditTransactionInput {
    fn check(&self) -> Result<(), String> {
        match &self.description {
            Some(description) => length("description", description, 1, 200),
            None => Ok(()),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Edited {
    pub before: Snapshot,
}

pub async fn edit_transaction(
    db: &SqlitePool,
    input: EditTransactionInput,
) -> Result<Output, ActionError> {
    let txn = get(db, input.transaction_id).await?;
    if input.amount.is_some_and(Money::is_zero) {
        return Err(ActionError::invalid("The amount can't be zero."));
    }
    let before = snapshot(db, &txn).await?;
    let changed: Vec<&str> = [
        ("description", input.description.is_some()),
        ("date", input.date.is_some()),
        ("amount", input.amount.is_some()),
        ("category", input.category.is_some()),
        ("account", input.account.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, given)| given.then_some(name))
    .collect();

    // Each posting's new account, amount, source and rule, worked out before anything is
    // written, so a refusal changes nothing.
    let mut legs = txn.legs.clone();
    let category_index = legs.iter().position(|l| common::is_category(l.kind));
    let money_index = legs.iter().position(|l| common::is_money_account(l.kind));
    let categories = txn.categories().len();
    let money_side = txn.money_side().len();
    let simple = categories == 1 && money_side == 1 && legs.len() == 2;
    let is_transfer = categories == 0 && money_side == 2;

    if input.category.is_some() || input.account.is_some() {
        let (Some(c), Some(m), true) = (category_index, money_index, simple) else {
            return Err(ActionError::invalid(
                "Only transactions with one category and one account can have their category \
                 or account changed here.",
            ));
        };
        if let Some(reference) = &input.category {
            let new_category = accounts::find_category(db, reference).await?;
            if new_category.kind != legs[c].kind {
                // Moving between spending and income flips which way the money counts.
                let spent = category_amount(legs[c].kind, legs[c].amount);
                legs[c].amount = category_amount(new_category.kind, spent);
                legs[m].amount = -legs[c].amount;
            }
            legs[c].account_id = new_category.id;
            legs[c].kind = new_category.kind;
            legs[c].categorised_by_id = CategorisationSource::Ai as i64;
            legs[c].rule_id = None;
        }
        if let Some(reference) = &input.account {
            legs[m].account_id = find_money_account(db, reference).await?.id;
        }
    }

    if let Some(amount) = input.amount {
        if let (true, Some(c), Some(m)) = (simple, category_index, money_index) {
            legs[c].amount = category_amount(legs[c].kind, amount);
            legs[m].amount = -legs[c].amount;
        } else if is_transfer {
            if amount.is_negative() {
                return Err(ActionError::invalid(
                    "A transfer's amount must be positive.",
                ));
            }
            let outgoing = if legs[0].amount <= legs[1].amount {
                0
            } else {
                1
            };
            legs[outgoing].amount = -amount;
            legs[1 - outgoing].amount = amount;
        } else {
            return Err(ActionError::invalid(
                "This transaction is split, so its amount can't be changed here.",
            ));
        }
    }

    let mut tx = db.begin().await?;
    sqlx::query("UPDATE transactions SET description = ?, date = ? WHERE id = ?")
        .bind(
            input
                .description
                .as_deref()
                .map_or(txn.header.description.clone(), |d| d.trim().to_owned()),
        )
        .bind(input.date.unwrap_or(txn.header.date))
        .bind(txn.header.id)
        .execute(&mut *tx)
        .await?;
    for leg in &legs {
        update_posting(
            &mut tx,
            leg.id,
            leg.account_id,
            leg.amount,
            leg.categorised_by_id,
            leg.rule_id,
        )
        .await?;
    }
    tx.commit().await?;

    let output = Output::new(json!({ "edited": describe(db, txn.header.id).await? }));
    if changed.is_empty() {
        return Ok(output);
    }
    let summary = format!(
        "Edited the {} of \"{}\" ({})",
        changed.join(", "),
        before.description,
        before.date
    );
    Ok(output.with_change(summary, Edited { before }))
}

pub(super) async fn update_posting(
    db: &mut sqlx::SqliteConnection,
    id: i64,
    account_id: i64,
    amount: Money,
    categorised_by_id: i64,
    rule_id: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE postings SET account_id = ?, amount = ?, categorised_by_id = ?, rule_id = ?
         WHERE id = ?",
    )
    .bind(account_id)
    .bind(amount)
    .bind(categorised_by_id)
    .bind(rule_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransactionIdsInput {
    /// IDs from list_transactions.
    #[schemars(length(min = 1, max = 200))]
    transaction_ids: Vec<i64>,
}

impl Check for TransactionIdsInput {
    fn check(&self) -> Result<(), String> {
        items("transaction_ids", &self.transaction_ids, 1, 200)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Deleted {
    pub transactions: Vec<Snapshot>,
}

pub async fn delete_transactions(
    db: &SqlitePool,
    input: TransactionIdsInput,
) -> Result<Output, ActionError> {
    let ids = unique(&input.transaction_ids);
    let loaded = common::load(db, &ids).await?;
    let (mut deleted, mut skipped, mut snapshots) = (Vec::new(), Vec::new(), Vec::new());
    for id in &ids {
        match loaded.get(id) {
            Some(txn) => {
                deleted.push(txn.describe());
                snapshots.push(snapshot(db, txn).await?);
            }
            None => skipped.push(json!({ "id": id, "reason": "not found" })),
        }
    }
    // Statement rows, card taps and transfer partners are unlinked by their foreign keys.
    let mut tx = db.begin().await?;
    for snapshot in &snapshots {
        ledger::delete(&mut *tx, snapshot.id)
            .await
            .map_err(|e| match e {
                ledger::LedgerError::Database(e) => ActionError::Database(e),
                other => ActionError::Invalid(other.to_string()),
            })?;
    }
    tx.commit().await?;
    let what = match deleted.as_slice() {
        [] => None,
        [one] => Some(format!("\"{}\" ({})", one.description, one.date)),
        many => Some(format!("{} transactions", many.len())),
    };
    let output = Output::new(json!({ "deleted": deleted, "skipped": skipped }));
    Ok(match what {
        Some(what) => output.with_change(
            format!("Deleted {what}"),
            Deleted {
                transactions: snapshots,
            },
        ),
        None => output,
    })
}

#[derive(Serialize, Deserialize)]
pub struct Reviewed {
    pub transaction_ids: Vec<i64>,
}

pub async fn mark_reviewed(
    db: &SqlitePool,
    input: TransactionIdsInput,
) -> Result<Output, ActionError> {
    let ids = unique(&input.transaction_ids);
    let loaded = common::load(db, &ids).await?;
    let (mut reviewed, mut skipped) = (Vec::new(), Vec::new());
    for id in ids {
        match loaded.get(&id) {
            None => skipped.push(json!({ "id": id, "reason": "not found" })),
            Some(txn) if txn.header.status == "confirmed" => {
                skipped.push(json!({ "id": id, "reason": "already reviewed" }));
            }
            Some(_) => reviewed.push(id),
        }
    }
    ledger::confirm_many(db, &reviewed).await?;
    let output = Output::new(json!({ "reviewed": reviewed, "skipped": skipped }));
    if reviewed.is_empty() {
        return Ok(output);
    }
    Ok(output.with_change(
        format!(
            "Marked {} as reviewed",
            count_of(reviewed.len(), "transaction")
        ),
        Reviewed {
            transaction_ids: reviewed,
        },
    ))
}
