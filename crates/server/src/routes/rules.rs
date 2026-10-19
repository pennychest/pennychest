//! `/api/rules`: categorisation rules.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use pennychest_core::rules::{
    self, ApplyPreview, ExportedRule, Imported, NewRule, Preview, Rule, RuleError, RuleUpdate,
    TestResult,
};
use serde::Deserialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody, PathParam};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/export", get(export))
        .route("/import", post(import))
        .route("/preview", post(preview))
        .route("/test", post(test))
        .route("/apply-preview", post(apply_preview))
        .route("/{id}", get(one).put(update).delete(delete))
}

async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<Rule>>> {
    Ok(Json(rules::list(&mut *state.db.acquire().await?).await?))
}

async fn one(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Rule>> {
    let rule = rules::get(&mut *state.db.acquire().await?, id).await?;
    rule.map(Json).ok_or_else(|| RuleError::NotFound.into())
}

async fn create(
    State(state): State<AppState>,
    JsonBody(new): JsonBody<NewRule>,
) -> ApiResult<(StatusCode, Json<Rule>)> {
    Ok((
        StatusCode::CREATED,
        Json(rules::create(&state.db, new).await?),
    ))
}

async fn update(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    JsonBody(changes): JsonBody<RuleUpdate>,
) -> ApiResult<Json<Rule>> {
    Ok(Json(rules::update(&state.db, id, changes).await?))
}

async fn delete(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    rules::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct PreviewBody {
    pattern: String,
    match_type_id: i64,
    #[serde(default)]
    exclude_rule_id: Option<i64>,
}

/// Which recent transactions a pattern would match, and which rules it overlaps with.
async fn preview(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<PreviewBody>,
) -> ApiResult<Json<Preview>> {
    Ok(Json(
        rules::preview(
            &state.db,
            &body.pattern,
            body.match_type_id,
            body.exclude_rule_id,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
struct TestBody {
    pattern: String,
    match_type_id: i64,
    descriptions: Vec<String>,
}

/// Which of the descriptions given a pattern would match.
async fn test(JsonBody(body): JsonBody<TestBody>) -> Json<TestResult> {
    Json(rules::test(
        &body.pattern,
        body.match_type_id,
        body.descriptions,
    ))
}

/// What re-applying every rule would change.
async fn apply_preview(State(state): State<AppState>) -> ApiResult<Json<ApplyPreview>> {
    Ok(Json(rules::apply_preview(&state.db).await?))
}

async fn export(State(state): State<AppState>) -> ApiResult<Json<Vec<ExportedRule>>> {
    Ok(Json(rules::export(&state.db).await?))
}

/// Adds the rules given to the ones there are.
async fn import(
    State(state): State<AppState>,
    JsonBody(exported): JsonBody<Vec<ExportedRule>>,
) -> ApiResult<Json<Imported>> {
    Ok(Json(rules::import(&state.db, exported).await?))
}

impl From<RuleError> for ApiError {
    fn from(err: RuleError) -> Self {
        let message = err.to_string();
        match err {
            RuleError::NotFound => ApiError::not_found(message),
            RuleError::MatchTypeNotFound(_)
            | RuleError::AccountNotFound(_)
            | RuleError::BadRegex(_) => ApiError::bad_request(message),
            RuleError::Database(err) => err.into(),
        }
    }
}
