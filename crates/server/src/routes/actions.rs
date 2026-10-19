//! `/api/actions`: the actions the chat and MCP server use, for a signed-in session, which
//! may run all of them. `/api/changes`: the changes they made, and undoing them.

use axum::body::Bytes;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use pennychest_core::actions::changes::{self, ChangeRecord};
use pennychest_core::actions::{self, ActionError, Caller, Scope, UndoError};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult, PathParam, QueryParams};

pub fn actions_router() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{name}", post(call))
}

pub fn changes_router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_changes))
        .route("/{id}/undo", post(undo))
}

impl From<ActionError> for ApiError {
    fn from(err: ActionError) -> Self {
        match err {
            ActionError::Unknown(_) => Self::not_found(err.to_string()),
            ActionError::Database(err) => Self::internal(err),
            other => Self::bad_request(other.to_string()),
        }
    }
}

impl From<UndoError> for ApiError {
    fn from(err: UndoError) -> Self {
        match err {
            UndoError::NotFound => Self::not_found(err.to_string()),
            UndoError::Refused(message) => Self::bad_request(message),
            UndoError::Database(err) => Self::internal(err),
        }
    }
}

async fn list() -> Json<Value> {
    Json(json!({ "actions": actions::describe(&Scope::ALL) }))
}

/// Runs an action with the arguments in the body, a JSON object (none means `{}`).
async fn call(
    State(state): State<AppState>,
    PathParam(name): PathParam<String>,
    body: Bytes,
) -> ApiResult<Json<Map<String, Value>>> {
    let arguments: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::invalid(format!("The body isn't valid JSON: {e}")))?
    };
    if !arguments.is_object() {
        return Err(ApiError::invalid(
            "The body must be a JSON object of arguments",
        ));
    }
    Ok(Json(
        actions::run(&state.db, &name, arguments, Caller::SESSION).await?,
    ))
}

#[derive(Deserialize)]
struct Page {
    #[serde(default = "fifty")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn fifty() -> i64 {
    50
}

async fn list_changes(
    State(state): State<AppState>,
    QueryParams(page): QueryParams<Page>,
) -> ApiResult<Json<Value>> {
    let (total, changes) =
        changes::list(&state.db, page.limit.clamp(1, 200), page.offset.max(0)).await?;
    Ok(Json(json!({ "total": total, "changes": changes })))
}

async fn undo(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<ChangeRecord>> {
    Ok(Json(changes::undo(&state.db, id).await?))
}
