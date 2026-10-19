//! The chart of accounts: a tree named by colon-separated paths, like `Expenses:Food:Groceries`.
//! Bank accounts and cards are assets and liabilities; categories are income and expense
//! accounts.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::{SqliteConnection, SqlitePool};

use crate::Money;
use crate::ledger::{self, Balanced, LedgerError, NewPosting, NewTransaction, Status};
use crate::lookups::{CategorisationSource, MatchType};

pub const OPENING_BALANCES_PATH: &str = "Equity:OpeningBalances";
const OPENING_BALANCE_DESCRIPTION: &str = "Opening Balance";
const DEFAULT_CURRENCY: &str = "GBP";

const SEED_ACCOUNTS: &str = include_str!("../seeds/uk_accounts.json");
const SEED_RULES: &str = include_str!("../seeds/uk_rules.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum AccountType {
    Asset,
    Liability,
    Income,
    Expense,
    Equity,
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub full_path: String,
    pub parent_id: Option<i64>,
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub kind: AccountType,
    pub currency: String,
    pub bank_identifier: Option<String>,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewAccount {
    pub name: String,
    pub full_path: String,
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(rename = "type")]
    pub kind: AccountType,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub bank_identifier: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// Changes to an account. Fields left out stay as they are; the ones that can be empty can
/// also be cleared with `null`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AccountUpdate {
    pub name: Option<String>,
    pub full_path: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub parent_id: Option<Option<i64>>,
    #[serde(rename = "type")]
    pub kind: Option<AccountType>,
    pub currency: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub bank_identifier: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub icon: Option<Option<String>>,
}

/// Tells a field given as `null` (`Some(None)`) from one left out (`None`).
fn present<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn default_currency() -> String {
    DEFAULT_CURRENCY.into()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpeningBalance {
    /// On the account's side: positive for money in an account, negative for a balance owed.
    pub amount: Money,
    pub date: NaiveDate,
    pub transaction_id: i64,
}

/// An account as the chart of accounts is exported and imported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedAccount {
    pub name: String,
    pub full_path: String,
    #[serde(rename = "type")]
    pub kind: AccountType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
    /// Left out when it's the default, GBP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeedAccount {
    name: String,
    full_path: String,
    #[serde(rename = "type")]
    kind: AccountType,
    parent_path: Option<String>,
    icon: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SeedRule {
    pattern: String,
    #[serde(default = "substring")]
    match_type: String,
    target_account: String,
    #[serde(default)]
    priority: i64,
}

fn substring() -> String {
    "substring".into()
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Account not found")]
    NotFound,
    #[error("Account with this path already exists")]
    PathTaken,
    #[error("Parent account not found")]
    ParentNotFound,
    #[error("An account can't be inside itself or one of its own subaccounts")]
    ParentIsDescendant,
    #[error("Cannot delete account with children")]
    HasChildren,
    #[error("{0}")]
    InUse(&'static str),
    #[error("Opening balance is only meaningful for asset or liability accounts")]
    NoOpeningBalance,
    #[error(
        "Required account '{OPENING_BALANCES_PATH}' is missing. Run the chart-of-accounts seed \
         and try again."
    )]
    MissingOpeningBalances,
    #[error(
        "Cannot import accounts when accounts already exist. Use this on a fresh database only."
    )]
    NotEmpty,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

const COLUMNS: &str = "id, name, full_path, parent_id, type, currency, bank_identifier, icon";

/// Every account, in path order.
pub async fn list(db: &SqlitePool) -> Result<Vec<Account>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM accounts ORDER BY full_path"
    ))
    .fetch_all(db)
    .await
}

/// The accounts of one type, in path order.
pub async fn list_of_type(db: &SqlitePool, kind: AccountType) -> Result<Vec<Account>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM accounts WHERE type = ? ORDER BY full_path"
    ))
    .bind(kind)
    .fetch_all(db)
    .await
}

pub async fn get(db: &mut SqliteConnection, id: i64) -> Result<Option<Account>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM accounts WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn find_by_path(
    db: &mut SqliteConnection,
    path: &str,
) -> Result<Option<Account>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM accounts WHERE full_path = ?"
    ))
    .bind(path)
    .fetch_optional(db)
    .await
}

async fn path_taken(db: &mut SqliteConnection, path: &str) -> Result<bool, sqlx::Error> {
    Ok(find_by_path(db, path).await?.is_some())
}

pub async fn create(db: &SqlitePool, new: NewAccount) -> Result<Account, AccountError> {
    let mut tx = db.begin().await?;
    if path_taken(&mut tx, &new.full_path).await? {
        return Err(AccountError::PathTaken);
    }
    if let Some(parent_id) = new.parent_id
        && get(&mut tx, parent_id).await?.is_none()
    {
        return Err(AccountError::ParentNotFound);
    }
    let account = insert(&mut tx, &new).await?;
    tx.commit().await?;
    Ok(account)
}

async fn insert(db: &mut SqliteConnection, new: &NewAccount) -> Result<Account, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO accounts (name, full_path, parent_id, type, currency, bank_identifier, icon)
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING {COLUMNS}"
    ))
    .bind(&new.name)
    .bind(&new.full_path)
    .bind(new.parent_id)
    .bind(new.kind)
    .bind(&new.currency)
    .bind(&new.bank_identifier)
    .bind(&new.icon)
    .fetch_one(db)
    .await
}

/// Applies `changes`. A new path carries the account's subaccounts along with it.
pub async fn update(
    db: &SqlitePool,
    id: i64,
    changes: AccountUpdate,
) -> Result<Account, AccountError> {
    let mut tx = db.begin().await?;
    let mut account = get(&mut tx, id).await?.ok_or(AccountError::NotFound)?;

    if let Some(full_path) = changes.full_path
        && full_path != account.full_path
    {
        if path_taken(&mut tx, &full_path).await? {
            return Err(AccountError::PathTaken);
        }
        move_subaccounts(&mut tx, &account.full_path, &full_path).await?;
        account.full_path = full_path;
    }
    if let Some(parent_id) = changes.parent_id {
        if let Some(parent_id) = parent_id {
            check_parent(&mut tx, id, parent_id).await?;
        }
        account.parent_id = parent_id;
    }
    if let Some(name) = changes.name {
        account.name = name;
    }
    if let Some(kind) = changes.kind {
        account.kind = kind;
    }
    if let Some(currency) = changes.currency {
        account.currency = currency;
    }
    if let Some(bank_identifier) = changes.bank_identifier {
        account.bank_identifier = bank_identifier;
    }
    if let Some(icon) = changes.icon {
        account.icon = icon;
    }

    sqlx::query(
        "UPDATE accounts SET name = ?, full_path = ?, parent_id = ?, type = ?, currency = ?,
         bank_identifier = ?, icon = ? WHERE id = ?",
    )
    .bind(&account.name)
    .bind(&account.full_path)
    .bind(account.parent_id)
    .bind(account.kind)
    .bind(&account.currency)
    .bind(&account.bank_identifier)
    .bind(&account.icon)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(account)
}

/// The parent must exist and mustn't be the account itself or one of its subaccounts.
async fn check_parent(
    db: &mut SqliteConnection,
    id: i64,
    parent_id: i64,
) -> Result<(), AccountError> {
    if get(db, parent_id).await?.is_none() {
        return Err(AccountError::ParentNotFound);
    }
    let mut next = Some(parent_id);
    while let Some(ancestor) = next {
        if ancestor == id {
            return Err(AccountError::ParentIsDescendant);
        }
        next = match get(db, ancestor).await? {
            Some(account) => account.parent_id,
            None => None,
        };
    }
    Ok(())
}

/// Renames the paths under `old_path` to sit under `new_path`. Returns how many moved.
pub async fn move_subaccounts(
    db: &mut SqliteConnection,
    old_path: &str,
    new_path: &str,
) -> Result<u64, sqlx::Error> {
    // Compared by prefix rather than LIKE, so `_` and `%` in names aren't wildcards.
    let prefix = format!("{old_path}:");
    let moved = sqlx::query(
        "UPDATE accounts SET full_path = ? || substr(full_path, ?)
         WHERE substr(full_path, 1, ?) = ?",
    )
    .bind(new_path)
    .bind(i64::try_from(old_path.chars().count()).unwrap_or(i64::MAX) + 1)
    .bind(i64::try_from(prefix.chars().count()).unwrap_or(i64::MAX))
    .bind(&prefix)
    .execute(db)
    .await?;
    Ok(moved.rows_affected())
}

/// Deletes an account that nothing depends on.
pub async fn delete(db: &SqlitePool, id: i64) -> Result<(), AccountError> {
    let mut tx = db.begin().await?;
    if get(&mut tx, id).await?.is_none() {
        return Err(AccountError::NotFound);
    }
    let uses = |sql: &'static str| sqlx::query_scalar::<_, bool>(sql).bind(id);
    if uses("SELECT EXISTS (SELECT 1 FROM accounts WHERE parent_id = ?)")
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(AccountError::HasChildren);
    }
    if uses("SELECT EXISTS (SELECT 1 FROM postings WHERE account_id = ?)")
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(AccountError::InUse(
            "Cannot delete an account that has transactions. Move them to another account first.",
        ));
    }
    if uses("SELECT EXISTS (SELECT 1 FROM rules WHERE target_account_id = ?)")
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(AccountError::InUse(
            "Cannot delete an account that rules put transactions in. Change or delete those \
             rules first.",
        ));
    }
    if uses("SELECT EXISTS (SELECT 1 FROM import_batches WHERE account_id = ?)")
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(AccountError::InUse(
            "Cannot delete an account that statements have been imported into.",
        ));
    }
    sqlx::query("DELETE FROM accounts WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// The opening-balance transaction: the earliest with one posting on this account and one on
/// Equity:OpeningBalances. Returns it with the account-side posting's id.
async fn find_opening_balance(
    db: &mut SqliteConnection,
    account_id: i64,
    equity_id: i64,
) -> Result<Option<(OpeningBalance, i64)>, sqlx::Error> {
    let row: Option<(i64, NaiveDate, i64, Money)> = sqlx::query_as(
        "SELECT t.id, t.date, bank.id, bank.amount
         FROM transactions t
         JOIN postings bank ON bank.transaction_id = t.id AND bank.account_id = ?
         JOIN postings equity ON equity.transaction_id = t.id AND equity.account_id = ?
         ORDER BY t.date, t.id
         LIMIT 1",
    )
    .bind(account_id)
    .bind(equity_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(transaction_id, date, posting_id, amount)| {
        (
            OpeningBalance {
                amount,
                date,
                transaction_id,
            },
            posting_id,
        )
    }))
}

async fn opening_balances_account(db: &mut SqliteConnection) -> Result<Account, AccountError> {
    find_by_path(db, OPENING_BALANCES_PATH)
        .await?
        .ok_or(AccountError::MissingOpeningBalances)
}

pub async fn opening_balance(
    db: &SqlitePool,
    account_id: i64,
) -> Result<Option<OpeningBalance>, AccountError> {
    let mut conn = db.acquire().await?;
    if get(&mut conn, account_id).await?.is_none() {
        return Err(AccountError::NotFound);
    }
    let equity = opening_balances_account(&mut conn).await?;
    Ok(find_opening_balance(&mut conn, account_id, equity.id)
        .await?
        .map(|(balance, _)| balance))
}

/// Creates or updates the account's opening balance: a confirmed transaction between the
/// account and Equity:OpeningBalances.
pub async fn set_opening_balance(
    db: &SqlitePool,
    account_id: i64,
    amount: Money,
    date: NaiveDate,
) -> Result<OpeningBalance, AccountError> {
    let mut tx = db.begin().await?;
    let account = get(&mut tx, account_id)
        .await?
        .ok_or(AccountError::NotFound)?;
    if !matches!(account.kind, AccountType::Asset | AccountType::Liability) {
        return Err(AccountError::NoOpeningBalance);
    }
    let equity = opening_balances_account(&mut tx).await?;

    let transaction_id = match find_opening_balance(&mut tx, account_id, equity.id).await? {
        Some((existing, posting_id)) => {
            sqlx::query("UPDATE transactions SET date = ? WHERE id = ?")
                .bind(date)
                .bind(existing.transaction_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE postings SET amount = ? WHERE id = ?")
                .bind(amount)
                .bind(posting_id)
                .execute(&mut *tx)
                .await?;
            // The equity side mirrors it, so the transaction still balances.
            sqlx::query(
                "UPDATE postings SET amount = ?
                 WHERE id = (SELECT id FROM postings
                             WHERE transaction_id = ? AND account_id = ? AND id != ? LIMIT 1)",
            )
            .bind(-amount)
            .bind(existing.transaction_id)
            .bind(equity.id)
            .bind(posting_id)
            .execute(&mut *tx)
            .await?;
            existing.transaction_id
        }
        None => {
            let postings = Balanced::new(vec![
                NewPosting {
                    account_id,
                    amount,
                    categorised_by: CategorisationSource::Manual,
                    rule_id: None,
                },
                NewPosting {
                    account_id: equity.id,
                    amount: -amount,
                    categorised_by: CategorisationSource::Manual,
                    rule_id: None,
                },
            ])
            .expect("an amount and its opposite balance");
            let new = NewTransaction {
                date,
                description: OPENING_BALANCE_DESCRIPTION.into(),
                status: Status::Confirmed,
                currency: account.currency.clone(),
                import_batch_id: None,
                import_metadata: None,
                postings,
            };
            ledger::insert(&mut tx, &new)
                .await
                .map_err(|err| match err {
                    LedgerError::Database(err) => AccountError::Database(err),
                    other => unreachable!("both accounts exist: {other}"),
                })?
        }
    };
    tx.commit().await?;
    Ok(OpeningBalance {
        amount,
        date,
        transaction_id,
    })
}

/// The chart of accounts, for importing into another install.
pub async fn export(db: &SqlitePool) -> Result<Vec<ExportedAccount>, sqlx::Error> {
    let accounts = list(db).await?;
    let paths: HashMap<i64, &str> = accounts
        .iter()
        .map(|a| (a.id, a.full_path.as_str()))
        .collect();
    Ok(accounts
        .iter()
        .map(|a| ExportedAccount {
            name: a.name.clone(),
            full_path: a.full_path.clone(),
            kind: a.kind,
            parent_path: a
                .parent_id
                .and_then(|id| paths.get(&id))
                .map(|p| (*p).to_owned()),
            currency: (a.currency != DEFAULT_CURRENCY).then(|| a.currency.clone()),
        })
        .collect())
}

/// Imports a chart of accounts into an install that has none yet. Returns how many were made.
pub async fn import(
    db: &SqlitePool,
    accounts: Vec<ExportedAccount>,
) -> Result<usize, AccountError> {
    let mut tx = db.begin().await?;
    if has_accounts(&mut tx).await? {
        return Err(AccountError::NotEmpty);
    }
    let mut ids: HashMap<String, i64> = HashMap::new();
    for exported in &accounts {
        if ids.contains_key(&exported.full_path) {
            return Err(AccountError::PathTaken);
        }
        let new = NewAccount {
            name: exported.name.clone(),
            full_path: exported.full_path.clone(),
            parent_id: exported
                .parent_path
                .as_ref()
                .and_then(|p| ids.get(p).copied()),
            kind: exported.kind,
            currency: exported.currency.clone().unwrap_or_else(default_currency),
            bank_identifier: None,
            icon: None,
        };
        let account = insert(&mut tx, &new).await?;
        ids.insert(account.full_path, account.id);
    }
    tx.commit().await?;
    Ok(accounts.len())
}

/// Looking an account up by what a person or model calls it.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Ambiguous(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// The path segment for a display name, as the Accounts page builds paths: words joined,
/// anything not a letter or digit dropped ("Amazon Prime" -> "AmazonPrime").
pub fn path_segment(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// A string in single quotes (or double, if it holds a single quote), for messages that quote
/// what was asked for.
pub(crate) fn quoted(text: &str) -> String {
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\'', "\\'"))
    }
}

/// An account of one of `kinds` (any kind if empty) by its full path
/// ("Expenses:Food:Groceries") or, when only one has it, its name or last path segment
/// ("Groceries"), ignoring case. `what` names the thing in messages, e.g. "category".
pub async fn find(
    db: &SqlitePool,
    reference: &str,
    kinds: &[AccountType],
    what: &str,
) -> Result<Account, FindError> {
    let reference = reference.trim();
    let accounts: Vec<Account> = list(db)
        .await?
        .into_iter()
        .filter(|a| kinds.is_empty() || kinds.contains(&a.kind))
        .collect();
    let wanted = reference.to_lowercase();
    if let Some(exact) = accounts
        .iter()
        .find(|a| a.full_path.to_lowercase() == wanted)
    {
        return Ok(exact.clone());
    }
    let squashed = path_segment(reference).to_lowercase();
    let mut matches: Vec<&Account> = accounts
        .iter()
        .filter(|a| {
            let last = a
                .full_path
                .rsplit(':')
                .next()
                .unwrap_or_default()
                .to_lowercase();
            a.name.to_lowercase() == wanted || last == wanted || last == squashed
        })
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(FindError::NotFound(format!(
            "No {what} called {}. Use list_categories or list_accounts to see them.",
            quoted(reference)
        ))),
        _ => {
            matches.sort_by(|a, b| a.full_path.cmp(&b.full_path));
            let options: Vec<&str> = matches.iter().map(|a| a.full_path.as_str()).collect();
            Err(FindError::Ambiguous(format!(
                "{} matches several {what}s: {}. Use the full path.",
                quoted(reference),
                options.join(", ")
            )))
        }
    }
}

/// A category (an income or expense account), as `find` looks it up.
pub async fn find_category(db: &SqlitePool, reference: &str) -> Result<Account, FindError> {
    find(
        db,
        reference,
        &[AccountType::Expense, AccountType::Income],
        "category",
    )
    .await
}

pub async fn has_accounts(db: &mut SqliteConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts)")
        .fetch_one(db)
        .await
}

pub async fn has_rules(db: &mut SqliteConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rules)")
        .fetch_one(db)
        .await
}

/// Adds the UK chart of accounts, skipping accounts that exist already but filling in their
/// icons if they have none. Returns how many seed accounts there are (new or not).
pub async fn load_seed_accounts(db: &SqlitePool) -> Result<usize, AccountError> {
    let seeds: Vec<SeedAccount> =
        serde_json::from_str(SEED_ACCOUNTS).expect("the seed accounts are valid JSON");
    let mut tx = db.begin().await?;
    let mut ids: HashMap<String, i64> = HashMap::new();
    for seed in seeds {
        if let Some(existing) = find_by_path(&mut tx, &seed.full_path).await? {
            if existing.icon.is_none() && seed.icon.is_some() {
                sqlx::query("UPDATE accounts SET icon = ? WHERE id = ?")
                    .bind(&seed.icon)
                    .bind(existing.id)
                    .execute(&mut *tx)
                    .await?;
            }
            ids.insert(existing.full_path, existing.id);
            continue;
        }
        let account = insert(
            &mut tx,
            &NewAccount {
                name: seed.name,
                full_path: seed.full_path,
                parent_id: seed.parent_path.and_then(|p| ids.get(&p).copied()),
                kind: seed.kind,
                currency: default_currency(),
                bank_identifier: None,
                icon: seed.icon,
            },
        )
        .await?;
        ids.insert(account.full_path, account.id);
    }
    tx.commit().await?;
    Ok(ids.len())
}

/// Adds the UK categorisation rules whose accounts exist, skipping ones already there. Returns
/// how many were added.
pub async fn load_seed_rules(db: &SqlitePool) -> Result<usize, AccountError> {
    let seeds: Vec<SeedRule> =
        serde_json::from_str(SEED_RULES).expect("the seed rules are valid JSON");
    let mut tx = db.begin().await?;
    let mut created = 0;
    for seed in seeds {
        let Some(match_type) = MatchType::from_name(&seed.match_type) else {
            continue;
        };
        let Some(target) = find_by_path(&mut tx, &seed.target_account).await? else {
            continue;
        };
        if seed.pattern.is_empty() {
            continue;
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM rules
                            WHERE pattern = ? AND match_type_id = ? AND target_account_id = ?)",
        )
        .bind(&seed.pattern)
        .bind(match_type as i64)
        .bind(target.id)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            continue;
        }
        sqlx::query(
            "INSERT INTO rules (pattern, match_type_id, target_account_id, priority)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&seed.pattern)
        .bind(match_type as i64)
        .bind(target.id)
        .bind(seed.priority)
        .execute(&mut *tx)
        .await?;
        created += 1;
    }
    tx.commit().await?;
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn new(name: &str, full_path: &str, kind: AccountType, parent_id: Option<i64>) -> NewAccount {
        NewAccount {
            name: name.into(),
            full_path: full_path.into(),
            parent_id,
            kind,
            currency: default_currency(),
            bank_identifier: None,
            icon: None,
        }
    }

    async fn seeded() -> SqlitePool {
        let pool = db::open_in_memory().await.unwrap();
        load_seed_accounts(&pool).await.unwrap();
        pool
    }

    async fn by_path(pool: &SqlitePool, path: &str) -> Account {
        find_by_path(&mut pool.acquire().await.unwrap(), path)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("no account {path}"))
    }

    #[tokio::test]
    async fn seeds_the_uk_chart_once() {
        let pool = db::open_in_memory().await.unwrap();
        assert_eq!(load_seed_accounts(&pool).await.unwrap(), 39);
        assert_eq!(load_seed_rules(&pool).await.unwrap(), 20);
        assert_eq!(list(&pool).await.unwrap().len(), 39);
        let current = by_path(&pool, "Assets:Bank:Current").await;
        assert_eq!(current.icon.as_deref(), Some("Building2"));
        assert_eq!(
            current.parent_id,
            Some(by_path(&pool, "Assets:Bank").await.id)
        );

        // Again: nothing new, but missing icons come back.
        sqlx::query("UPDATE accounts SET icon = NULL WHERE full_path = 'Assets:Bank:Current'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(load_seed_accounts(&pool).await.unwrap(), 39);
        assert_eq!(load_seed_rules(&pool).await.unwrap(), 0);
        assert_eq!(list(&pool).await.unwrap().len(), 39);
        assert_eq!(
            by_path(&pool, "Assets:Bank:Current").await.icon.as_deref(),
            Some("Building2")
        );
    }

    #[tokio::test]
    async fn lists_in_path_order_and_by_type() {
        let pool = seeded().await;
        let all = list(&pool).await.unwrap();
        let mut paths: Vec<&str> = all.iter().map(|a| a.full_path.as_str()).collect();
        let listed = paths.clone();
        paths.sort_unstable();
        assert_eq!(listed, paths);
        let liabilities = list_of_type(&pool, AccountType::Liability).await.unwrap();
        assert_eq!(liabilities.len(), 3);
    }

    #[tokio::test]
    async fn refuses_duplicate_paths_and_missing_parents() {
        let pool = seeded().await;
        let duplicate = create(
            &pool,
            new("Food", "Expenses:Food", AccountType::Expense, None),
        )
        .await;
        assert!(matches!(duplicate, Err(AccountError::PathTaken)));
        let orphan = create(&pool, new("X", "X", AccountType::Asset, Some(999))).await;
        assert!(matches!(orphan, Err(AccountError::ParentNotFound)));
    }

    #[tokio::test]
    async fn moving_an_account_moves_its_subaccounts() {
        let pool = seeded().await;
        let bank = by_path(&pool, "Assets:Bank").await;
        let moved = update(
            &pool,
            bank.id,
            AccountUpdate {
                full_path: Some("Assets:Banks".into()),
                name: Some("Banks".into()),
                ..AccountUpdate::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(moved.full_path, "Assets:Banks");
        assert_eq!(moved.name, "Banks");
        for path in [
            "Assets:Banks:Current",
            "Assets:Banks:Savings",
            "Assets:Banks:ISA",
        ] {
            by_path(&pool, path).await;
        }
        // Accounts that merely start with the same letters stay put.
        create(
            &pool,
            new("Bankruptcy", "Assets:Bankruptcy", AccountType::Asset, None),
        )
        .await
        .unwrap();
        let banks = by_path(&pool, "Assets:Banks").await;
        update(
            &pool,
            banks.id,
            AccountUpdate {
                full_path: Some("Assets:Bank".into()),
                ..AccountUpdate::default()
            },
        )
        .await
        .unwrap();
        by_path(&pool, "Assets:Bankruptcy").await;
        by_path(&pool, "Assets:Bank:Current").await;
    }

    #[tokio::test]
    async fn updates_only_what_is_given_and_clears_with_null() {
        let pool = seeded().await;
        let current = by_path(&pool, "Assets:Bank:Current").await;
        let changes: AccountUpdate = serde_json::from_value(
            serde_json::json!({"icon": null, "bank_identifier": "12-34-56"}),
        )
        .unwrap();
        let updated = update(&pool, current.id, changes).await.unwrap();
        assert_eq!(updated.icon, None);
        assert_eq!(updated.bank_identifier.as_deref(), Some("12-34-56"));
        assert_eq!(updated.name, current.name);
        assert_eq!(updated.parent_id, current.parent_id);
    }

    #[tokio::test]
    async fn refuses_a_path_taken_or_a_parent_inside_itself() {
        let pool = seeded().await;
        let bank = by_path(&pool, "Assets:Bank").await;
        let current = by_path(&pool, "Assets:Bank:Current").await;
        let taken = update(
            &pool,
            bank.id,
            AccountUpdate {
                full_path: Some("Assets:Cash".into()),
                ..AccountUpdate::default()
            },
        )
        .await;
        assert!(matches!(taken, Err(AccountError::PathTaken)));
        let cycle = update(
            &pool,
            bank.id,
            AccountUpdate {
                parent_id: Some(Some(current.id)),
                ..AccountUpdate::default()
            },
        )
        .await;
        assert!(matches!(cycle, Err(AccountError::ParentIsDescendant)));
    }

    #[tokio::test]
    async fn deletes_only_accounts_nothing_depends_on() {
        let pool = seeded().await;
        let food = by_path(&pool, "Expenses:Food").await;
        assert!(matches!(
            delete(&pool, food.id).await,
            Err(AccountError::HasChildren)
        ));

        let groceries = by_path(&pool, "Expenses:Food:Groceries").await;
        load_seed_rules(&pool).await.unwrap();
        assert!(matches!(
            delete(&pool, groceries.id).await,
            Err(AccountError::InUse(_))
        ));

        let current = by_path(&pool, "Assets:Bank:Current").await;
        set_opening_balance(&pool, current.id, "10".parse().unwrap(), NaiveDate::MIN)
            .await
            .unwrap();
        assert!(matches!(
            delete(&pool, current.id).await,
            Err(AccountError::InUse(_))
        ));

        let isa = by_path(&pool, "Assets:Bank:ISA").await;
        delete(&pool, isa.id).await.unwrap();
        assert!(matches!(
            delete(&pool, isa.id).await,
            Err(AccountError::NotFound)
        ));
    }

    #[tokio::test]
    async fn opening_balances_are_created_then_updated_and_balance() {
        let pool = seeded().await;
        let card = by_path(&pool, "Liabilities:CreditCard").await;
        assert_eq!(opening_balance(&pool, card.id).await.unwrap(), None);

        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let first = set_opening_balance(&pool, card.id, "-250.5".parse().unwrap(), date)
            .await
            .unwrap();
        let later = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
        let second = set_opening_balance(&pool, card.id, "-300".parse().unwrap(), later)
            .await
            .unwrap();
        assert_eq!(first.transaction_id, second.transaction_id);
        assert_eq!(opening_balance(&pool, card.id).await.unwrap(), Some(second));

        let (count, total): (i64, Money) =
            sqlx::query_as("SELECT COUNT(*), SUM(amount) FROM postings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
        assert!(total.is_zero());

        let food = by_path(&pool, "Expenses:Food").await;
        let refused = set_opening_balance(&pool, food.id, "1".parse().unwrap(), date).await;
        assert!(matches!(refused, Err(AccountError::NoOpeningBalance)));
    }

    #[tokio::test]
    async fn opening_balances_need_the_equity_account() {
        let pool = db::open_in_memory().await.unwrap();
        let account = create(&pool, new("Bank", "Bank", AccountType::Asset, None))
            .await
            .unwrap();
        let result = opening_balance(&pool, account.id).await;
        assert!(matches!(result, Err(AccountError::MissingOpeningBalances)));
    }

    #[tokio::test]
    async fn exports_and_imports_into_a_fresh_install() {
        let pool = seeded().await;
        let card = by_path(&pool, "Liabilities:CreditCard").await;
        update(
            &pool,
            card.id,
            AccountUpdate {
                currency: Some("EUR".into()),
                ..AccountUpdate::default()
            },
        )
        .await
        .unwrap();
        let exported = export(&pool).await.unwrap();
        let card_export = exported
            .iter()
            .find(|a| a.full_path == card.full_path)
            .unwrap();
        assert_eq!(card_export.currency.as_deref(), Some("EUR"));
        assert_eq!(card_export.parent_path.as_deref(), Some("Liabilities"));
        assert!(matches!(
            import(&pool, exported.clone()).await,
            Err(AccountError::NotEmpty)
        ));

        let fresh = db::open_in_memory().await.unwrap();
        assert_eq!(import(&fresh, exported.clone()).await.unwrap(), 39);
        assert_eq!(export(&fresh).await.unwrap(), exported);
    }
}
