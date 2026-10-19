//! `/api/export`: the ledger in the installed exporters' formats, and a copy of the database.

use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pennychest_core::export::{self, ExporterInfo};
use serde::Serialize;

use crate::AppState;
use crate::error::{ApiError, ApiResult, PathParam};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(info))
        // Before /{name}, so it isn't taken for an exporter's name.
        .route("/database", get(database))
        .route("/{name}", get(download))
}

#[derive(Serialize)]
struct ExportInfo {
    engine: &'static str,
    database_backup: bool,
    size_bytes: Option<i64>,
    exporters: Vec<ExporterInfo>,
}

async fn info(State(state): State<AppState>) -> ApiResult<Json<ExportInfo>> {
    Ok(Json(ExportInfo {
        engine: "sqlite",
        database_backup: true,
        size_bytes: Some(export::database_size(&state.db).await?),
        exporters: state.exporters().list(),
    }))
}

fn attachment(body: Vec<u8>, media_type: &str, filename: &str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(media_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

/// A consistent copy of the whole database, safe to take while the app is in use.
async fn database(State(state): State<AppState>) -> ApiResult<Response> {
    let copy = export::backup(&state.db).await?;
    let today = chrono::Local::now().date_naive();
    Ok(attachment(
        copy,
        "application/vnd.sqlite3",
        &format!("pennychest-backup-{today}.db"),
    ))
}

/// The whole ledger in an installed exporter's format.
async fn download(
    State(state): State<AppState>,
    PathParam(name): PathParam<String>,
) -> ApiResult<Response> {
    let exporter = state
        .exporters()
        .get(&name)
        .ok_or_else(|| ApiError::not_found(format!("Exporter '{name}' isn't installed")))?;
    let today = chrono::Local::now().date_naive();
    let ledger = export::load_ledger(&state.db, today).await?;
    // Exporters run plugins, which block; keep them off the async workers.
    let file = tokio::task::spawn_blocking({
        let exporter = exporter.clone();
        move || exporter.export(&ledger)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::bad_request)?;
    Ok(attachment(
        file,
        exporter.media_type(),
        &format!("pennychest-{today}.{}", exporter.file_extension()),
    ))
}
