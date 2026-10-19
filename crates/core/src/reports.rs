//! Totals over the ledger: the report endpoints, spending and income grouped by category,
//! month or merchant, and month-by-month cash flow and net worth. The dashboard's charts and
//! the chat both use these, so they always agree.
//!
//! Amounts are stored as exact integers, so the sums happen in SQL.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, Days, NaiveDate};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::Money;
use crate::accounts::{self, FindError};

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    UnknownCategory(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<FindError> for ReportError {
    fn from(error: FindError) -> Self {
        match error {
            FindError::NotFound(message) | FindError::Ambiguous(message) => {
                Self::UnknownCategory(message)
            }
            FindError::Database(error) => Self::Database(error),
        }
    }
}

/// The first of `today`'s month, for reports that default to this month so far.
pub fn month_start(day: NaiveDate) -> NaiveDate {
    day.with_day(1).expect("every month has a first day")
}

fn month_key(day: NaiveDate) -> String {
    day.format("%Y-%m").to_string()
}

/// The first day of each of the `months` calendar months ending with `end`'s, oldest first.
fn month_starts(end: NaiveDate, months: u32) -> Vec<NaiveDate> {
    let last = end.year() * 12 + end.month0() as i32;
    (last - months as i32 + 1..=last)
        .map(|i| {
            NaiveDate::from_ymd_opt(i.div_euclid(12), i.rem_euclid(12) as u32 + 1, 1)
                .expect("a valid month")
        })
        .collect()
}

fn month_end(start: NaiveDate) -> NaiveDate {
    start
        .checked_add_months(chrono::Months::new(1))
        .and_then(|next| next.checked_sub_days(Days::new(1)))
        .expect("a valid month end")
}

// The report endpoints.

/// Net worth, and income and spending over a period, from confirmed transactions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// Everything in asset accounts minus what's owed on liabilities, per currency.
    pub net_worth: BTreeMap<String, Money>,
    pub income: Money,
    pub expenses: Money,
    /// The share of income not spent, as a percentage to one decimal place; 0 with no income.
    pub savings_rate: rust_decimal::Decimal,
    pub date_from: NaiveDate,
    pub date_to: NaiveDate,
}

pub async fn summary(
    db: &SqlitePool,
    date_from: NaiveDate,
    date_to: NaiveDate,
) -> Result<Summary, sqlx::Error> {
    let net_worth: Vec<(String, Money)> = sqlx::query_as(
        "SELECT a.currency, SUM(p.amount)
         FROM postings p
         JOIN accounts a ON a.id = p.account_id
         JOIN transactions t ON t.id = p.transaction_id
         WHERE t.status = 'confirmed' AND a.type IN ('asset', 'liability')
         GROUP BY a.currency",
    )
    .fetch_all(db)
    .await?;
    let (income, expenses): (Money, Money) = sqlx::query_as(
        "SELECT
            COALESCE(SUM(CASE WHEN a.type = 'income' THEN abs(p.amount) END), 0),
            COALESCE(SUM(CASE WHEN a.type = 'expense' THEN p.amount END), 0)
         FROM postings p
         JOIN accounts a ON a.id = p.account_id
         JOIN transactions t ON t.id = p.transaction_id
         WHERE t.status = 'confirmed' AND t.date >= ? AND t.date <= ?",
    )
    .bind(date_from)
    .bind(date_to)
    .fetch_one(db)
    .await?;
    let savings_rate = if income.is_positive() {
        ((income - expenses).to_decimal() / income.to_decimal()
            * rust_decimal::Decimal::ONE_HUNDRED)
            .round_dp(1)
    } else {
        rust_decimal::Decimal::ZERO
    };
    Ok(Summary {
        net_worth: net_worth.into_iter().collect(),
        income,
        expenses,
        savings_rate,
        date_from,
        date_to,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct CategoryTotal {
    pub full_path: String,
    pub name: String,
    pub total: Money,
}

/// Confirmed spending in each account beneath `parent_path` over a period, largest first.
pub async fn expenses_by_category(
    db: &SqlitePool,
    date_from: NaiveDate,
    date_to: NaiveDate,
    parent_path: &str,
) -> Result<Vec<CategoryTotal>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.full_path, a.name, SUM(p.amount) AS total
         FROM postings p
         JOIN accounts a ON a.id = p.account_id
         JOIN transactions t ON t.id = p.transaction_id
         WHERE t.status = 'confirmed'
           AND substr(a.full_path, 1, length(?1) + 1) = ?1 || ':'
           AND t.date >= ?2 AND t.date <= ?3
         GROUP BY a.full_path, a.name
         ORDER BY total DESC, a.full_path",
    )
    .bind(parent_path)
    .bind(date_from)
    .bind(date_to)
    .fetch_all(db)
    .await
}

// Grouped totals, for the chat's spending summary and the cash flow chart.

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Expenses,
    Income,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    #[default]
    Category,
    Month,
    Merchant,
}

/// What to total: spending or income from `start` to `end` inclusive.
#[derive(Debug, Clone, PartialEq)]
pub struct Grouping {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub kind: Kind,
    pub group_by: GroupBy,
    /// Only this category and its subcategories (a path or name).
    pub category: Option<String>,
    /// When grouping by category, roll subcategories up to this many path levels.
    pub category_depth: Option<usize>,
}

impl Grouping {
    pub fn new(start: NaiveDate, end: NaiveDate) -> Self {
        Self {
            start,
            end,
            kind: Kind::Expenses,
            group_by: GroupBy::Category,
            category: None,
            category_depth: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    pub total: Money,
    /// The transactions in the group.
    pub transactions: HashSet<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub groups: HashMap<String, Group>,
    pub total: Money,
    /// The part of the total from transactions not yet reviewed.
    pub unreviewed: Money,
}

/// Spending (positive, with refunds reducing it) or income (positive) in each group. Pending
/// transactions count too; `unreviewed` says how much of the total they make up.
pub async fn group_totals(db: &SqlitePool, grouping: &Grouping) -> Result<Totals, ReportError> {
    let (account_type, sign) = match grouping.kind {
        Kind::Expenses => ("expense", 1),
        Kind::Income => ("income", -1),
    };
    let key = match grouping.group_by {
        GroupBy::Category => "a.full_path",
        GroupBy::Month => "substr(t.date, 1, 7)",
        GroupBy::Merchant => "t.description",
    };
    let within = match &grouping.category {
        Some(category) => Some(accounts::find_category(db, category).await?.full_path),
        None => None,
    };
    // Summed per group and transaction, so each group can count its transactions.
    let rows: Vec<(String, i64, String, i64)> = sqlx::query_as(&format!(
        "SELECT {key}, t.id, t.status, SUM(p.amount) * ?1
         FROM postings p
         JOIN accounts a ON a.id = p.account_id
         JOIN transactions t ON t.id = p.transaction_id
         WHERE a.type = ?2 AND t.date >= ?3 AND t.date <= ?4
           AND (?5 IS NULL OR a.full_path = ?5 OR substr(a.full_path, 1, length(?5) + 1) = ?5 || ':')
         GROUP BY {key}, t.id"
    ))
    .bind(sign)
    .bind(account_type)
    .bind(grouping.start)
    .bind(grouping.end)
    .bind(within)
    .fetch_all(db)
    .await?;

    let mut totals = Totals::default();
    for (key, id, status, amount) in rows {
        let amount = Money::from_minor(amount);
        let key = match (grouping.group_by, grouping.category_depth) {
            (GroupBy::Category, Some(depth)) if depth > 0 => {
                key.split(':').take(depth).collect::<Vec<_>>().join(":")
            }
            _ => key,
        };
        totals.total += amount;
        if status == "pending" {
            totals.unreviewed += amount;
        }
        let group = totals.groups.entry(key).or_default();
        group.total += amount;
        group.transactions.insert(id);
    }
    Ok(totals)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SummaryGroup {
    pub key: String,
    pub total: String,
    pub transactions: usize,
}

/// Total spending or income over a period, grouped, as the chat's `spending_summary` reports
/// it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpendingSummary {
    pub kind: Kind,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub total: String,
    pub unreviewed: String,
    /// Largest first; months in date order.
    pub groups: Vec<SummaryGroup>,
    pub groups_not_shown: usize,
}

pub async fn spending_summary(
    db: &SqlitePool,
    grouping: &Grouping,
    limit: usize,
) -> Result<SpendingSummary, ReportError> {
    if grouping.end < grouping.start {
        return Err(ReportError::Invalid(
            "end_date must be on or after start_date.".into(),
        ));
    }
    let totals = group_totals(db, grouping).await?;
    let mut groups: Vec<(String, Group)> = totals.groups.into_iter().collect();
    if grouping.group_by == GroupBy::Month {
        groups.sort_by(|a, b| a.0.cmp(&b.0));
    } else {
        groups.sort_by(|a, b| b.1.total.cmp(&a.1.total).then_with(|| a.0.cmp(&b.0)));
    }
    let not_shown = groups.len().saturating_sub(limit);
    groups.truncate(limit);
    Ok(SpendingSummary {
        kind: grouping.kind,
        start_date: grouping.start,
        end_date: grouping.end,
        total: totals.total.two_places(),
        unreviewed: totals.unreviewed.two_places(),
        groups: groups
            .into_iter()
            .map(|(key, group)| SummaryGroup {
                key,
                total: group.total.two_places(),
                transactions: group.transactions.len(),
            })
            .collect(),
        groups_not_shown: not_shown,
    })
}

// Month by month.

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CashFlowMonth {
    /// e.g. "2026-08"
    pub month: String,
    pub income: String,
    pub expenses: String,
    pub net: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CashFlow {
    pub months: Vec<CashFlowMonth>,
    pub income_total: String,
    pub expenses_total: String,
    pub unreviewed_expenses: String,
}

/// The most months `cash_flow` covers.
pub const CASH_FLOW_MONTHS: u32 = 60;
/// The most month ends `net_worth_history` covers.
pub const NET_WORTH_MONTHS: u32 = 120;

fn check_months(months: u32, most: u32) -> Result<(), ReportError> {
    if (1..=most).contains(&months) {
        Ok(())
    } else {
        Err(ReportError::Invalid(format!(
            "months must be between 1 and {most}."
        )))
    }
}

/// Income, spending and what was left over in each of the `months` calendar months up to
/// and including `end`'s (so far), oldest first. Transfers between the user's own accounts
/// are neither.
pub async fn cash_flow(
    db: &SqlitePool,
    months: u32,
    end: NaiveDate,
) -> Result<CashFlow, ReportError> {
    check_months(months, CASH_FLOW_MONTHS)?;
    let starts = month_starts(end, months);
    let by_month = |kind| Grouping {
        kind,
        group_by: GroupBy::Month,
        ..Grouping::new(starts[0], end)
    };
    let income = group_totals(db, &by_month(Kind::Income)).await?;
    let spent = group_totals(db, &by_month(Kind::Expenses)).await?;
    let total_in =
        |totals: &Totals, key: &str| totals.groups.get(key).map(|g| g.total).unwrap_or_default();
    let months = starts
        .iter()
        .map(|start| {
            let key = month_key(*start);
            let (income, expenses) = (total_in(&income, &key), total_in(&spent, &key));
            CashFlowMonth {
                income: income.two_places(),
                expenses: expenses.two_places(),
                net: (income - expenses).two_places(),
                month: key,
            }
        })
        .collect();
    Ok(CashFlow {
        months,
        income_total: income.total.two_places(),
        expenses_total: spent.total.two_places(),
        unreviewed_expenses: spent.unreviewed.two_places(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetWorthPoint {
    pub date: NaiveDate,
    pub net_worth: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetWorthHistory {
    pub currencies: BTreeMap<String, Vec<NetWorthPoint>>,
}

/// Net worth (asset accounts less liabilities) at the end of each of the `months` calendar
/// months up to `end`'s, oldest first, per currency; the last point is `end` itself if its
/// month isn't over. Includes transactions not yet reviewed.
pub async fn net_worth_history(
    db: &SqlitePool,
    months: u32,
    end: NaiveDate,
) -> Result<NetWorthHistory, ReportError> {
    check_months(months, NET_WORTH_MONTHS)?;
    let points: Vec<NaiveDate> = month_starts(end, months)
        .into_iter()
        .map(|start| month_end(start).min(end))
        .collect();
    let mut currencies: BTreeMap<String, Vec<NetWorthPoint>> = BTreeMap::new();
    let rows: Vec<(String, NaiveDate, Money)> = sqlx::query_as(
        "SELECT a.currency, t.date, SUM(p.amount)
         FROM postings p
         JOIN accounts a ON a.id = p.account_id
         JOIN transactions t ON t.id = p.transaction_id
         WHERE a.type IN ('asset', 'liability') AND t.date <= ?
         GROUP BY a.currency, t.date
         ORDER BY t.date",
    )
    .bind(end)
    .fetch_all(db)
    .await?;
    let mut by_currency: BTreeMap<String, Vec<(NaiveDate, Money)>> = BTreeMap::new();
    for (currency, day, amount) in rows {
        by_currency.entry(currency).or_default().push((day, amount));
    }
    for (currency, days) in by_currency {
        let mut balance = Money::ZERO;
        let mut next = days.iter().peekable();
        // Daily totals come from the database; running them up to each point is cheap.
        let series = points
            .iter()
            .map(|point| {
                while let Some((_, amount)) = next.next_if(|(day, _)| day <= point) {
                    balance += *amount;
                }
                NetWorthPoint {
                    date: *point,
                    net_worth: balance.two_places(),
                }
            })
            .collect();
        currencies.insert(currency, series);
    }
    Ok(NetWorthHistory { currencies })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{AccountType, NewAccount};
    use crate::db;
    use crate::ledger::{self, Balanced, NewPosting, NewTransaction, Status};
    use crate::lookups::CategorisationSource;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn m(s: &str) -> Money {
        s.parse().unwrap()
    }

    struct Ledger {
        pool: SqlitePool,
        ids: HashMap<&'static str, i64>,
    }

    impl Ledger {
        async fn new() -> Self {
            let pool = db::open_in_memory().await.unwrap();
            let mut ids = HashMap::new();
            for (path, kind) in [
                ("Assets", AccountType::Asset),
                ("Assets:Bank", AccountType::Asset),
                ("Assets:Bank:Current", AccountType::Asset),
                ("Assets:Bank:Savings", AccountType::Asset),
                ("Liabilities", AccountType::Liability),
                ("Liabilities:Visa", AccountType::Liability),
                ("Income", AccountType::Income),
                ("Income:Salary", AccountType::Income),
                ("Expenses", AccountType::Expense),
                ("Expenses:Food", AccountType::Expense),
                ("Expenses:Food:Groceries", AccountType::Expense),
                ("Expenses:Food:Restaurants", AccountType::Expense),
                ("Expenses:Transport", AccountType::Expense),
            ] {
                let parent_id = path.rsplit_once(':').map(|(parent, _)| ids[parent]);
                let account = accounts::create(
                    &pool,
                    NewAccount {
                        name: path.rsplit(':').next().unwrap().to_owned(),
                        full_path: path.to_owned(),
                        parent_id,
                        kind,
                        currency: "GBP".into(),
                        bank_identifier: None,
                        icon: None,
                    },
                )
                .await
                .unwrap();
                ids.insert(path, account.id);
            }
            Self { pool, ids }
        }

        async fn txn(
            &self,
            date: &str,
            description: &str,
            postings: &[(&str, &str)],
            confirmed: bool,
        ) {
            let postings = postings
                .iter()
                .map(|(path, amount)| NewPosting {
                    account_id: self.ids[path],
                    amount: m(amount),
                    categorised_by: CategorisationSource::Manual,
                    rule_id: None,
                })
                .collect();
            let status = if confirmed {
                Status::Confirmed
            } else {
                Status::Pending
            };
            ledger::create(
                &self.pool,
                &NewTransaction {
                    date: day(date),
                    description: description.into(),
                    status,
                    currency: "GBP".into(),
                    import_batch_id: None,
                    import_metadata: None,
                    postings: Balanced::new(postings).unwrap(),
                },
            )
            .await
            .unwrap();
        }

        async fn spend(&self, date: &str, description: &str, category: &str, amount: &str) {
            self.spend_with(
                date,
                description,
                category,
                amount,
                "Assets:Bank:Current",
                true,
            )
            .await;
        }

        async fn spend_with(
            &self,
            date: &str,
            description: &str,
            category: &str,
            amount: &str,
            paid_with: &str,
            confirmed: bool,
        ) {
            let negated = (-m(amount)).to_string();
            self.txn(
                date,
                description,
                &[(category, amount), (paid_with, &negated)],
                confirmed,
            )
            .await;
        }

        /// A few months of activity.
        async fn spending() -> Self {
            let l = Self::new().await;
            l.spend(
                "2026-07-03",
                "TESCO STORES 1",
                "Expenses:Food:Groceries",
                "40.00",
            )
            .await;
            l.spend(
                "2026-08-03",
                "TESCO STORES 1",
                "Expenses:Food:Groceries",
                "55.50",
            )
            .await;
            l.spend_with(
                "2026-08-10",
                "PIZZA 100% REAL",
                "Expenses:Food:Restaurants",
                "30.00",
                "Liabilities:Visa",
                true,
            )
            .await;
            l.spend("2026-08-12", "TRAINLINE", "Expenses:Transport", "25.00")
                .await;
            l.spend(
                "2026-08-20",
                "TESCO REFUND",
                "Expenses:Food:Groceries",
                "-5.50",
            )
            .await;
            l.spend_with(
                "2026-08-25",
                "ALDI",
                "Expenses:Food:Groceries",
                "10.00",
                "Assets:Bank:Current",
                false,
            )
            .await;
            l.txn(
                "2026-08-28",
                "ACME PAYROLL",
                &[
                    ("Assets:Bank:Current", "2000.00"),
                    ("Income:Salary", "-2000.00"),
                ],
                true,
            )
            .await;
            l.txn(
                "2026-08-29",
                "TO SAVINGS",
                &[
                    ("Assets:Bank:Current", "-100.00"),
                    ("Assets:Bank:Savings", "100.00"),
                ],
                true,
            )
            .await;
            l.txn(
                "2026-08-30",
                "SUPERMARKET + TRAIN",
                &[
                    ("Assets:Bank:Current", "-20.00"),
                    ("Expenses:Food:Groceries", "12.00"),
                    ("Expenses:Transport", "8.00"),
                ],
                true,
            )
            .await;
            l
        }
    }

    fn august() -> Grouping {
        Grouping::new(day("2026-08-01"), day("2026-08-31"))
    }

    #[tokio::test]
    async fn spending_by_category() {
        let l = Ledger::spending().await;
        let summary = spending_summary(&l.pool, &august(), 25).await.unwrap();
        // 55.50 + 30 + 25 - 5.50 + 10 (pending) + 12 + 8
        assert_eq!(summary.total, "135.00");
        assert_eq!(summary.unreviewed, "10.00");
        assert_eq!(
            summary.groups[0],
            SummaryGroup {
                key: "Expenses:Food:Groceries".into(),
                total: "72.00".into(),
                transactions: 4,
            }
        );

        let rolled = Grouping {
            category_depth: Some(2),
            ..august()
        };
        let rolled = spending_summary(&l.pool, &rolled, 25).await.unwrap();
        let totals: Vec<(&str, &str)> = rolled
            .groups
            .iter()
            .map(|g| (g.key.as_str(), g.total.as_str()))
            .collect();
        assert_eq!(
            totals,
            [("Expenses:Food", "102.00"), ("Expenses:Transport", "33.00")]
        );
    }

    #[tokio::test]
    async fn spending_by_month_merchant_and_within_a_category() {
        let l = Ledger::spending().await;
        let months = Grouping {
            group_by: GroupBy::Month,
            category: Some("Food".into()),
            ..Grouping::new(day("2026-07-01"), day("2026-08-31"))
        };
        let months = spending_summary(&l.pool, &months, 25).await.unwrap();
        let totals: Vec<(&str, &str)> = months
            .groups
            .iter()
            .map(|g| (g.key.as_str(), g.total.as_str()))
            .collect();
        assert_eq!(totals, [("2026-07", "40.00"), ("2026-08", "102.00")]);

        let merchants = Grouping {
            group_by: GroupBy::Merchant,
            ..august()
        };
        let merchants = spending_summary(&l.pool, &merchants, 2).await.unwrap();
        let keys: Vec<&str> = merchants.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["TESCO STORES 1", "PIZZA 100% REAL"]);
        assert_eq!(merchants.groups_not_shown, 4);

        let unknown = Grouping {
            category: Some("Gadgets".into()),
            ..august()
        };
        assert_eq!(
            spending_summary(&l.pool, &unknown, 25)
                .await
                .unwrap_err()
                .to_string(),
            "No category called 'Gadgets'. Use list_categories or list_accounts to see them."
        );
        let backwards = Grouping::new(day("2026-02-01"), day("2026-01-01"));
        assert_eq!(
            spending_summary(&l.pool, &backwards, 25)
                .await
                .unwrap_err()
                .to_string(),
            "end_date must be on or after start_date."
        );
    }

    #[tokio::test]
    async fn income_is_positive() {
        let l = Ledger::spending().await;
        let income = Grouping {
            kind: Kind::Income,
            ..august()
        };
        let income = spending_summary(&l.pool, &income, 25).await.unwrap();
        assert_eq!(income.total, "2000.00");
        assert_eq!(
            income.groups,
            [SummaryGroup {
                key: "Income:Salary".into(),
                total: "2000.00".into(),
                transactions: 1,
            }]
        );
    }

    #[tokio::test]
    async fn cash_flow_by_month_agrees_with_the_summary() {
        let l = Ledger::spending().await;
        let flow = cash_flow(&l.pool, 3, day("2026-09-15")).await.unwrap();
        let row = |month: &str, income: &str, expenses: &str, net: &str| CashFlowMonth {
            month: month.into(),
            income: income.into(),
            expenses: expenses.into(),
            net: net.into(),
        };
        assert_eq!(
            flow.months,
            [
                row("2026-07", "0.00", "40.00", "-40.00"),
                row("2026-08", "2000.00", "135.00", "1865.00"),
                row("2026-09", "0.00", "0.00", "0.00"),
            ]
        );
        assert_eq!(flow.income_total, "2000.00");
        assert_eq!(flow.expenses_total, "175.00");
        assert_eq!(flow.unreviewed_expenses, "10.00");
        assert!(cash_flow(&l.pool, 0, day("2026-09-15")).await.is_err());
        assert!(cash_flow(&l.pool, 61, day("2026-09-15")).await.is_err());
    }

    #[tokio::test]
    async fn net_worth_at_each_month_end() {
        let l = Ledger::spending().await;
        let history = net_worth_history(&l.pool, 4, day("2026-09-15"))
            .await
            .unwrap();
        let points: Vec<(String, &str)> = history.currencies["GBP"]
            .iter()
            .map(|p| (p.date.to_string(), p.net_worth.as_str()))
            .collect();
        assert_eq!(
            points,
            [
                ("2026-06-30".to_owned(), "0.00"),
                ("2026-07-31".to_owned(), "-40.00"),
                ("2026-08-31".to_owned(), "1825.00"),
                ("2026-09-15".to_owned(), "1825.00"),
            ]
        );
        let across_years = net_worth_history(&l.pool, 14, day("2026-01-10"))
            .await
            .unwrap();
        assert!(across_years.currencies.is_empty());
        assert_eq!(
            month_starts(day("2026-01-10"), 2),
            [day("2025-12-01"), day("2026-01-01")]
        );
    }

    #[tokio::test]
    async fn report_endpoints_use_confirmed_transactions() {
        let l = Ledger::spending().await;
        let s = summary(&l.pool, day("2026-08-01"), day("2026-08-31"))
            .await
            .unwrap();
        // The pending £10 at Aldi isn't counted anywhere.
        assert_eq!(s.net_worth["GBP"], m("1835"));
        assert_eq!(s.income, m("2000"));
        assert_eq!(s.expenses, m("125"));
        assert_eq!(s.savings_rate.to_string(), "93.8");
        let none = summary(&l.pool, day("2026-01-01"), day("2026-01-31"))
            .await
            .unwrap();
        assert_eq!(none.savings_rate, rust_decimal::Decimal::ZERO);

        let by_category =
            expenses_by_category(&l.pool, day("2026-08-01"), day("2026-08-31"), "Expenses")
                .await
                .unwrap();
        let totals: Vec<(&str, Money)> = by_category
            .iter()
            .map(|c| (c.full_path.as_str(), c.total))
            .collect();
        assert_eq!(
            totals,
            [
                ("Expenses:Food:Groceries", m("62")),
                ("Expenses:Transport", m("33")),
                ("Expenses:Food:Restaurants", m("30")),
            ]
        );
        let food = expenses_by_category(
            &l.pool,
            day("2026-08-01"),
            day("2026-08-31"),
            "Expenses:Food",
        )
        .await
        .unwrap();
        assert_eq!(food.len(), 2);
    }
}
