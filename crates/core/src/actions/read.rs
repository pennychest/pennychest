//! Actions that look things up. Anyone may run them.

use chrono::{Local, NaiveDate};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use super::common::{self, CATEGORY_TYPES, MONEY_ACCOUNT_TYPES, find_money_account};
use super::{Action, ActionError, Check, Output, Scope, in_range, parse, schema};
use crate::accounts::{self, AccountType};
use crate::budgets;
use crate::reports::{self, GroupBy, Grouping, Kind};
use crate::{Money, lookups::MatchType};

pub fn actions() -> Vec<Action> {
    vec![
        action!(
            list_categories,
            ListCategoriesInput,
            Scope::Read,
            "List the user's categories (spending and income), as colon-separated paths from \
             general to specific, e.g. \"Expenses:Food:Groceries\". Spending in a category \
             includes its subcategories."
        ),
        action!(
            list_accounts,
            Nothing,
            Scope::Read,
            "List the user's bank accounts, cards and other asset or liability accounts."
        ),
        action!(
            spending_summary,
            SpendingSummaryInput,
            Scope::Read,
            "Total spending (or income) over a date range, grouped by category, month or \
             merchant. Amounts are exact sums from the database; spending is positive and \
             refunds reduce it. \"unreviewed\" is the part from imported transactions the user \
             hasn't reviewed yet. Groups are sorted largest first (months are sorted by date)."
        ),
        action!(
            list_transactions,
            ListTransactionsInput,
            Scope::Read,
            "Find transactions, newest first. Each has its category (or categories), the \
             amount categorised (positive for spending, negative for income or refunds), the \
             bank account or card it was paid from or into, and whether it has been reviewed. \
             Returns the total number of matches so results can be paged with offset."
        ),
        action!(
            list_rules,
            Nothing,
            Scope::Read,
            "List the rules that categorise imported transactions automatically: a pattern \
             matched against the description, the category it assigns, and a priority (higher \
             wins)."
        ),
        action!(
            get_budgets,
            GetBudgetsInput,
            Scope::Read,
            "The user's budgets with how much has been spent against each in the period \
             containing the given date. \"remaining\" is negative when a budget is overspent."
        ),
        action!(
            list_card_taps,
            ListCardTapsInput,
            Scope::Read,
            "Card payments recorded by the phone wallet shortcut. Unmatched taps are spending \
             that hasn't appeared on an imported statement yet."
        ),
        action!(
            cash_flow,
            CashFlowInput,
            Scope::Read,
            "Income, spending and what was left over in each calendar month, oldest first, up \
             to and including the current month (so far). Spending is positive and refunds \
             reduce it; transfers between the user's own accounts are neither. Use this for \
             \"am I spending more than I earn?\" or month-by-month comparisons of money in and \
             out."
        ),
        action!(
            net_worth_history,
            NetWorthHistoryInput,
            Scope::Read,
            "Net worth (everything in the user's bank, savings and other asset accounts minus \
             what they owe on cards and loans) at the end of each calendar month, oldest first, \
             per currency. The last point is today's balance if the month isn't over. Includes \
             transactions not yet reviewed."
        ),
    ]
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// An action that takes no input.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Nothing {}

impl Check for Nothing {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CategoryKind {
    Expense,
    Income,
    #[default]
    All,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListCategoriesInput {
    /// Spending categories, income categories, or both.
    #[serde(default)]
    kind: CategoryKind,
}

impl Check for ListCategoriesInput {}

pub async fn list_categories(
    db: &SqlitePool,
    input: ListCategoriesInput,
) -> Result<Output, ActionError> {
    let kinds: &[AccountType] = match input.kind {
        CategoryKind::Expense => &[AccountType::Expense],
        CategoryKind::Income => &[AccountType::Income],
        CategoryKind::All => &CATEGORY_TYPES,
    };
    let categories: Vec<_> = accounts::list(db)
        .await?
        .into_iter()
        .filter(|a| kinds.contains(&a.kind))
        .map(|a| json!({ "path": a.full_path, "name": a.name, "type": a.kind }))
        .collect();
    Ok(Output::new(json!({ "categories": categories })))
}

pub async fn list_accounts(db: &SqlitePool, _: Nothing) -> Result<Output, ActionError> {
    let accounts: Vec<_> = accounts::list(db)
        .await?
        .into_iter()
        .filter(|a| MONEY_ACCOUNT_TYPES.contains(&a.kind))
        .map(|a| {
            json!({ "path": a.full_path, "name": a.name, "type": a.kind, "currency": a.currency })
        })
        .collect();
    Ok(Output::new(json!({ "accounts": accounts })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpendingSummaryInput {
    /// First day to include.
    start_date: NaiveDate,
    /// Last day to include (inclusive).
    end_date: NaiveDate,
    /// Group totals by category, by calendar month, or by merchant (the transaction
    /// description).
    #[serde(default)]
    group_by: GroupBy,
    /// Summarise spending or income.
    #[serde(default)]
    kind: Kind,
    /// Only include this category and its subcategories (path or name).
    #[serde(default)]
    category: Option<String>,
    /// When grouping by category, roll subcategories up to this many path levels, e.g. 2
    /// groups "Expenses:Food:Groceries" under "Expenses:Food".
    #[serde(default)]
    #[schemars(range(min = 1, max = 6))]
    category_depth: Option<i64>,
    /// Maximum number of groups.
    #[serde(default = "twenty_five")]
    #[schemars(range(min = 1, max = 200))]
    limit: i64,
}

fn twenty_five() -> i64 {
    25
}

impl Check for SpendingSummaryInput {
    fn check(&self) -> Result<(), String> {
        if let Some(depth) = self.category_depth {
            in_range("category_depth", depth, 1, 6)?;
        }
        in_range("limit", self.limit, 1, 200)
    }
}

pub async fn spending_summary(
    db: &SqlitePool,
    input: SpendingSummaryInput,
) -> Result<Output, ActionError> {
    let grouping = Grouping {
        kind: input.kind,
        group_by: input.group_by,
        category: input.category,
        category_depth: input.category_depth.map(|d| d as usize),
        ..Grouping::new(input.start_date, input.end_date)
    };
    let summary = reports::spending_summary(db, &grouping, input.limit as usize).await?;
    Ok(Output::new(
        serde_json::to_value(summary).expect("summaries serialise"),
    ))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StatusFilter {
    Pending,
    Confirmed,
    #[default]
    Any,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTransactionsInput {
    /// Earliest date to include.
    #[serde(default)]
    start_date: Option<NaiveDate>,
    /// Latest date to include.
    #[serde(default)]
    end_date: Option<NaiveDate>,
    /// Case-insensitive text to look for in the description.
    #[serde(default)]
    search: Option<String>,
    /// Only this category and its subcategories (path or name).
    #[serde(default)]
    category: Option<String>,
    /// With category, leave out its subcategories: only transactions recorded against the
    /// category itself.
    #[serde(default)]
    exact_category: bool,
    /// Only transactions on this bank account or card (path or name).
    #[serde(default)]
    account: Option<String>,
    /// "pending" means imported but not yet reviewed.
    #[serde(default)]
    status: StatusFilter,
    /// Smallest categorised amount to include.
    #[serde(default)]
    min_amount: Option<Money>,
    /// Largest categorised amount to include.
    #[serde(default)]
    max_amount: Option<Money>,
    #[serde(default = "fifty")]
    #[schemars(range(min = 1, max = 200))]
    limit: i64,
    #[serde(default)]
    #[schemars(range(min = 0))]
    offset: i64,
}

fn fifty() -> i64 {
    50
}

impl Check for ListTransactionsInput {
    fn check(&self) -> Result<(), String> {
        in_range("limit", self.limit, 1, 200)?;
        in_range("offset", self.offset, 0, i64::MAX)
    }
}

/// The filters of a `list_transactions` call, with categories and accounts looked up.
struct Filters {
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    search: Option<String>,
    status: Option<&'static str>,
    /// A category's id, for that category only.
    exact_category: Option<i64>,
    /// Paths of accounts whose transactions, including beneath them, to include.
    within: Vec<String>,
    min_amount: Option<Money>,
    max_amount: Option<Money>,
}

impl Filters {
    fn push(&self, query: &mut QueryBuilder<'_, Sqlite>) {
        query.push(" FROM transactions t WHERE 1 = 1");
        if let Some(start) = self.start {
            query.push(" AND t.date >= ").push_bind(start);
        }
        if let Some(end) = self.end {
            query.push(" AND t.date <= ").push_bind(end);
        }
        if let Some(search) = &self.search {
            query
                .push(" AND instr(lower(t.description), lower(")
                .push_bind(search.clone())
                .push(")) > 0");
        }
        if let Some(status) = self.status {
            query.push(" AND t.status = ").push_bind(status);
        }
        if let Some(category) = self.exact_category {
            query
                .push(" AND t.id IN (SELECT transaction_id FROM postings WHERE account_id = ")
                .push_bind(category)
                .push(")");
        }
        for path in &self.within {
            query
                .push(
                    " AND t.id IN (SELECT p.transaction_id FROM postings p
                      JOIN accounts a ON a.id = p.account_id WHERE a.full_path = ",
                )
                .push_bind(path.clone())
                .push(" OR substr(a.full_path, 1, ")
                .push_bind(path.chars().count() as i64 + 1)
                .push(") = ")
                .push_bind(format!("{path}:"))
                .push(")");
        }
        if self.min_amount.is_some() || self.max_amount.is_some() {
            query.push(
                " AND t.id IN (SELECT p.transaction_id FROM postings p
                  JOIN accounts a ON a.id = p.account_id WHERE a.type IN ('expense', 'income')",
            );
            if let Some(min) = self.min_amount {
                query.push(" AND p.amount >= ").push_bind(min);
            }
            if let Some(max) = self.max_amount {
                query.push(" AND p.amount <= ").push_bind(max);
            }
            query.push(")");
        }
    }
}

pub async fn list_transactions(
    db: &SqlitePool,
    input: ListTransactionsInput,
) -> Result<Output, ActionError> {
    let mut filters = Filters {
        start: input.start_date,
        end: input.end_date,
        search: input.search,
        status: match input.status {
            StatusFilter::Pending => Some("pending"),
            StatusFilter::Confirmed => Some("confirmed"),
            StatusFilter::Any => None,
        },
        exact_category: None,
        within: Vec::new(),
        min_amount: input.min_amount,
        max_amount: input.max_amount,
    };
    if let Some(category) = input.category {
        let category = accounts::find_category(db, &category).await?;
        if input.exact_category {
            filters.exact_category = Some(category.id);
        } else {
            filters.within.push(category.full_path);
        }
    }
    if let Some(account) = input.account {
        filters
            .within
            .push(find_money_account(db, &account).await?.full_path);
    }

    let mut count = QueryBuilder::new("SELECT COUNT(*)");
    filters.push(&mut count);
    let total: i64 = count.build_query_scalar().fetch_one(db).await?;

    let mut page = QueryBuilder::new("SELECT t.id");
    filters.push(&mut page);
    page.push(" ORDER BY t.date DESC, t.id DESC LIMIT ")
        .push_bind(input.limit)
        .push(" OFFSET ")
        .push_bind(input.offset);
    let ids: Vec<i64> = page.build_query_scalar().fetch_all(db).await?;

    let loaded = common::load(db, &ids).await?;
    let transactions: Vec<_> = ids
        .iter()
        .filter_map(|id| loaded.get(id))
        .map(|t| t.describe())
        .collect();
    Ok(Output::new(json!({
        "total": total,
        "offset": input.offset,
        "transactions": transactions,
    })))
}

pub async fn list_rules(db: &SqlitePool, _: Nothing) -> Result<Output, ActionError> {
    let rules: Vec<(i64, String, i64, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT r.id, r.pattern, r.match_type_id, a.full_path, r.priority, r.description
         FROM rules r JOIN accounts a ON a.id = r.target_account_id
         ORDER BY r.priority DESC, r.pattern",
    )
    .fetch_all(db)
    .await?;
    let rules: Vec<_> = rules
        .into_iter()
        .map(
            |(id, pattern, match_type, category, priority, description)| {
                json!({
                    "id": id,
                    "pattern": pattern,
                    "match_type": MatchType::try_from(match_type).ok().map(MatchType::name),
                    "category": category,
                    "priority": priority,
                    "description": description,
                })
            },
        )
        .collect();
    Ok(Output::new(json!({ "rules": rules })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetBudgetsInput {
    /// Any date in the month (or year, for annual budgets) to report on. Defaults to today.
    #[serde(default)]
    on: Option<NaiveDate>,
}

impl Check for GetBudgetsInput {}

pub async fn get_budgets(db: &SqlitePool, input: GetBudgetsInput) -> Result<Output, ActionError> {
    let mut list = budgets::list(db, input.on.unwrap_or_else(today)).await?;
    list.sort_by(|a, b| {
        (a.period.name(), &a.account_full_path).cmp(&(b.period.name(), &b.account_full_path))
    });
    let budgets: Vec<_> = list
        .into_iter()
        .map(|b| {
            json!({
                "category": b.account_full_path,
                "period": b.period,
                "period_start": b.period_start,
                "period_end": b.period_end,
                "limit": b.amount.two_places(),
                "spent": b.spent.two_places(),
                "unreviewed": b.unreviewed.two_places(),
                "remaining": b.remaining.two_places(),
            })
        })
        .collect();
    Ok(Output::new(json!({ "budgets": budgets })))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TapStatus {
    #[default]
    Unmatched,
    Reconciled,
    Dismissed,
    All,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListCardTapsInput {
    /// "unmatched" taps are card payments not yet seen on an imported statement.
    #[serde(default)]
    status: TapStatus,
    #[serde(default = "fifty")]
    #[schemars(range(min = 1, max = 200))]
    limit: i64,
}

impl Check for ListCardTapsInput {
    fn check(&self) -> Result<(), String> {
        in_range("limit", self.limit, 1, 200)
    }
}

pub async fn list_card_taps(
    db: &SqlitePool,
    input: ListCardTapsInput,
) -> Result<Output, ActionError> {
    let filter = match input.status {
        TapStatus::Unmatched => "transaction_id IS NULL AND dismissed = 0",
        TapStatus::Reconciled => "transaction_id IS NOT NULL",
        TapStatus::Dismissed => "dismissed = 1",
        TapStatus::All => "1 = 1",
    };
    type Tap = (
        i64,
        NaiveDate,
        String,
        Money,
        String,
        Option<String>,
        Option<i64>,
        bool,
    );
    let taps: Vec<Tap> = sqlx::query_as(&format!(
        "SELECT id, tapped_on, merchant, amount, currency, card_name, transaction_id, dismissed
         FROM card_taps WHERE {filter} ORDER BY tapped_on DESC, id DESC LIMIT ?"
    ))
    .bind(input.limit)
    .fetch_all(db)
    .await?;
    let taps: Vec<_> = taps
        .into_iter()
        .map(
            |(id, date, merchant, amount, currency, card, matched, dismissed)| {
                json!({
                    "id": id,
                    "date": date,
                    "merchant": merchant,
                    "amount": amount.two_places(),
                    "currency": currency,
                    "card": card,
                    "matched_transaction_id": matched,
                    "dismissed": dismissed,
                })
            },
        )
        .collect();
    Ok(Output::new(json!({ "taps": taps })))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CashFlowInput {
    /// How many calendar months.
    #[serde(default = "twelve")]
    #[schemars(range(min = 1, max = 60))]
    months: i64,
    /// A day in the last month to include. Defaults to today.
    #[serde(default)]
    end: Option<NaiveDate>,
}

fn twelve() -> i64 {
    12
}

impl Check for CashFlowInput {
    fn check(&self) -> Result<(), String> {
        in_range("months", self.months, 1, reports::CASH_FLOW_MONTHS.into())
    }
}

pub async fn cash_flow(db: &SqlitePool, input: CashFlowInput) -> Result<Output, ActionError> {
    let end = input.end.unwrap_or_else(today);
    let flow = reports::cash_flow(db, input.months as u32, end).await?;
    Ok(Output::new(
        serde_json::to_value(flow).expect("cash flow serialises"),
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetWorthHistoryInput {
    /// How many month ends.
    #[serde(default = "twelve")]
    #[schemars(range(min = 1, max = 120))]
    months: i64,
    /// A day in the last month to include. Defaults to today.
    #[serde(default)]
    end: Option<NaiveDate>,
}

impl Check for NetWorthHistoryInput {
    fn check(&self) -> Result<(), String> {
        in_range("months", self.months, 1, reports::NET_WORTH_MONTHS.into())
    }
}

pub async fn net_worth_history(
    db: &SqlitePool,
    input: NetWorthHistoryInput,
) -> Result<Output, ActionError> {
    let end = input.end.unwrap_or_else(today);
    let history = reports::net_worth_history(db, input.months as u32, end).await?;
    Ok(Output::new(
        serde_json::to_value(history).expect("net worth serialises"),
    ))
}
