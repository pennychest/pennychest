//! What the actions share: how transactions are described and how accounts are looked up.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::Serialize;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::Money;
use crate::accounts::{self, Account, AccountType, FindError};

pub use crate::imports::batches::UNCATEGORISED_PATH;

pub const CATEGORY_TYPES: [AccountType; 2] = [AccountType::Expense, AccountType::Income];
pub const MONEY_ACCOUNT_TYPES: [AccountType; 2] = [AccountType::Asset, AccountType::Liability];

/// A bank account, card or other asset or liability account, by path or name.
pub async fn find_money_account(db: &SqlitePool, reference: &str) -> Result<Account, FindError> {
    accounts::find(db, reference, &MONEY_ACCOUNT_TYPES, "account").await
}

pub fn is_category(kind: AccountType) -> bool {
    CATEGORY_TYPES.contains(&kind)
}

pub fn is_money_account(kind: AccountType) -> bool {
    MONEY_ACCOUNT_TYPES.contains(&kind)
}

/// Spending is recorded as a positive amount against an expense category and income as a
/// negative amount against an income category; the bank or card side is the opposite.
pub fn category_amount(category: AccountType, amount: Money) -> Money {
    if category == AccountType::Expense {
        amount
    } else {
        -amount
    }
}

/// One side of a transaction, with what its account is.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Leg {
    pub id: i64,
    pub transaction_id: i64,
    pub account_id: i64,
    pub full_path: String,
    #[sqlx(rename = "type")]
    pub kind: AccountType,
    pub amount: Money,
    pub categorised_by_id: i64,
    pub rule_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Header {
    pub id: i64,
    pub date: NaiveDate,
    pub description: String,
    pub status: String,
    pub import_batch_id: Option<i64>,
}

/// A transaction with its legs, in the order they were written.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub header: Header,
    pub legs: Vec<Leg>,
}

impl Loaded {
    pub fn categories(&self) -> Vec<&Leg> {
        self.legs.iter().filter(|l| is_category(l.kind)).collect()
    }

    pub fn money_side(&self) -> Vec<&Leg> {
        self.legs
            .iter()
            .filter(|l| is_money_account(l.kind))
            .collect()
    }

    /// The transaction as actions report it: the amount categorised (positive for spending,
    /// negative for income or refunds) and the bank account or card on the other side.
    pub fn describe(&self) -> Described {
        let join = |legs: Vec<&Leg>| {
            let paths: Vec<&str> = legs.iter().map(|l| l.full_path.as_str()).collect();
            (!paths.is_empty()).then(|| paths.join(", "))
        };
        let categories = self.categories();
        let amount: Money = categories.iter().map(|l| l.amount).sum();
        Described {
            id: self.header.id,
            date: self.header.date,
            description: self.header.description.clone(),
            category: join(categories),
            amount: amount.two_places(),
            account: join(self.money_side()),
            status: self.header.status.clone(),
            imported: self.header.import_batch_id.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Described {
    pub id: i64,
    pub date: NaiveDate,
    pub description: String,
    pub category: Option<String>,
    pub amount: String,
    pub account: Option<String>,
    pub status: String,
    pub imported: bool,
}

fn push_ids(query: &mut QueryBuilder<'_, Sqlite>, ids: &[i64]) {
    let mut list = query.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    query.push(")");
}

/// The transactions with these ids that exist, by id.
pub async fn load(db: &SqlitePool, ids: &[i64]) -> Result<HashMap<i64, Loaded>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut query = QueryBuilder::new(
        "SELECT id, date, description, status, import_batch_id FROM transactions WHERE id IN (",
    );
    push_ids(&mut query, ids);
    let headers: Vec<Header> = query.build_query_as().fetch_all(db).await?;

    let mut query = QueryBuilder::new(
        "SELECT p.id, p.transaction_id, p.account_id, a.full_path, a.type, p.amount,
                p.categorised_by_id, p.rule_id
         FROM postings p JOIN accounts a ON a.id = p.account_id
         WHERE p.transaction_id IN (",
    );
    push_ids(&mut query, ids);
    query.push(" ORDER BY p.id");
    let legs: Vec<Leg> = query.build_query_as().fetch_all(db).await?;

    let mut loaded: HashMap<i64, Loaded> = headers
        .into_iter()
        .map(|header| {
            let id = header.id;
            (
                id,
                Loaded {
                    header,
                    legs: Vec::new(),
                },
            )
        })
        .collect();
    for leg in legs {
        if let Some(txn) = loaded.get_mut(&leg.transaction_id) {
            txn.legs.push(leg);
        }
    }
    Ok(loaded)
}

pub async fn load_one(db: &SqlitePool, id: i64) -> Result<Option<Loaded>, sqlx::Error> {
    Ok(load(db, &[id]).await?.remove(&id))
}

/// The ids in order, each once.
pub fn unique(ids: &[i64]) -> Vec<i64> {
    let mut seen = std::collections::HashSet::new();
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

/// "1 transaction", "2 transactions".
pub fn count_of(count: usize, what: &str) -> String {
    if count == 1 {
        format!("1 {what}")
    } else {
        format!("{count} {what}s")
    }
}
