//! `/api/dashboard`: the dashboard's layout.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use pennychest_core::dashboard::{self, Layout, LayoutError};

use crate::AppState;
use crate::error::{ApiError, ApiResult, JsonBody};

pub fn router() -> Router<AppState> {
    Router::new().route("/layout", get(layout).put(save_layout).delete(reset_layout))
}

async fn layout(State(state): State<AppState>) -> ApiResult<Json<Layout>> {
    Ok(Json(dashboard::layout(&state.db).await?))
}

async fn save_layout(
    State(state): State<AppState>,
    JsonBody(layout): JsonBody<Layout>,
) -> ApiResult<Json<Layout>> {
    Ok(Json(dashboard::save_layout(&state.db, layout).await?))
}

/// Goes back to the default dashboard.
async fn reset_layout(State(state): State<AppState>) -> ApiResult<Json<Layout>> {
    Ok(Json(dashboard::reset_layout(&state.db).await?))
}

impl From<LayoutError> for ApiError {
    fn from(err: LayoutError) -> Self {
        match err {
            LayoutError::Invalid(message) => ApiError::invalid(message),
            LayoutError::Setting(err) => err.into(),
        }
    }
}
