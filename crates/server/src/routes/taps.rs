//! `/api/taps`: card payments the phone's wallet shortcut reports, the token it sends them
//! with, and which wallet card is which account.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use pennychest_core::ai::config::{self, Task};
use pennychest_core::ai::providers::{self, ResolveError};
use pennychest_core::taps::ai::{self as tap_ai, CategoriseError};
use pennychest_core::taps::{self, Card, PairError, Reported, Status, Tap, TapError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam, QueryParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(ingest))
        // Before the /{id} routes, so "token" isn't read as a tap id.
        .route(
            "/token",
            get(read_token).post(new_token).delete(clear_token),
        )
        .route("/cards", get(cards).put(pair))
        .route("/reconcile", post(reconcile))
        .route("/{id}", axum::routing::delete(delete_tap))
        .route("/{id}/categorise", post(categorise))
        .route("/{id}/dismiss", post(dismiss))
        .route("/{id}/restore", post(restore))
}

impl From<TapError> for ApiError {
    fn from(err: TapError) -> Self {
        match err {
            TapError::NotFound => Self::not_found("Tap not found"),
            TapError::Invalid(message) => Self::bad_request(message),
            TapError::Database(e) => Self::internal(e),
        }
    }
}

impl From<PairError> for ApiError {
    fn from(err: PairError) -> Self {
        match err {
            PairError::Database(e) => Self::internal(e),
            other => Self::bad_request(other.to_string()),
        }
    }
}

#[derive(Serialize)]
struct TapToken {
    token: Option<String>,
}

async fn read_token(State(state): State<AppState>) -> ApiResult<Json<TapToken>> {
    Ok(Json(TapToken {
        token: taps::token(&state.db).await.map_err(ApiError::internal)?,
    }))
}

/// A new token, replacing any there was.
async fn new_token(State(state): State<AppState>) -> ApiResult<Json<TapToken>> {
    let token = taps::new_token(&state.db)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(TapToken { token: Some(token) }))
}

async fn clear_token(State(state): State<AppState>) -> ApiResult<StatusCode> {
    taps::clear_token(&state.db)
        .await
        .map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The shortcut's field, which must be text if it's sent at all.
fn text(body: &Map<String, Value>, field: &str) -> ApiResult<Option<String>> {
    match body.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(ApiError::invalid(format!("{field}: must be text"))),
    }
}

/// Records a card payment sent by the phone's wallet shortcut, authenticated with the tap
/// token rather than a session. The phone never waits on a model: categorising the tap, and
/// matching it when that needs the model, happen after the reply.
async fn ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<Map<String, Value>>,
) -> ApiResult<(StatusCode, Json<Tap>)> {
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|a| a.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
        .unwrap_or_default();
    match taps::token_matches(&state.db, given)
        .await
        .map_err(ApiError::internal)?
    {
        None => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Tap ingest is off: create a token in Settings > Card taps",
            ));
        }
        Some(false) => {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Invalid API token"));
        }
        Some(true) => {}
    }
    let loose = |field: &str| -> ApiResult<Value> {
        match body.get(field) {
            None => Ok(Value::Null),
            Some(value @ (Value::Null | Value::String(_) | Value::Number(_))) => Ok(value.clone()),
            Some(_) => Err(ApiError::invalid(format!(
                "{field}: must be text or a number"
            ))),
        }
    };
    let reported = Reported {
        merchant: text(&body, "merchant")?,
        amount: loose("amount")?,
        date: match body.get("date") {
            None | Some(Value::Null) => Value::Null,
            Some(Value::String(text)) => Value::String(text.clone()),
            Some(_) => return Err(ApiError::invalid("date: must be text")),
        },
        card: text(&body, "card")?,
        currency: text(&body, "currency")?,
    };
    let today = chrono::Local::now().date_naive();
    let id = taps::record(&state.db, state.http.as_ref(), reported, today).await?;
    let tap = taps::get(&state.db, &state.learned, id)
        .await?
        .ok_or_else(|| ApiError::internal("a tap just recorded is missing"))?;

    let categorise = config::runs_automatically(&state.db, Task::Taps)
        .await
        .unwrap_or(false);
    let reconcile = tap.transaction_id.is_none()
        && config::runs_automatically(&state.db, Task::Matching)
            .await
            .unwrap_or(false);
    if categorise || reconcile {
        let (db, http, learned) = (state.db.clone(), state.http.clone(), state.learned.clone());
        state.background.spawn(async move {
            // Failures are already in the AI logs.
            if categorise
                && let Err(CategoriseError::Database(e)) =
                    tap_ai::categorise(&db, http.as_ref(), &learned, id).await
            {
                tracing::warn!("Categorising a card tap failed: {e}");
            }
            if reconcile && let Err(e) = taps::reconcile(&db, http.as_ref(), true).await {
                tracing::warn!("Reconciling card taps failed: {e}");
            }
        });
    }
    Ok((StatusCode::CREATED, Json(tap)))
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Showing {
    Unmatched,
    Reconciled,
    Dismissed,
    All,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default = "unmatched")]
    status: Showing,
}

const fn unmatched() -> Showing {
    Showing::Unmatched
}

async fn list(
    State(state): State<AppState>,
    QueryParams(query): QueryParams<ListQuery>,
) -> ApiResult<Json<Vec<Tap>>> {
    let status = match query.status {
        Showing::Unmatched => Some(Status::Unmatched),
        Showing::Reconciled => Some(Status::Reconciled),
        Showing::Dismissed => Some(Status::Dismissed),
        Showing::All => None,
    };
    Ok(Json(taps::list(&state.db, &state.learned, status).await?))
}

async fn shown(state: &AppState, id: i64) -> ApiResult<Json<Tap>> {
    let tap = taps::get(&state.db, &state.learned, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Tap not found"))?;
    Ok(Json(tap))
}

/// Suggests a category for a tap, for when card taps are categorised on demand rather than as
/// each one arrives.
async fn categorise(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Tap>> {
    // A missing tap is a 404 before anything else.
    let _exists = shown(&state, id).await?;
    match providers::resolve_task(&state.db, Task::Taps).await {
        Ok(_) => {}
        Err(ResolveError::Provider(e)) => return Err(ApiError::bad_request(e.message)),
        Err(ResolveError::Setting(e)) => return Err(ApiError::internal(e)),
    }
    match tap_ai::categorise(&state.db, state.http.as_ref(), &state.learned, id).await {
        Ok(()) => {}
        Err(CategoriseError::Model(message)) => return Err(ApiError::bad_request(message)),
        Err(CategoriseError::Database(e)) => return Err(ApiError::internal(e)),
    }
    shown(&state, id).await
}

async fn dismiss(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Tap>> {
    taps::dismiss(&state.db, id).await?;
    shown(&state, id).await
}

async fn restore(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Tap>> {
    taps::restore(&state.db, state.http.as_ref(), id).await?;
    shown(&state, id).await
}

async fn delete_tap(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    taps::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reconcile(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let linked = taps::reconcile(&state.db, state.http.as_ref(), true).await?;
    Ok(Json(json!({ "reconciled": linked })))
}

/// Every card name seen in a tap or paired, with its pairing.
async fn cards(State(state): State<AppState>) -> ApiResult<Json<Vec<Card>>> {
    Ok(Json(taps::cards(&state.db).await?))
}

#[derive(Deserialize)]
struct Pairing {
    card_name: String,
    /// `None` unpairs it.
    account_id: Option<i64>,
}

/// Pairs a wallet card with an asset or liability account, or unpairs it.
async fn pair(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<Pairing>,
) -> ApiResult<Json<Card>> {
    Ok(Json(
        taps::pair(
            &state.db,
            state.http.as_ref(),
            &body.card_name,
            body.account_id,
        )
        .await?,
    ))
}
