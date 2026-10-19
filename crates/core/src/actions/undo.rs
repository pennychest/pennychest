//! Reversing recorded changes. Each undo checks that the data still looks the way the change
//! left it, and refuses with an explanation rather than guessing if it doesn't. It runs in a
//! transaction, so a refusal part way through leaves no trace.

use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};

use super::ledger::{Added, Deleted, Edited, Reviewed, update_posting};
use super::write::{
    self, BudgetDeleted, BudgetSet, CreatedCategory, CreatedRule, Recategorised, Renamed,
    stored_amount,
};

#[derive(Debug, thiserror::Error)]
pub enum UndoError {
    #[error("Change not found")]
    NotFound,
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn refuse(message: impl Into<String>) -> UndoError {
    UndoError::Refused(message.into())
}

fn read<T: DeserializeOwned>(data: Value) -> Result<T, UndoError> {
    serde_json::from_value(data).map_err(|_| refuse("This change's undo record is damaged."))
}

/// The actions `reverse` undoes: every action that changes data, as a test checks.
#[cfg(test)]
pub const UNDOABLE: [&str; 11] = [
    "create_category",
    "rename_category",
    "create_rule",
    "set_budget",
    "delete_budget",
    "recategorise_transactions",
    "add_transaction",
    "add_transfer",
    "edit_transaction",
    "delete_transactions",
    "mark_reviewed",
];

pub async fn reverse(
    db: &mut SqliteConnection,
    action: &str,
    data: Value,
) -> Result<(), UndoError> {
    match action {
        "create_category" => create_category(db, read(data)?).await,
        "rename_category" => rename_category(db, read(data)?).await,
        "create_rule" => create_rule(db, read(data)?).await,
        "set_budget" => set_budget(db, read(data)?).await,
        "delete_budget" => delete_budget(db, read(data)?).await,
        "recategorise_transactions" => recategorise(db, read(data)?).await,
        "add_transaction" | "add_transfer" => add(db, read(data)?).await,
        "edit_transaction" => edit(db, read(data)?).await,
        "delete_transactions" => delete_transactions(db, read(data)?).await,
        "mark_reviewed" => mark_reviewed(db, read(data)?).await,
        _ => Err(refuse(format!("Changes made by {action} can't be undone."))),
    }
}

async fn exists(db: &mut SqliteConnection, sql: &str, id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!("SELECT EXISTS ({sql})"))
        .bind(id)
        .fetch_one(db)
        .await
}

async fn account(
    db: &mut SqliteConnection,
    id: i64,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT full_path, name FROM accounts WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

async fn create_category(
    db: &mut SqliteConnection,
    data: CreatedCategory,
) -> Result<(), UndoError> {
    let id = data.account_id;
    let Some((path, _)) = account(db, id).await? else {
        return Err(refuse("That category has already been deleted."));
    };
    let mut uses = Vec::new();
    for (what, sql) in [
        (
            "transactions",
            "SELECT 1 FROM postings WHERE account_id = ?",
        ),
        (
            "subcategories",
            "SELECT 1 FROM accounts WHERE parent_id = ?",
        ),
        ("rules", "SELECT 1 FROM rules WHERE target_account_id = ?"),
        ("budgets", "SELECT 1 FROM budgets WHERE account_id = ?"),
    ] {
        if exists(db, sql, id).await? {
            uses.push(what);
        }
    }
    if !uses.is_empty() {
        return Err(refuse(format!(
            "{path} now has {}, so it wasn't removed.",
            uses.join(" and ")
        )));
    }
    sqlx::query("DELETE FROM accounts WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

async fn rename_category(db: &mut SqliteConnection, data: Renamed) -> Result<(), UndoError> {
    let Some((path, _)) = account(db, data.account_id).await? else {
        return Err(refuse("That category has since been deleted."));
    };
    if path != data.to_path {
        return Err(refuse(format!(
            "It has been renamed again since (it's now {path})."
        )));
    }
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE full_path = ?)")
            .bind(&data.from_path)
            .fetch_one(&mut *db)
            .await?;
    if taken {
        return Err(refuse(format!(
            "Another category is now called {}.",
            data.from_path
        )));
    }
    write::move_account(db, data.account_id, &path, &data.from_path, &data.from_name).await?;
    Ok(())
}

async fn create_rule(db: &mut SqliteConnection, data: CreatedRule) -> Result<(), UndoError> {
    let deleted = sqlx::query("DELETE FROM rules WHERE id = ?")
        .bind(data.rule_id)
        .execute(db)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(refuse("That rule has already been deleted."));
    }
    Ok(())
}

fn amount(text: &str) -> Result<crate::Money, UndoError> {
    stored_amount(text).ok_or_else(|| refuse("This change's undo record is damaged."))
}

async fn put_budget(
    db: &mut SqliteConnection,
    account_id: i64,
    period_id: i64,
    limit: crate::Money,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO budgets (account_id, period_id, amount) VALUES (?, ?, ?)
         ON CONFLICT (account_id, period_id) DO UPDATE SET amount = excluded.amount",
    )
    .bind(account_id)
    .bind(period_id)
    .bind(limit)
    .execute(db)
    .await?;
    Ok(())
}

async fn set_budget(db: &mut SqliteConnection, data: BudgetSet) -> Result<(), UndoError> {
    match data.previous {
        None => {
            sqlx::query("DELETE FROM budgets WHERE account_id = ? AND period_id = ?")
                .bind(data.account_id)
                .bind(data.period_id)
                .execute(db)
                .await?;
        }
        Some(previous) => {
            put_budget(db, data.account_id, data.period_id, amount(&previous)?).await?;
        }
    }
    Ok(())
}

async fn delete_budget(db: &mut SqliteConnection, data: BudgetDeleted) -> Result<(), UndoError> {
    if account(db, data.account_id).await?.is_none() {
        return Err(refuse("That category has since been deleted."));
    }
    let again: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM budgets WHERE account_id = ? AND period_id = ?)",
    )
    .bind(data.account_id)
    .bind(data.period_id)
    .fetch_one(&mut *db)
    .await?;
    if again {
        return Err(refuse("That category has a budget for this period again."));
    }
    put_budget(db, data.account_id, data.period_id, amount(&data.amount)?).await?;
    Ok(())
}

async fn recategorise(db: &mut SqliteConnection, data: Recategorised) -> Result<(), UndoError> {
    let mut restored = 0;
    for before in data.postings {
        if !exists(db, "SELECT 1 FROM accounts WHERE id = ?", before.account_id).await? {
            continue;
        }
        let rule_id = existing_rule(db, before.rule_id).await?;
        restored += sqlx::query(
            "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = ? WHERE id = ?",
        )
        .bind(before.account_id)
        .bind(before.categorised_by_id)
        .bind(rule_id)
        .bind(before.id)
        .execute(&mut *db)
        .await?
        .rows_affected();
    }
    if restored == 0 {
        return Err(refuse("Those transactions have since been deleted."));
    }
    Ok(())
}

/// The rule, unless it has been deleted since: a posting can't point at it then.
async fn existing_rule(
    db: &mut SqliteConnection,
    rule: Option<i64>,
) -> Result<Option<i64>, sqlx::Error> {
    Ok(match rule {
        Some(id) if exists(db, "SELECT 1 FROM rules WHERE id = ?", id).await? => Some(id),
        _ => None,
    })
}

async fn add(db: &mut SqliteConnection, data: Added) -> Result<(), UndoError> {
    let deleted = sqlx::query("DELETE FROM transactions WHERE id = ?")
        .bind(data.transaction_id)
        .execute(db)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(refuse("That transaction has already been deleted."));
    }
    Ok(())
}

async fn edit(db: &mut SqliteConnection, data: Edited) -> Result<(), UndoError> {
    let before = data.before;
    if !exists(db, "SELECT 1 FROM transactions WHERE id = ?", before.id).await? {
        return Err(refuse("That transaction has since been deleted."));
    }
    let mut now: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM postings WHERE transaction_id = ? ORDER BY id")
            .bind(before.id)
            .fetch_all(&mut *db)
            .await?;
    let mut then: Vec<i64> = before.postings.iter().map(|p| p.id).collect();
    now.sort_unstable();
    then.sort_unstable();
    if now != then {
        return Err(refuse("That transaction has been split or rebuilt since."));
    }
    sqlx::query("UPDATE transactions SET date = ?, description = ? WHERE id = ?")
        .bind(before.date)
        .bind(&before.description)
        .bind(before.id)
        .execute(&mut *db)
        .await?;
    for p in &before.postings {
        let rule_id = existing_rule(db, p.rule_id).await?;
        if !exists(db, "SELECT 1 FROM accounts WHERE id = ?", p.account_id).await? {
            return Err(refuse("An account it used has since been deleted."));
        }
        update_posting(
            db,
            p.id,
            p.account_id,
            p.amount,
            p.categorised_by_id,
            rule_id,
        )
        .await?;
    }
    Ok(())
}

fn push_ids(query: &mut QueryBuilder<'_, Sqlite>, ids: &[i64]) {
    let mut list = query.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    query.push(")");
}

/// Points rows that pointed at a transaction before it was deleted back at it, unless
/// something else has claimed them since.
async fn relink(
    db: &mut SqliteConnection,
    table: &str,
    column: &str,
    ids: &[i64],
    transaction_id: i64,
) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::new(format!("UPDATE {table} SET {column} = "));
    query.push_bind(transaction_id);
    query.push(format!(" WHERE {column} IS NULL AND id IN ("));
    push_ids(&mut query, ids);
    query.build().execute(db).await?;
    Ok(())
}

async fn delete_transactions(db: &mut SqliteConnection, data: Deleted) -> Result<(), UndoError> {
    for snap in &data.transactions {
        if exists(db, "SELECT 1 FROM transactions WHERE id = ?", snap.id).await? {
            return Err(refuse(format!(
                "Transaction {} already exists again.",
                snap.id
            )));
        }
        for p in &snap.postings {
            if !exists(db, "SELECT 1 FROM accounts WHERE id = ?", p.account_id).await? {
                return Err(refuse(
                    "An account one of the transactions used has since been deleted.",
                ));
            }
        }
    }
    for snap in &data.transactions {
        let batch = match snap.import_batch_id {
            Some(id) if exists(db, "SELECT 1 FROM import_batches WHERE id = ?", id).await? => {
                Some(id)
            }
            _ => None,
        };
        let peer = match snap.transfer_peer_id {
            Some(id) if exists(db, "SELECT 1 FROM transactions WHERE id = ?", id).await? => {
                Some(id)
            }
            _ => None,
        };
        sqlx::query(
            "INSERT INTO transactions (id, date, description, status, currency, import_batch_id,
                import_metadata, transfer_peer_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(snap.id)
        .bind(snap.date)
        .bind(&snap.description)
        .bind(&snap.status)
        .bind(&snap.currency)
        .bind(batch)
        .bind(snap.import_metadata.as_ref().map(Value::to_string))
        .bind(peer)
        .execute(&mut *db)
        .await?;
        for p in &snap.postings {
            let rule_id = existing_rule(db, p.rule_id).await?;
            sqlx::query(
                "INSERT INTO postings (id, transaction_id, account_id, amount, categorised_by_id,
                    rule_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(p.id)
            .bind(snap.id)
            .bind(p.account_id)
            .bind(p.amount)
            .bind(p.categorised_by_id)
            .bind(rule_id)
            .execute(&mut *db)
            .await?;
        }
        relink(
            db,
            "raw_import_rows",
            "transaction_id",
            &snap.raw_row_ids,
            snap.id,
        )
        .await?;
        relink(db, "card_taps", "transaction_id", &snap.tap_ids, snap.id).await?;
        relink(
            db,
            "transactions",
            "transfer_peer_id",
            &snap.peer_of,
            snap.id,
        )
        .await?;
    }
    Ok(())
}

async fn mark_reviewed(db: &mut SqliteConnection, data: Reviewed) -> Result<(), UndoError> {
    if data.transaction_ids.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::new("UPDATE transactions SET status = 'pending' WHERE id IN (");
    push_ids(&mut query, &data.transaction_ids);
    query.build().execute(db).await?;
    Ok(())
}
