//! Higher-level read actions that answer broad questions in one call. The work is in
//! `crate::insights`.

use chrono::{Local, NaiveDate};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::SqlitePool;

use super::{Action, ActionError, Check, Output, Scope, in_range, parse, schema};
use crate::insights::{self, Comparison, DIGEST_MAX_TRANSACTIONS, Detail, PaymentKind};
use crate::reports::GroupBy;

pub fn actions() -> Vec<Action> {
    vec![
        action!(
            recurring_payments,
            RecurringPaymentsInput,
            Scope::Read,
            "Subscriptions, bills and other charges that repeat weekly, monthly, quarterly or \
             yearly and are still active, found from the last 13 months of spending. Each has \
             its typical amount, whether that amount varies, and what it costs a year; most \
             expensive per year first. Use this for questions about subscriptions, bills or \
             regular outgoings. Each has a kind (subscription, bill or other) once the insights \
             model has labelled it, otherwise \"unlabelled\"; pass kind to list only one kind."
        ),
        action!(
            unusual_charges,
            UnusualChargesInput,
            Scope::Read,
            "Charges that look unusual for their merchant or category (much more than usual, a \
             large one-off, or a possible mistake or fraud), and subscriptions or bills whose \
             latest charge went up, between two dates. Use this for \"any unusual charges?\", \
             \"anything odd on my card?\" or \"did any bills go up?\". Charges are only judged \
             once an insights model is set up in Settings > AI; unscored_charges says how many \
             in the period haven't been judged."
        ),
        action!(
            compare_periods,
            ComparePeriodsInput,
            Scope::Read,
            "Compare spending in two periods side by side, grouped by category or merchant, \
             with the change in each group; groups that changed most come first. Use this \
             instead of calling spending_summary once per period."
        ),
        action!(
            savings_opportunities,
            SavingsOpportunitiesInput,
            Scope::Read,
            "One-call overview of where the user could save money: average monthly spending by \
             category and merchant over recent full months, categories that went up last month \
             compared with the months before, active subscriptions and bills with their yearly \
             cost, and budgets that are overspent this period. Start here for broad questions \
             like \"how can I save money?\", \"where does my money go?\" or \"what should I cut \
             back on?\", then drill in with other actions only if needed."
        ),
        action!(
            transactions_digest,
            TransactionsDigestInput,
            Scope::Read,
            "Every categorised transaction in a date range as compact CSV (transfers between \
             the user's own accounts are left out), so you can read them all at once and spot \
             patterns: subscriptions, habits, unusual or growing spending. Amounts are positive \
             for spending and negative for income or refunds; status \"pending\" means \
             unreviewed. Over 1000 transactions it gives one row per merchant instead (count, \
             total, typical amount, first and last date). Use it to find things; for totals \
             quote \"total_spending\", \"total_income\" or the other actions rather than adding \
             up rows yourself."
        ),
    ]
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn output(value: serde_json::Value) -> Output {
    Output::new(value)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecurringPaymentsInput {
    /// Report payments still active on this date. Defaults to today.
    #[serde(default)]
    on: Option<NaiveDate>,
    /// Only payments of this kind: subscription (services signed up for), bill (household
    /// essentials and contracts) or other (ordinary spending that happens to repeat, such as
    /// groceries).
    #[serde(default)]
    kind: PaymentKind,
    #[serde(default = "thirty")]
    #[schemars(range(min = 1, max = 100))]
    limit: i64,
}

fn thirty() -> i64 {
    30
}

impl Check for RecurringPaymentsInput {
    fn check(&self) -> Result<(), String> {
        in_range("limit", self.limit, 1, 100)
    }
}

pub async fn recurring_payments(
    db: &SqlitePool,
    input: RecurringPaymentsInput,
) -> Result<Output, ActionError> {
    let on = input.on.unwrap_or_else(today);
    Ok(output(
        insights::recurring_payments(db, on, input.kind, input.limit as usize).await?,
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnusualChargesInput {
    /// First day to include.
    start_date: NaiveDate,
    /// Last day to include. Defaults to today.
    #[serde(default)]
    end_date: Option<NaiveDate>,
    #[serde(default = "twenty")]
    #[schemars(range(min = 1, max = 100))]
    limit: i64,
}

fn twenty() -> i64 {
    20
}

impl Check for UnusualChargesInput {
    fn check(&self) -> Result<(), String> {
        in_range("limit", self.limit, 1, 100)
    }
}

pub async fn unusual_charges(
    db: &SqlitePool,
    input: UnusualChargesInput,
) -> Result<Output, ActionError> {
    let end = input.end_date.unwrap_or_else(today);
    Ok(output(
        insights::unusual_charges(db, input.start_date, end, input.limit as usize).await?,
    ))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CompareBy {
    #[default]
    Category,
    Merchant,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComparePeriodsInput {
    /// First day of the period to look at.
    start_date: NaiveDate,
    /// Last day of the period to look at (inclusive).
    end_date: NaiveDate,
    /// First day of the period to compare against. Defaults to the same number of days
    /// immediately before start_date.
    #[serde(default)]
    compare_start_date: Option<NaiveDate>,
    /// Last day of the period to compare against (inclusive).
    #[serde(default)]
    compare_end_date: Option<NaiveDate>,
    /// Group by category or by merchant.
    #[serde(default)]
    group_by: CompareBy,
    /// Only include this category and its subcategories (path or name).
    #[serde(default)]
    category: Option<String>,
    /// When grouping by category, roll subcategories up to this many path levels.
    #[serde(default)]
    #[schemars(range(min = 1, max = 6))]
    category_depth: Option<i64>,
    /// Maximum number of groups.
    #[serde(default = "fifteen")]
    #[schemars(range(min = 1, max = 100))]
    limit: i64,
}

fn fifteen() -> i64 {
    15
}

impl Check for ComparePeriodsInput {
    fn check(&self) -> Result<(), String> {
        if let Some(depth) = self.category_depth {
            in_range("category_depth", depth, 1, 6)?;
        }
        in_range("limit", self.limit, 1, 100)
    }
}

pub async fn compare_periods(
    db: &SqlitePool,
    input: ComparePeriodsInput,
) -> Result<Output, ActionError> {
    let comparison = Comparison {
        start: input.start_date,
        end: input.end_date,
        compare_start: input.compare_start_date,
        compare_end: input.compare_end_date,
        group_by: match input.group_by {
            CompareBy::Category => GroupBy::Category,
            CompareBy::Merchant => GroupBy::Merchant,
        },
        category: input.category,
        category_depth: input.category_depth.map(|d| d as usize),
        limit: input.limit as usize,
    };
    Ok(output(insights::compare_periods(db, comparison).await?))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavingsOpportunitiesInput {
    /// How many recent full calendar months to average over.
    #[serde(default = "three")]
    #[schemars(range(min = 1, max = 12))]
    months: i64,
    /// Treat this as today. Defaults to today.
    #[serde(default)]
    on: Option<NaiveDate>,
}

fn three() -> i64 {
    3
}

impl Check for SavingsOpportunitiesInput {
    fn check(&self) -> Result<(), String> {
        in_range("months", self.months, 1, 12)
    }
}

pub async fn savings_opportunities(
    db: &SqlitePool,
    input: SavingsOpportunitiesInput,
) -> Result<Output, ActionError> {
    let today = input.on.unwrap_or_else(today);
    Ok(output(
        insights::savings_opportunities(db, input.months as u32, today).await?,
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransactionsDigestInput {
    /// First day to include.
    start_date: NaiveDate,
    /// Last day to include (inclusive).
    end_date: NaiveDate,
    /// "transactions" lists every transaction, "merchants" one row per merchant. "auto" lists
    /// transactions when there are at most 1000, otherwise merchants.
    #[serde(default)]
    detail: Detail,
}

impl Check for TransactionsDigestInput {}

pub async fn transactions_digest(
    db: &SqlitePool,
    input: TransactionsDigestInput,
) -> Result<Output, ActionError> {
    Ok(output(
        insights::transactions_digest(
            db,
            input.start_date,
            input.end_date,
            input.detail,
            DIGEST_MAX_TRANSACTIONS,
        )
        .await?,
    ))
}
