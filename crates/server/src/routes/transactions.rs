//! `/api/transactions`: the ledger's transactions.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::NaiveDate;
use pennychest_core::Money;
use pennychest_core::ledger::{
    self, Balanced, Filters, LedgerError, NewPosting, NewTransaction, Status, Transaction,
    TransactionUpdate,
};
use pennychest_core::lookups::CategorisationSource;
use pennychest_core::tags;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam, QueryParams};

const MAX_PER_PAGE: u32 = 200;
const STATUS_ERROR: &str = "Status must be 'pending' or 'confirmed'";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/count", get(count))
        .route("/tags", get(list_tags))
        .route("/tags/rename", post(rename_tag))
        .route("/tags/delete", post(delete_tag))
        .route("/bulk-confirm", post(confirm_many))
        .route("/{id}", get(one).put(update).delete(delete))
        .route("/{id}/confirm", post(confirm))
        .route("/{id}/tags", put(set_tags))
}

/// The filters and a page. Spelt out rather than `#[serde(flatten)]` of `Filters`, which
/// can't read numbers from a query string.
#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
    account_id: Option<i64>,
    category_id: Option<i64>,
    batch_id: Option<i64>,
    date_from: Option<NaiveDate>,
    date_to: Option<NaiveDate>,
    search: Option<String>,
    /// Comma-separated: transactions with every one of them.
    tags: Option<String>,
    #[serde(default = "first_page")]
    page: u32,
    #[serde(default = "default_per_page")]
    per_page: u32,
}

fn first_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    50
}

async fn list(
    State(state): State<AppState>,
    QueryParams(query): QueryParams<ListQuery>,
) -> ApiResult<Json<Vec<Transaction>>> {
    if query.page < 1 {
        return Err(ApiError::invalid("page must be 1 or more"));
    }
    if !(1..=MAX_PER_PAGE).contains(&query.per_page) {
        return Err(ApiError::invalid(format!(
            "per_page must be from 1 to {MAX_PER_PAGE}"
        )));
    }
    let filters = Filters {
        status: query.status,
        account_id: query.account_id,
        category_id: query.category_id,
        batch_id: query.batch_id,
        date_from: query.date_from,
        date_to: query.date_to,
        search: query.search,
        tags: query
            .tags
            .as_deref()
            .map(ledger::split_tags)
            .unwrap_or_default(),
    };
    Ok(Json(
        ledger::list(&state.db, &filters, query.page, query.per_page).await?,
    ))
}

#[derive(Serialize)]
struct Count {
    count: i64,
}

async fn count(
    State(state): State<AppState>,
    QueryParams(filters): QueryParams<Filters>,
) -> ApiResult<Json<Count>> {
    Ok(Json(Count {
        count: ledger::count(&state.db, &filters).await?,
    }))
}

async fn one(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Transaction>> {
    ledger::get(&state.db, id)
        .await?
        .map(Json)
        .ok_or_else(|| LedgerError::NotFound.into())
}

#[derive(Deserialize)]
struct PostingBody {
    account_id: i64,
    amount: Money,
    categorised_by_id: i64,
    #[serde(default)]
    rule_id: Option<i64>,
}

#[derive(Deserialize)]
struct CreateBody {
    date: NaiveDate,
    description: String,
    #[serde(default = "pending")]
    status: String,
    postings: Vec<PostingBody>,
}

fn pending() -> String {
    "pending".into()
}

async fn create(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<CreateBody>,
) -> ApiResult<(StatusCode, Json<Transaction>)> {
    let postings = body
        .postings
        .into_iter()
        .map(|p| {
            let categorised_by =
                CategorisationSource::try_from(p.categorised_by_id).map_err(|id| {
                    ApiError::bad_request(format!("Categorisation source {id} not found"))
                })?;
            Ok(NewPosting {
                account_id: p.account_id,
                amount: p.amount,
                categorised_by,
                rule_id: p.rule_id,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let postings = Balanced::new(postings)?;
    let status = Status::parse(&body.status).ok_or_else(|| ApiError::bad_request(STATUS_ERROR))?;
    let new = NewTransaction {
        date: body.date,
        description: body.description,
        status,
        currency: "GBP".into(),
        import_batch_id: None,
        import_metadata: None,
        tags: Vec::new(),
        postings,
    };
    Ok((
        StatusCode::CREATED,
        Json(ledger::create(&state.db, &new).await?),
    ))
}

#[derive(Deserialize)]
struct UpdateBody {
    date: Option<NaiveDate>,
    description: Option<String>,
    status: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    JsonBody(body): JsonBody<UpdateBody>,
) -> ApiResult<Json<Transaction>> {
    let status = body
        .status
        .map(|s| Status::parse(&s).ok_or_else(|| ApiError::bad_request(STATUS_ERROR)))
        .transpose()?;
    let changes = TransactionUpdate {
        date: body.date,
        description: body.description,
        status,
    };
    Ok(Json(ledger::update(&state.db, id, changes).await?))
}

async fn delete(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    ledger::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn confirm(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Transaction>> {
    Ok(Json(ledger::confirm(&state.db, id).await?))
}

#[derive(Serialize)]
struct Confirmed {
    confirmed: u64,
}

/// Confirms the pending ones among the ids given.
async fn confirm_many(
    State(state): State<AppState>,
    JsonBody(ids): JsonBody<Vec<i64>>,
) -> ApiResult<Json<Confirmed>> {
    Ok(Json(Confirmed {
        confirmed: ledger::confirm_many(&state.db, &ids).await?,
    }))
}

/// Every tag in use, with how many transactions have it.
async fn list_tags(State(state): State<AppState>) -> ApiResult<Json<Vec<tags::TagCount>>> {
    Ok(Json(tags::list(&state.db).await?))
}

fn tag_name(name: &str) -> Result<String, ApiError> {
    tags::normalise(name).map_err(|e| ApiError::bad_request(e.to_string()))
}

fn no_tag(name: &str) -> ApiError {
    ApiError::not_found(format!("There is no tag '{name}'"))
}

#[derive(Deserialize)]
struct RenameTag {
    from: String,
    to: String,
}

/// Renames a tag on every transaction that has it, merging it into `to` if that's another tag.
async fn rename_tag(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<RenameTag>,
) -> ApiResult<Json<tags::Renamed>> {
    let (from, to) = (tag_name(&body.from)?, tag_name(&body.to)?);
    let mut tx = state.db.begin().await?;
    let renamed = tags::rename(&mut tx, &from, &to)
        .await?
        .ok_or_else(|| no_tag(&from))?;
    tx.commit().await?;
    Ok(Json(renamed))
}

#[derive(Deserialize)]
struct DeleteTag {
    name: String,
}

/// Takes a tag off every transaction that has it.
async fn delete_tag(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<DeleteTag>,
) -> ApiResult<StatusCode> {
    let name = tag_name(&body.name)?;
    let mut tx = state.db.begin().await?;
    tags::delete(&mut tx, &name)
        .await?
        .ok_or_else(|| no_tag(&name))?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Gives the transaction exactly these tags.
async fn set_tags(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    JsonBody(names): JsonBody<Vec<String>>,
) -> ApiResult<Json<Transaction>> {
    let names = tags::normalise_all(&names).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut tx = state.db.begin().await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM transactions WHERE id = ?)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Err(LedgerError::NotFound.into());
    }
    tags::set(&mut tx, id, &names).await?;
    tx.commit().await?;
    one(State(state), PathParam(id)).await
}

impl From<LedgerError> for ApiError {
    fn from(err: LedgerError) -> Self {
        let message = err.to_string();
        match err {
            LedgerError::NotFound | LedgerError::CategoryNotFound => ApiError::not_found(message),
            LedgerError::Unbalanced(_)
            | LedgerError::TooFewPostings
            | LedgerError::AccountNotFound(_)
            | LedgerError::RuleNotFound(_) => ApiError::bad_request(message),
            LedgerError::Database(err) => err.into(),
        }
    }
}
