//! `/api/budgets`: spending limits per category, with what's been spent against them.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use chrono::{Local, NaiveDate};
use pennychest_core::Money;
use pennychest_core::budgets::{self, Budget, BudgetError, Period, Suggestion};
use serde::Deserialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam, QueryParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/suggestion", get(suggestion))
        .route("/{id}", patch(update).delete(delete))
}

impl From<BudgetError> for ApiError {
    fn from(err: BudgetError) -> Self {
        let status = match err {
            BudgetError::CategoryNotFound | BudgetError::NotFound => StatusCode::NOT_FOUND,
            BudgetError::NotSpending => StatusCode::BAD_REQUEST,
            BudgetError::Taken => StatusCode::CONFLICT,
            BudgetError::NotPositive => StatusCode::UNPROCESSABLE_ENTITY,
            BudgetError::Database(err) => return Self::internal(err),
        };
        Self::new(status, err.to_string())
    }
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

#[derive(Deserialize)]
struct On {
    /// Any date in the period to report on.
    on: Option<NaiveDate>,
}

async fn list(
    State(state): State<AppState>,
    QueryParams(query): QueryParams<On>,
) -> ApiResult<Json<Vec<Budget>>> {
    let on = query.on.unwrap_or_else(today);
    Ok(Json(budgets::list(&state.db, on).await?))
}

#[derive(Deserialize)]
struct SuggestionQuery {
    account_id: i64,
    #[serde(default)]
    period: Period,
    on: Option<NaiveDate>,
}

/// What this category has cost recently, as a starting point for a budget.
async fn suggestion(
    State(state): State<AppState>,
    QueryParams(query): QueryParams<SuggestionQuery>,
) -> ApiResult<Json<Suggestion>> {
    let on = query.on.unwrap_or_else(today);
    Ok(Json(
        budgets::suggest(&state.db, query.account_id, query.period, on).await?,
    ))
}

#[derive(Deserialize)]
struct NewBudget {
    account_id: i64,
    amount: Money,
    #[serde(default)]
    period: Period,
}

async fn create(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<NewBudget>,
) -> ApiResult<(StatusCode, Json<Budget>)> {
    let budget = budgets::create(
        &state.db,
        body.account_id,
        body.amount,
        body.period,
        today(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(budget)))
}

#[derive(Deserialize)]
struct BudgetUpdate {
    amount: Option<Money>,
    period: Option<Period>,
}

async fn update(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    JsonBody(body): JsonBody<BudgetUpdate>,
) -> ApiResult<Json<Budget>> {
    Ok(Json(
        budgets::update(&state.db, id, body.amount, body.period, today()).await?,
    ))
}

async fn delete(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    budgets::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
