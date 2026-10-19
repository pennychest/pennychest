//! Answers to broad questions ("where can I save?", "what's changed?") in one call, so a model
//! doesn't have to piece them together from many small lookups: recurring payments, unusual
//! charges, comparing periods, savings opportunities and a digest of every transaction.
//!
//! The AI half (labelling merchants and scoring charges) is in `ai::insights`; this reads
//! what that stores in `merchant_labels` and `transactions.unusual_score`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{Datelike, Days, Months, NaiveDate};
use regex::Regex;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::Money;
use crate::budgets;
use crate::reports::{GroupBy, Grouping, ReportError, Totals, group_totals};

/// Writes an exact amount to two places, halves to even, as the actions report amounts.
pub(crate) fn money(value: Decimal) -> String {
    format!("{:.2}", value.round_dp(2))
}

fn decimal(amount: Money) -> Decimal {
    amount.to_decimal()
}

/// The middle value, or the mean of the middle two. `values` mustn't be empty.
pub(crate) fn median(values: &[Decimal]) -> Decimal {
    let mut sorted = values.to_vec();
    sorted.sort();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / Decimal::TWO
    }
}

/// The value seen most often, the first seen winning a tie.
fn most_common<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for value in values {
        match counts.iter_mut().find(|(v, _)| *v == value) {
            Some((_, count)) => *count += 1,
            None => counts.push((value, 1)),
        }
    }
    let best = counts.iter().map(|(_, c)| *c).max().unwrap_or_default();
    counts
        .into_iter()
        .find(|(_, c)| *c == best)
        .map(|(v, _)| v.to_owned())
        .unwrap_or_default()
}

fn months_before(month_start: NaiveDate, months: u32) -> NaiveDate {
    month_start - Months::new(months)
}

static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s*/#]+").unwrap());
static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d").unwrap());
static NOT_LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z&]+").unwrap());

/// A description with references stripped, so repeat charges from one merchant group
/// together: "NETFLIX.COM 8829301" and "NETFLIX.COM 1123987" both become "netflix com".
pub fn merchant_key(description: &str) -> String {
    let lower = description.to_lowercase();
    let kept: Vec<String> = SEPARATORS
        .split(&lower)
        .filter(|word| !DIGIT.is_match(word))
        .map(|word| NOT_LETTERS.replace_all(word, " ").into_owned())
        .collect();
    let key = kept
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if key.is_empty() {
        lower.trim().to_owned()
    } else {
        key
    }
}

/// How often a payment repeats: typical days between charges, the shortest and longest gap
/// that count, and charges a year.
struct Cadence {
    name: &'static str,
    days: i64,
    shortest: i64,
    longest: i64,
    per_year: i64,
}

const CADENCES: [Cadence; 4] = [
    Cadence {
        name: "weekly",
        days: 7,
        shortest: 5,
        longest: 9,
        per_year: 52,
    },
    Cadence {
        name: "monthly",
        days: 30,
        shortest: 25,
        longest: 35,
        per_year: 12,
    },
    Cadence {
        name: "quarterly",
        days: 91,
        shortest: 80,
        longest: 100,
        per_year: 4,
    },
    Cadence {
        name: "yearly",
        days: 365,
        shortest: 340,
        longest: 390,
        per_year: 1,
    },
];

/// How far back `find_recurring` looks: a little over a year, so yearly charges show.
pub const RECURRING_LOOKBACK_DAYS: u64 = 400;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recurring {
    /// The latest charge's description.
    pub merchant: String,
    pub merchant_key: String,
    pub category: String,
    pub cadence: &'static str,
    pub amount: String,
    pub amount_varies: bool,
    pub yearly_cost: String,
    pub charges: usize,
    pub first_date: NaiveDate,
    pub last_date: NaiveDate,
    pub last_amount: String,
    /// What it usually cost before the latest charge, to spot bills going up.
    pub usual_amount: String,
    /// subscription, bill or other once the insights model has labelled it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip)]
    yearly: Decimal,
    #[serde(skip)]
    last: Decimal,
    #[serde(skip)]
    usual: Decimal,
}

/// One transaction's spending: its total over expense postings, and the categories.
pub(crate) struct Charge {
    pub id: i64,
    pub date: NaiveDate,
    pub description: String,
    pub amount: Decimal,
    paths: Vec<String>,
    /// The category of the largest expense posting.
    pub category: String,
    largest: Decimal,
}

/// Spending per transaction from `start` to `end` inclusive, in transaction order. Refunds
/// (net negative) are left out.
pub(crate) async fn charges(
    db: &SqlitePool,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<Charge>, sqlx::Error> {
    let rows: Vec<(i64, NaiveDate, String, String, Money)> = sqlx::query_as(
        "SELECT t.id, t.date, t.description, a.full_path, p.amount
         FROM postings p
         JOIN accounts a ON a.id = p.account_id
         JOIN transactions t ON t.id = p.transaction_id
         WHERE a.type = 'expense' AND t.date >= ? AND t.date <= ?
         ORDER BY t.id, p.id",
    )
    .bind(start)
    .bind(end)
    .fetch_all(db)
    .await?;
    let mut charges: Vec<Charge> = Vec::new();
    for (id, date, description, path, amount) in rows {
        let amount = decimal(amount);
        match charges.last_mut() {
            Some(charge) if charge.id == id => {
                charge.amount += amount;
                if amount > charge.largest {
                    charge.category.clone_from(&path);
                    charge.largest = amount;
                }
                charge.paths.push(path);
            }
            _ => charges.push(Charge {
                id,
                date,
                description,
                amount,
                paths: vec![path.clone()],
                category: path,
                largest: amount,
            }),
        }
    }
    charges.retain(|c| c.amount > Decimal::ZERO);
    Ok(charges)
}

/// Charges that repeat on a regular cadence and are still active on `end`, most expensive
/// per year first.
pub async fn find_recurring(
    db: &SqlitePool,
    end: NaiveDate,
) -> Result<Vec<Recurring>, sqlx::Error> {
    let start = end - Days::new(RECURRING_LOOKBACK_DAYS - 1);
    let mut by_merchant: Vec<(String, Vec<Charge>)> = Vec::new();
    for charge in charges(db, start, end).await? {
        let key = merchant_key(&charge.description);
        match by_merchant.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.push(charge),
            None => by_merchant.push((key, vec![charge])),
        }
    }

    let mut found = Vec::new();
    for (key, mut group) in by_merchant {
        if group.len() < 2 {
            continue;
        }
        group.sort_by_key(|c| c.date);
        let gaps: Vec<i64> = group
            .windows(2)
            .map(|pair| (pair[1].date - pair[0].date).num_days())
            .collect();
        let mut sorted = gaps.clone();
        sorted.sort_unstable();
        // Twice the median, so an even count's half-day median stays a whole number.
        let middle = sorted.len() / 2;
        let twice_typical = if sorted.len() % 2 == 1 {
            2 * sorted[middle]
        } else {
            sorted[middle - 1] + sorted[middle]
        };
        let Some(cadence) = CADENCES
            .iter()
            .find(|c| 2 * c.shortest <= twice_typical && twice_typical <= 2 * c.longest)
        else {
            continue;
        };
        let regular = gaps
            .iter()
            .filter(|g| (cadence.shortest..=cadence.longest).contains(*g))
            .count();
        if (cadence.name != "yearly" && group.len() < 3) || regular * 3 < gaps.len() * 2 {
            continue;
        }
        let last = group.last().expect("at least two charges");
        // Stopped, if it's been more than half as long again as usual since the last one.
        if 2 * (end - last.date).num_days() > 3 * cadence.days {
            continue;
        }
        let amounts: Vec<Decimal> = group.iter().map(|c| c.amount).collect();
        let recent = &amounts[amounts.len().saturating_sub(3)..];
        let amount = median(recent);
        let before_last = &amounts[..amounts.len() - 1];
        let usual = median(&before_last[before_last.len().saturating_sub(3)..]);
        let lowest = recent.iter().min().copied().unwrap_or_default();
        let highest = recent.iter().max().copied().unwrap_or_default();
        let yearly = amount * Decimal::from(cadence.per_year);
        found.push(Recurring {
            merchant: last.description.clone(),
            merchant_key: key,
            category: most_common(
                group
                    .iter()
                    .flat_map(|c| c.paths.iter().map(String::as_str)),
            ),
            cadence: cadence.name,
            amount: money(amount),
            amount_varies: highest > lowest * Decimal::new(115, 2),
            yearly_cost: money(yearly),
            charges: group.len(),
            first_date: group[0].date,
            last_date: last.date,
            last_amount: money(last.amount),
            usual_amount: money(usual),
            kind: None,
            yearly: yearly.round_dp(2),
            last: last.amount.round_dp(2),
            usual: usual.round_dp(2),
        });
    }
    // Stable, so payments costing the same stay in the order their merchants first appear.
    found.sort_by_key(|r| std::cmp::Reverse(r.yearly));
    Ok(found)
}

/// What the insights model has called each merchant: subscription, bill or other.
async fn merchant_kinds(db: &SqlitePool) -> Result<HashMap<String, String>, sqlx::Error> {
    Ok(
        sqlx::query_as("SELECT merchant_key, kind FROM merchant_labels")
            .fetch_all(db)
            .await?
            .into_iter()
            .collect(),
    )
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum PaymentKind {
    #[default]
    Any,
    Subscription,
    Bill,
    Other,
}

pub async fn recurring_payments(
    db: &SqlitePool,
    on: NaiveDate,
    kind: PaymentKind,
    limit: usize,
) -> Result<Value, ReportError> {
    let kinds = merchant_kinds(db).await?;
    let mut found = find_recurring(db, on).await?;
    for payment in &mut found {
        payment.kind = Some(
            kinds
                .get(&payment.merchant_key)
                .cloned()
                .unwrap_or_else(|| "unlabelled".into()),
        );
    }
    if kind != PaymentKind::Any {
        let wanted = serde_json::to_value(kind).expect("kinds serialise");
        found.retain(|p| p.kind.as_deref() == wanted.as_str());
    }
    let total: Decimal = found.iter().map(|p| p.yearly).sum();
    let not_shown = found.len().saturating_sub(limit);
    found.truncate(limit);
    Ok(json!({
        "payments": found,
        "yearly_total": money(total),
        "payments_not_shown": not_shown,
    }))
}

/// Unusual-charge scores run from 0 (normal) to 1; at or above this a charge is reported.
pub const UNUSUAL_SCORE: f64 = 0.5;

pub async fn unusual_charges(
    db: &SqlitePool,
    start: NaiveDate,
    end: NaiveDate,
    limit: usize,
) -> Result<Value, ReportError> {
    if end < start {
        return Err(ReportError::Invalid(
            "end_date must not be before start_date.".into(),
        ));
    }
    let charges = charges(db, start, end).await?;
    let scores: HashMap<i64, Option<f64>> =
        sqlx::query_as("SELECT id, unusual_score FROM transactions WHERE date >= ? AND date <= ?")
            .bind(start)
            .bind(end)
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();
    let score = |id: i64| scores.get(&id).copied().flatten();
    let mut unusual: Vec<(f64, &Charge)> = charges
        .iter()
        .filter_map(|c| score(c.id).filter(|s| *s >= UNUSUAL_SCORE).map(|s| (s, c)))
        .collect();
    unusual.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.date.cmp(&b.1.date)));
    let not_shown = unusual.len().saturating_sub(limit);
    let unusual: Vec<Value> = unusual
        .into_iter()
        .take(limit)
        .map(|(score, c)| {
            json!({
                "transaction_id": c.id,
                "date": c.date,
                "description": c.description,
                "amount": money(c.amount),
                "category": c.category,
                "unusual_score": score,
            })
        })
        .collect();

    // A recurring payment's latest charge must beat its usual amount by this much to count
    // as a rise.
    let rise = Decimal::new(102, 2);
    let kinds = merchant_kinds(db).await?;
    let mut rises: Vec<(Decimal, Value)> = Vec::new();
    for r in find_recurring(db, end).await? {
        let kind = kinds
            .get(&r.merchant_key)
            .map_or("unlabelled", String::as_str);
        if kind != "other" && r.last_date >= start && r.last > r.usual * rise {
            let increase = r.last - r.usual;
            rises.push((
                increase,
                json!({
                    "merchant": r.merchant,
                    "kind": kind,
                    "cadence": r.cadence,
                    "usual_amount": r.usual_amount,
                    "latest_amount": r.last_amount,
                    "latest_date": r.last_date,
                    "increase": money(increase),
                }),
            ));
        }
    }
    rises.sort_by_key(|(increase, _)| std::cmp::Reverse(*increase));
    Ok(json!({
        "unusual_charges": unusual,
        "unusual_not_shown": not_shown,
        "price_rises": rises.into_iter().map(|(_, r)| r).collect::<Vec<_>>(),
        "unscored_charges": charges.iter().filter(|c| score(c.id).is_none()).count(),
    }))
}

/// What `compare_periods` compares.
pub struct Comparison {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub compare_start: Option<NaiveDate>,
    pub compare_end: Option<NaiveDate>,
    pub group_by: GroupBy,
    pub category: Option<String>,
    pub category_depth: Option<usize>,
    pub limit: usize,
}

fn total_of(totals: &Totals, key: &str) -> Decimal {
    totals
        .groups
        .get(key)
        .map(|g| decimal(g.total))
        .unwrap_or_default()
}

pub async fn compare_periods(db: &SqlitePool, c: Comparison) -> Result<Value, ReportError> {
    if c.end < c.start {
        return Err(ReportError::Invalid(
            "end_date must be on or after start_date.".into(),
        ));
    }
    let (compare_start, compare_end) = match (c.compare_start, c.compare_end) {
        (None, None) => {
            // The same number of days immediately before.
            let compare_end = c.start - Days::new(1);
            (compare_end - (c.end - c.start), compare_end)
        }
        (Some(start), Some(end)) if end < start => {
            return Err(ReportError::Invalid(
                "compare_end_date must be on or after compare_start_date.".into(),
            ));
        }
        (Some(start), Some(end)) => (start, end),
        _ => {
            return Err(ReportError::Invalid(
                "Give both compare_start_date and compare_end_date, or neither.".into(),
            ));
        }
    };
    let totals = |start, end| Grouping {
        group_by: c.group_by,
        category: c.category.clone(),
        category_depth: c.category_depth,
        ..Grouping::new(start, end)
    };
    let now = group_totals(db, &totals(c.start, c.end)).await?;
    let before = group_totals(db, &totals(compare_start, compare_end)).await?;
    let mut keys: Vec<&String> = now
        .groups
        .keys()
        .chain(before.groups.keys())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let change = |key: &str| total_of(&now, key) - total_of(&before, key);
    keys.sort_by(|a, b| change(b).abs().cmp(&change(a).abs()).then_with(|| a.cmp(b)));
    let not_shown = keys.len().saturating_sub(c.limit);
    let groups: Vec<Value> = keys
        .into_iter()
        .take(c.limit)
        .map(|key| {
            json!({
                "key": key,
                "total": money(total_of(&now, key)),
                "compared_total": money(total_of(&before, key)),
                "change": money(change(key)),
            })
        })
        .collect();
    Ok(json!({
        "period": {
            "start_date": c.start,
            "end_date": c.end,
            "total": now.total.two_places(),
            "unreviewed": now.unreviewed.two_places(),
        },
        "compared_with": {
            "start_date": compare_start,
            "end_date": compare_end,
            "total": before.total.two_places(),
            "unreviewed": before.unreviewed.two_places(),
        },
        "change": (now.total - before.total).two_places(),
        "groups": groups,
        "groups_not_shown": not_shown,
    }))
}

/// The groups with the largest totals, largest first.
fn top(totals: &Totals, n: usize) -> Vec<(&String, Decimal, usize)> {
    let mut groups: Vec<(&String, Decimal, usize)> = totals
        .groups
        .iter()
        .map(|(k, g)| (k, decimal(g.total), g.transactions.len()))
        .collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    groups.truncate(n);
    groups
}

/// An overview of where the user could save: average monthly spending by category and
/// merchant over the `months` full months before `today`'s, categories that went up last
/// month, recurring payments, and overspent budgets.
pub async fn savings_opportunities(
    db: &SqlitePool,
    months: u32,
    today: NaiveDate,
) -> Result<Value, ReportError> {
    let this_month = today.with_day(1).expect("every month has a first day");
    let start = months_before(this_month, months);
    let end = this_month - Days::new(1);
    let count = Decimal::from(months);
    let by_depth = |start, end| Grouping {
        category_depth: Some(2),
        ..Grouping::new(start, end)
    };

    let by_category = group_totals(db, &by_depth(start, end)).await?;
    let by_merchant = group_totals(
        db,
        &Grouping {
            group_by: GroupBy::Merchant,
            ..Grouping::new(start, end)
        },
    )
    .await?;

    // Last full month against the average of the months before it.
    let last_month = months_before(this_month, 1);
    let baseline_start = months_before(last_month, months);
    let recent = group_totals(db, &by_depth(last_month, end)).await?;
    let baseline = group_totals(db, &by_depth(baseline_start, last_month - Days::new(1))).await?;
    let mut rising: Vec<(&String, Decimal, Decimal)> = recent
        .groups
        .iter()
        .map(|(key, g)| (key, decimal(g.total), total_of(&baseline, key) / count))
        .filter(|(_, amount, usual)| *amount - *usual >= Decimal::ONE)
        .collect();
    rising.sort_by(|a, b| (b.1 - b.2).cmp(&(a.1 - a.2)).then_with(|| a.0.cmp(b.0)));

    let recurring = find_recurring(db, today).await?;
    let recurring_total: Decimal = recurring.iter().map(|r| r.yearly).sum();

    let mut overspent = Vec::new();
    for budget in budgets::list(db, today).await? {
        if budget.spent > budget.amount {
            overspent.push(json!({
                "category": budget.account_full_path,
                "period": budget.period,
                "limit": budget.amount.two_places(),
                "spent": budget.spent.two_places(),
                "over_by": (budget.spent - budget.amount).two_places(),
            }));
        }
    }

    Ok(json!({
        "averaged_over": { "start_date": start, "end_date": end, "months": months },
        "monthly_average_spending": money(decimal(by_category.total) / count),
        "unreviewed": by_category.unreviewed.two_places(),
        "top_categories": top(&by_category, 10)
            .into_iter()
            .map(|(k, v, _)| json!({ "category": k, "monthly_average": money(v / count) }))
            .collect::<Vec<_>>(),
        "top_merchants": top(&by_merchant, 10)
            .into_iter()
            .map(|(k, v, n)| {
                json!({ "merchant": k, "monthly_average": money(v / count), "transactions": n })
            })
            .collect::<Vec<_>>(),
        "rising_last_month": {
            "month": last_month.format("%Y-%m").to_string(),
            "categories": rising
                .into_iter()
                .take(5)
                .map(|(k, amount, usual)| {
                    json!({
                        "category": k,
                        "last_month": money(amount),
                        "usual_month": money(usual),
                        "increase": money(amount - usual),
                    })
                })
                .collect::<Vec<_>>(),
        },
        "recurring_payments": recurring.into_iter().take(15).collect::<Vec<_>>(),
        "recurring_yearly_total": money(recurring_total),
        "overspent_budgets": overspent,
    }))
}

/// Above this many transactions the digest lists merchants instead, to keep it a sensible
/// size for a model's context (a transaction row is roughly 25 tokens).
pub const DIGEST_MAX_TRANSACTIONS: usize = 1000;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    #[default]
    Auto,
    Transactions,
    Merchants,
}

struct DigestRow {
    date: NaiveDate,
    description: String,
    amount: Decimal,
    category: String,
    account: String,
    status: String,
}

fn to_csv(header: &[&str], rows: Vec<Vec<String>>) -> String {
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer.write_record(header).expect("writing to memory");
    for row in rows {
        writer.write_record(row).expect("writing to memory");
    }
    String::from_utf8(writer.into_inner().expect("writing to memory")).expect("UTF-8 in, UTF-8 out")
}

/// Every categorised transaction from `start` to `end` as CSV, or one row per merchant when
/// there are more than `max` (with `Detail::Auto`). Transfers between the user's own
/// accounts are left out.
pub async fn transactions_digest(
    db: &SqlitePool,
    start: NaiveDate,
    end: NaiveDate,
    detail: Detail,
    max: usize,
) -> Result<Value, ReportError> {
    if end < start {
        return Err(ReportError::Invalid(
            "end_date must be on or after start_date.".into(),
        ));
    }
    type Leg = (i64, NaiveDate, String, String, String, String, Money);
    let legs: Vec<Leg> = sqlx::query_as(
        "SELECT t.id, t.date, t.description, t.status, a.full_path, a.type, p.amount
         FROM transactions t
         JOIN postings p ON p.transaction_id = t.id
         JOIN accounts a ON a.id = p.account_id
         WHERE t.date >= ? AND t.date <= ?
         ORDER BY t.date, t.id, p.id",
    )
    .bind(start)
    .bind(end)
    .fetch_all(db)
    .await?;

    let (mut spending, mut income, mut unreviewed) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    let mut rows: Vec<DigestRow> = Vec::new();
    let mut i = 0;
    while i < legs.len() {
        let id = legs[i].0;
        let txn: Vec<&Leg> = legs[i..].iter().take_while(|l| l.0 == id).collect();
        i += txn.len();
        let is = |kinds: &[&str], leg: &&&Leg| kinds.contains(&leg.5.as_str());
        let categories: Vec<&&Leg> = txn
            .iter()
            .filter(|l| is(&["expense", "income"], l))
            .collect();
        if categories.is_empty() {
            continue;
        }
        let paid_with: Vec<&&Leg> = txn
            .iter()
            .filter(|l| is(&["asset", "liability"], l))
            .collect();
        let (_, date, description, status, ..) = txn[0];
        for leg in &categories {
            let amount = decimal(leg.6);
            if leg.5 == "expense" {
                spending += amount;
                if status == "pending" {
                    unreviewed += amount;
                }
            } else {
                income -= amount;
            }
        }
        let join = |legs: &[&&Leg]| {
            legs.iter()
                .map(|l| l.4.as_str())
                .collect::<Vec<_>>()
                .join(";")
        };
        rows.push(DigestRow {
            date: *date,
            description: description.clone(),
            amount: categories.iter().map(|l| decimal(l.6)).sum(),
            category: join(&categories),
            account: join(&paid_with),
            status: status.clone(),
        });
    }

    let detail = match detail {
        Detail::Auto if rows.len() <= max => Detail::Transactions,
        Detail::Auto => Detail::Merchants,
        chosen => chosen,
    };
    let csv = if detail == Detail::Transactions {
        to_csv(
            &[
                "date",
                "description",
                "amount",
                "category",
                "account",
                "status",
            ],
            rows.iter()
                .map(|r| {
                    vec![
                        r.date.to_string(),
                        r.description.clone(),
                        money(r.amount),
                        r.category.clone(),
                        r.account.clone(),
                        r.status.clone(),
                    ]
                })
                .collect(),
        )
    } else {
        let mut by_merchant: Vec<(String, Vec<&DigestRow>)> = Vec::new();
        for row in &rows {
            let key = merchant_key(&row.description);
            match by_merchant.iter_mut().find(|(k, _)| *k == key) {
                Some((_, group)) => group.push(row),
                None => by_merchant.push((key, vec![row])),
            }
        }
        let total = |group: &[&DigestRow]| group.iter().map(|r| r.amount).sum::<Decimal>();
        // Stable, so merchants with the same total stay in the order they first appear.
        by_merchant.sort_by_cached_key(|(_, group)| std::cmp::Reverse(total(group)));
        to_csv(
            &[
                "merchant",
                "category",
                "transactions",
                "total",
                "typical_amount",
                "first_date",
                "last_date",
            ],
            by_merchant
                .iter()
                .map(|(_, group)| {
                    let last = group.last().expect("groups aren't empty");
                    let amounts: Vec<Decimal> = group.iter().map(|r| r.amount).collect();
                    vec![
                        last.description.clone(),
                        most_common(group.iter().map(|r| r.category.as_str())),
                        group.len().to_string(),
                        money(total(group)),
                        money(median(&amounts)),
                        group[0].date.to_string(),
                        last.date.to_string(),
                    ]
                })
                .collect(),
        )
    };
    Ok(json!({
        "start_date": start,
        "end_date": end,
        "detail": detail,
        "transactions": rows.len(),
        "total_spending": money(spending),
        "total_income": money(income),
        "unreviewed": money(unreviewed),
        "csv": csv,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merchant_keys_drop_references() {
        assert_eq!(merchant_key("NETFLIX.COM 0988"), "netflix com");
        assert_eq!(merchant_key("NETFLIX.COM 1288"), "netflix com");
        assert_eq!(merchant_key("AMAZON MKTPLACE*AB12CD"), "amazon mktplace");
        assert_eq!(merchant_key("12345"), "12345");
        assert_eq!(merchant_key("M&S SIMPLY FOOD"), "m&s simply food");
    }

    #[test]
    fn medians_and_most_common() {
        let d = |s: &str| s.parse::<Decimal>().unwrap();
        assert_eq!(median(&[d("3"), d("1"), d("2")]), d("2"));
        assert_eq!(median(&[d("1"), d("2")]), d("1.5"));
        assert_eq!(most_common(["a", "b", "b", "a", "c"]), "a");
        assert_eq!(money(d("1.005")), "1.00");
    }
}
