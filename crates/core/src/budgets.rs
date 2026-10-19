//! Spending limits for a category, covering it and its subcategories, per calendar month or
//! year.

use chrono::{Datelike, Days, Months, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::Money;
use crate::accounts::{self, Account, AccountType};

/// How long a budget runs. The ids are `budget_periods`' rows.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Period {
    #[default]
    Monthly = 1,
    Annual = 2,
}

impl Period {
    pub fn id(self) -> i64 {
        self as i64
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Monthly => "monthly",
            Self::Annual => "annual",
        }
    }

    fn from_id(id: i64) -> Self {
        if id == 2 { Self::Annual } else { Self::Monthly }
    }

    /// The calendar month or year containing `on`, as `[start, end)`.
    pub fn bounds(self, on: NaiveDate) -> (NaiveDate, NaiveDate) {
        let start = match self {
            Self::Monthly => on.with_day(1),
            Self::Annual => NaiveDate::from_ymd_opt(on.year(), 1, 1),
        }
        .expect("a valid period start");
        let length = match self {
            Self::Monthly => Months::new(1),
            Self::Annual => Months::new(12),
        };
        (start, start + length)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    #[error("Category not found")]
    CategoryNotFound,
    #[error("Budgets can only be set on spending categories")]
    NotSpending,
    #[error("That category already has a budget for this period")]
    Taken,
    #[error("Budget not found")]
    NotFound,
    #[error("The budget must be more than 0")]
    NotPositive,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A budget with how much has gone against it in one period.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Budget {
    pub id: i64,
    pub account_id: i64,
    pub account_full_path: String,
    pub account_icon: Option<String>,
    pub currency: String,
    pub period: Period,
    pub period_start: NaiveDate,
    /// The period's last day.
    pub period_end: NaiveDate,
    pub amount: Money,
    /// Refunds reduce it.
    pub spent: Money,
    /// The part of `spent` in transactions not yet reviewed.
    pub unreviewed: Money,
    /// Negative when overspent.
    pub remaining: Money,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    account_id: i64,
    full_path: String,
    icon: Option<String>,
    currency: String,
    period_id: i64,
    amount: Money,
}

const SELECT: &str =
    "SELECT b.id, b.account_id, a.full_path, a.icon, a.currency, b.period_id, b.amount
     FROM budgets b JOIN accounts a ON a.id = b.account_id";

/// Spending in `[start, end)` in a category and its subcategories (matched on the current
/// path, so renames are followed), and how much of it is unreviewed.
pub async fn category_spend(
    db: &SqlitePool,
    path: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<(Money, Money), sqlx::Error> {
    sqlx::query_as(
        "SELECT COALESCE(SUM(p.amount), 0),
                COALESCE(SUM(CASE WHEN t.status = 'pending' THEN p.amount END), 0)
         FROM postings p
         JOIN accounts a ON a.id = p.account_id
         JOIN transactions t ON t.id = p.transaction_id
         WHERE (a.full_path = ?1 OR substr(a.full_path, 1, length(?1) + 1) = ?1 || ':')
           AND t.date >= ?2 AND t.date < ?3",
    )
    .bind(path)
    .bind(start)
    .bind(end)
    .fetch_one(db)
    .await
}

async fn with_spending(db: &SqlitePool, row: Row, on: NaiveDate) -> Result<Budget, sqlx::Error> {
    let period = Period::from_id(row.period_id);
    let (start, end) = period.bounds(on);
    let (spent, unreviewed) = category_spend(db, &row.full_path, start, end).await?;
    Ok(Budget {
        id: row.id,
        account_id: row.account_id,
        account_full_path: row.full_path,
        account_icon: row.icon,
        currency: row.currency,
        period,
        period_start: start,
        period_end: end - Days::new(1),
        amount: row.amount,
        spent,
        unreviewed,
        remaining: row.amount - spent,
    })
}

/// Every budget, for the period containing `on`: monthly ones first, then by category.
pub async fn list(db: &SqlitePool, on: NaiveDate) -> Result<Vec<Budget>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(&format!("{SELECT} ORDER BY b.period_id, a.full_path"))
        .fetch_all(db)
        .await?;
    let mut budgets = Vec::with_capacity(rows.len());
    for row in rows {
        budgets.push(with_spending(db, row, on).await?);
    }
    Ok(budgets)
}

pub async fn get(db: &SqlitePool, id: i64, on: NaiveDate) -> Result<Budget, BudgetError> {
    let row: Row = sqlx::query_as(&format!("{SELECT} WHERE b.id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(BudgetError::NotFound)?;
    Ok(with_spending(db, row, on).await?)
}

async fn spending_category(db: &SqlitePool, account_id: i64) -> Result<Account, BudgetError> {
    let mut conn = db.acquire().await?;
    let account = accounts::get(&mut conn, account_id)
        .await?
        .ok_or(BudgetError::CategoryNotFound)?;
    if account.kind != AccountType::Expense {
        return Err(BudgetError::NotSpending);
    }
    Ok(account)
}

async fn taken(
    db: &SqlitePool,
    account_id: i64,
    period: Period,
    except: Option<i64>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM budgets
         WHERE account_id = ? AND period_id = ? AND id IS NOT ?)",
    )
    .bind(account_id)
    .bind(period.id())
    .bind(except)
    .fetch_one(db)
    .await
}

fn check_amount(amount: Money) -> Result<(), BudgetError> {
    if amount.is_positive() {
        Ok(())
    } else {
        Err(BudgetError::NotPositive)
    }
}

/// A new budget, reported for the period containing `today`.
pub async fn create(
    db: &SqlitePool,
    account_id: i64,
    amount: Money,
    period: Period,
    today: NaiveDate,
) -> Result<Budget, BudgetError> {
    check_amount(amount)?;
    let account = spending_category(db, account_id).await?;
    if taken(db, account.id, period, None).await? {
        return Err(BudgetError::Taken);
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO budgets (account_id, period_id, amount) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(account.id)
    .bind(period.id())
    .bind(amount)
    .fetch_one(db)
    .await?;
    get(db, id, today).await
}

/// Changes a budget's limit or period, reported for the period containing `today`.
pub async fn update(
    db: &SqlitePool,
    id: i64,
    amount: Option<Money>,
    period: Option<Period>,
    today: NaiveDate,
) -> Result<Budget, BudgetError> {
    if let Some(amount) = amount {
        check_amount(amount)?;
    }
    let budget = get(db, id, today).await?;
    if let Some(period) = period
        && taken(db, budget.account_id, period, Some(id)).await?
    {
        return Err(BudgetError::Taken);
    }
    sqlx::query("UPDATE budgets SET amount = ?, period_id = ? WHERE id = ?")
        .bind(amount.unwrap_or(budget.amount))
        .bind(period.unwrap_or(budget.period).id())
        .bind(id)
        .execute(db)
        .await?;
    get(db, id, today).await
}

pub async fn delete(db: &SqlitePool, id: i64) -> Result<(), BudgetError> {
    let result = sqlx::query("DELETE FROM budgets WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(BudgetError::NotFound);
    }
    Ok(())
}

/// The limit a category has for a period, if it has a budget.
pub async fn limit(
    db: &SqlitePool,
    account_id: i64,
    period: Period,
) -> Result<Option<Money>, sqlx::Error> {
    sqlx::query_scalar("SELECT amount FROM budgets WHERE account_id = ? AND period_id = ?")
        .bind(account_id)
        .bind(period.id())
        .fetch_optional(db)
        .await
}

/// Sets a category's limit for a period, creating the budget if there isn't one. Returns the
/// previous limit. For the chat, which names categories rather than budgets.
pub async fn set(
    db: &SqlitePool,
    account_id: i64,
    period: Period,
    amount: Money,
) -> Result<Option<Money>, BudgetError> {
    check_amount(amount)?;
    spending_category(db, account_id).await?;
    let previous = limit(db, account_id, period).await?;
    sqlx::query(
        "INSERT INTO budgets (account_id, period_id, amount) VALUES (?, ?, ?)
         ON CONFLICT (account_id, period_id) DO UPDATE SET amount = excluded.amount",
    )
    .bind(account_id)
    .bind(period.id())
    .bind(amount)
    .execute(db)
    .await?;
    Ok(previous)
}

/// Removes a category's budget for a period, returning its limit, or `None` if it had none.
pub async fn remove(
    db: &SqlitePool,
    account_id: i64,
    period: Period,
) -> Result<Option<Money>, sqlx::Error> {
    sqlx::query_scalar(
        "DELETE FROM budgets WHERE account_id = ? AND period_id = ? RETURNING amount",
    )
    .bind(account_id)
    .bind(period.id())
    .fetch_optional(db)
    .await
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonthSpend {
    /// e.g. "2026-08"
    pub month: String,
    pub spent: Money,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub suggested: Money,
    pub months: Vec<MonthSpend>,
}

/// What a category has cost recently, as a starting point for a budget: the average of the
/// last three full months for a monthly budget (to the penny, halves to even), or the last
/// twelve full months combined for an annual one.
pub async fn suggest(
    db: &SqlitePool,
    account_id: i64,
    period: Period,
    on: NaiveDate,
) -> Result<Suggestion, BudgetError> {
    let account = spending_category(db, account_id).await?;
    let count = match period {
        Period::Monthly => 3,
        Period::Annual => 12,
    };
    let this_month = Period::Monthly.bounds(on).0;
    let mut months = Vec::with_capacity(count);
    for back in (1..=count as u32).rev() {
        let start = this_month - Months::new(back);
        let (spent, _) =
            category_spend(db, &account.full_path, start, start + Months::new(1)).await?;
        months.push(MonthSpend {
            month: start.format("%Y-%m").to_string(),
            spent,
        });
    }
    let total: Money = months.iter().map(|m| m.spent).sum();
    let suggested = match period {
        Period::Monthly => total.to_decimal() / rust_decimal::Decimal::from(3),
        Period::Annual => total.to_decimal(),
    };
    let suggested = Money::from_decimal(suggested.round_dp(2)).expect("a sum of amounts fits");
    Ok(Suggestion { suggested, months })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn periods_are_calendar_months_and_years() {
        for (period, on, start, end) in [
            (Period::Monthly, "2026-09-15", "2026-09-01", "2026-10-01"),
            (Period::Monthly, "2026-12-31", "2026-12-01", "2027-01-01"),
            (Period::Monthly, "2028-02-29", "2028-02-01", "2028-03-01"),
            (Period::Annual, "2026-06-01", "2026-01-01", "2027-01-01"),
        ] {
            assert_eq!(
                period.bounds(day(on)),
                (day(start), day(end)),
                "{period:?} {on}"
            );
        }
    }
}
