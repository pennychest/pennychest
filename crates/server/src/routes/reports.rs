//! `/api/reports`: net worth, income and spending over a period, and spending by category.
//! The dashboard's charts use the actions, which share their sums with these.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{Local, NaiveDate};
use pennychest_core::{Money, reports};
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiResult, QueryParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/summary", get(summary))
        .route("/expenses-by-category", get(expenses_by_category))
}

#[derive(Deserialize)]
struct Period {
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
    #[serde(default = "expenses")]
    parent_path: String,
}

fn expenses() -> String {
    "Expenses".into()
}

impl Period {
    /// The given dates, or this month so far.
    fn dates(&self) -> (NaiveDate, NaiveDate) {
        let today = Local::now().date_naive();
        (
            self.date_from
                .unwrap_or_else(|| reports::month_start(today)),
            self.date_to.unwrap_or(today),
        )
    }
}

/// These endpoints answer with JSON numbers, as the frontend's types expect. The sums are
/// exact; only the last step to a number isn't.
fn number(amount: Money) -> Value {
    json!(amount.to_decimal().to_f64().unwrap_or_default())
}

async fn summary(
    State(state): State<AppState>,
    QueryParams(period): QueryParams<Period>,
) -> ApiResult<Json<Value>> {
    let (date_from, date_to) = period.dates();
    let summary = reports::summary(&state.db, date_from, date_to).await?;
    let net_worth: Map<String, Value> = summary
        .net_worth
        .into_iter()
        .map(|(currency, amount)| (currency, number(amount)))
        .collect();
    Ok(Json(json!({
        "net_worth": net_worth,
        "income": number(summary.income),
        "expenses": number(summary.expenses),
        "savings_rate": summary.savings_rate.to_f64().unwrap_or_default(),
        "date_from": summary.date_from,
        "date_to": summary.date_to,
    })))
}

async fn expenses_by_category(
    State(state): State<AppState>,
    QueryParams(period): QueryParams<Period>,
) -> ApiResult<Json<Vec<Value>>> {
    let (date_from, date_to) = period.dates();
    let totals =
        reports::expenses_by_category(&state.db, date_from, date_to, &period.parent_path).await?;
    Ok(Json(
        totals
            .into_iter()
            .map(|c| json!({ "full_path": c.full_path, "name": c.name, "total": number(c.total) }))
            .collect(),
    ))
}
