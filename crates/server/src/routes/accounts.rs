//! `/api/accounts`: the chart of accounts.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use pennychest_core::Money;
use pennychest_core::accounts::{
    self, Account, AccountError, AccountType, AccountUpdate, ExportedAccount, NewAccount,
    OpeningBalance,
};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/export", get(export))
        .route("/import", post(import))
        .route("/tree/hierarchy", get(tree))
        .route("/{id}", get(one).put(update).delete(delete))
        .route(
            "/{id}/opening-balance",
            get(opening_balance).put(set_opening_balance),
        )
}

async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<Account>>> {
    Ok(Json(accounts::list(&state.db).await?))
}

#[derive(Deserialize)]
struct TreeQuery {
    account_type: Option<AccountType>,
}

/// The accounts in path order, optionally of one type.
async fn tree(
    State(state): State<AppState>,
    Query(query): Query<TreeQuery>,
) -> ApiResult<Json<Vec<Account>>> {
    Ok(Json(match query.account_type {
        Some(kind) => accounts::list_of_type(&state.db, kind).await?,
        None => accounts::list(&state.db).await?,
    }))
}

async fn one(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Account>> {
    let mut conn = state.db.acquire().await?;
    let account = accounts::get(&mut conn, id).await?;
    account.map(Json).ok_or(AccountError::NotFound.into())
}

async fn create(
    State(state): State<AppState>,
    JsonBody(new): JsonBody<NewAccount>,
) -> ApiResult<(StatusCode, Json<Account>)> {
    Ok((
        StatusCode::CREATED,
        Json(accounts::create(&state.db, new).await?),
    ))
}

async fn update(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    JsonBody(changes): JsonBody<AccountUpdate>,
) -> ApiResult<Json<Account>> {
    Ok(Json(accounts::update(&state.db, id, changes).await?))
}

async fn delete(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    accounts::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The account's opening balance, or null if it has none.
async fn opening_balance(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Option<OpeningBalance>>> {
    Ok(Json(accounts::opening_balance(&state.db, id).await?))
}

#[derive(Deserialize)]
struct OpeningBalanceUpdate {
    /// On the account's side: positive for money in it, negative for a balance owed.
    amount: Money,
    date: NaiveDate,
}

async fn set_opening_balance(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    JsonBody(body): JsonBody<OpeningBalanceUpdate>,
) -> ApiResult<Json<OpeningBalance>> {
    Ok(Json(
        accounts::set_opening_balance(&state.db, id, body.amount, body.date).await?,
    ))
}

async fn export(State(state): State<AppState>) -> ApiResult<Json<Vec<ExportedAccount>>> {
    Ok(Json(accounts::export(&state.db).await?))
}

#[derive(Serialize)]
struct Imported {
    accounts_created: usize,
}

async fn import(
    State(state): State<AppState>,
    JsonBody(exported): JsonBody<Vec<ExportedAccount>>,
) -> ApiResult<Json<Imported>> {
    Ok(Json(Imported {
        accounts_created: accounts::import(&state.db, exported).await?,
    }))
}

impl From<AccountError> for ApiError {
    fn from(err: AccountError) -> Self {
        let message = err.to_string();
        match err {
            AccountError::NotFound => ApiError::not_found(message),
            AccountError::PathTaken => ApiError::new(StatusCode::CONFLICT, message),
            AccountError::ParentNotFound
            | AccountError::ParentIsDescendant
            | AccountError::HasChildren
            | AccountError::InUse(_)
            | AccountError::NoOpeningBalance
            | AccountError::NotEmpty => ApiError::bad_request(message),
            // The seed is missing a required account: shown as it is, since the user can fix it.
            AccountError::MissingOpeningBalances => {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message)
            }
            AccountError::Database(err) => err.into(),
        }
    }
}
