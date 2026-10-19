//! `/api/imports`: working out how to read a statement, CSV column mappings, and
//! importing statements and reviewing them.

use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::NaiveDate;
use pennychest_core::ai::config::{self, Task};
use pennychest_core::ai::{csv_columns, insights as ai_insights};
use pennychest_core::imports::batches::{
    self, AccountReview, BatchReview, BatchSummary, ImportError, ImportOutcome, Upload,
};
use pennychest_core::imports::csv::{self, Columns};
use pennychest_core::imports::matching::{
    self, MarkAsTransfer, MarkedAsTransfer, MatchError, Matched, TransferSuggestion,
};
use pennychest_core::imports::templates::{self, Source, Suggestion, Template, TemplateError};
use pennychest_core::imports::{ImporterInfo, is_csv};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::error::{ApiError, ApiResult, Form, JsonBody, PathParam, QueryParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/detect", post(detect))
        .route("/importers", get(importers))
        .route("/csv/preview", post(preview_csv))
        .route("/csv/templates", get(list_templates).post(save_template))
        .route("/csv/templates/{id}", delete(delete_template))
        .route("/upload", post(upload))
        .route("/batches", get(list_batches))
        .route("/batches/{id}", get(review_batch).delete(delete_batch))
        .route("/batches/{id}/file", get(download_file))
        .route("/batches/{id}/confirm-all", post(confirm_batch))
        .route("/batches/{id}/reapply-rules", post(reapply_rules))
        .route("/batches/{id}/match", post(match_batch))
        .route("/transactions/{id}/categorise", put(categorise))
        .route("/transactions/{id}/not-duplicate", post(not_duplicate))
        .route(
            "/transactions/{id}/transfer-suggestions",
            get(transfer_suggestions),
        )
        .route(
            "/transactions/{id}/mark-as-transfer",
            post(mark_as_transfer),
        )
        .route("/transfers/pending-balance", get(pending_balance))
        .route("/account/{id}", get(account_review))
        .route("/account/{id}/confirm-all", post(confirm_account))
}

async fn importers(State(state): State<AppState>) -> Json<Vec<ImporterInfo>> {
    Json(state.importers().list())
}

#[derive(Serialize)]
struct StatementDetection {
    statement_type: String,
    bank_identifier: String,
    label: String,
    name_hint: Option<String>,
    suggested_account_id: Option<i64>,
    suggested_account_name: Option<String>,
    period_start: Option<NaiveDate>,
    period_end: Option<NaiveDate>,
    /// Signed decimal strings, as the statement wrote them.
    opening_balance: Option<String>,
    closing_balance: Option<String>,
}

#[derive(Serialize)]
struct Detection {
    importer: String,
    /// Only for formats that identify their account, like bank PDF statements.
    statement: Option<StatementDetection>,
}

/// Which importer handles a file and, for statements that identify their account, which
/// account it belongs to. Saves nothing.
async fn detect(State(state): State<AppState>, mut form: Form) -> ApiResult<Json<Detection>> {
    let file = form.require_file()?;
    // Plugins' importers run to recognise a file, which blocks.
    let importers = state.importers();
    let found = tokio::task::spawn_blocking({
        let importers = importers.clone();
        let (content, filename) = (file.content.clone(), file.filename.clone());
        move || importers.find(&content, &filename)
    })
    .await
    .map_err(ApiError::internal)?;
    let Some(importer) = found else {
        if is_csv(&file.filename) && importers.get(csv::NAME).is_some() {
            // A layout the CSV importer doesn't recognise: its columns are mapped first.
            return Ok(Json(Detection {
                importer: csv::NAME.into(),
                statement: None,
            }));
        }
        let name = if file.filename.is_empty() {
            "this file"
        } else {
            &file.filename
        };
        let supported = importers.supported_extensions().join(", ");
        let supported = if supported.is_empty() {
            "none".into()
        } else {
            supported
        };
        return Err(ApiError::invalid(format!(
            "No installed importer handles {name}. Supported file types: {supported}."
        )));
    };

    let content = file.content;
    let reader = importer.clone();
    let info = tokio::task::spawn_blocking(move || reader.statement_info(&content))
        .await
        .map_err(ApiError::internal)?;
    let Some(info) = info else {
        return Ok(Json(Detection {
            importer: importer.name().to_owned(),
            statement: None,
        }));
    };
    let suggested: Option<(i64, String)> =
        sqlx::query_as("SELECT id, full_path FROM accounts WHERE bank_identifier = ? LIMIT 1")
            .bind(&info.bank_identifier)
            .fetch_optional(&state.db)
            .await?;
    Ok(Json(Detection {
        importer: importer.name().to_owned(),
        statement: Some(StatementDetection {
            statement_type: info.statement_type,
            bank_identifier: info.bank_identifier,
            label: info.label,
            name_hint: info.name_hint,
            suggested_account_id: suggested.as_ref().map(|(id, _)| *id),
            suggested_account_name: suggested.map(|(_, path)| path),
            period_start: info.period_start,
            period_end: info.period_end,
            opening_balance: info.opening_balance.map(|b| b.to_string()),
            closing_balance: info.closing_balance.map(|b| b.to_string()),
        }),
    }))
}

fn read_csv(content: &[u8]) -> ApiResult<(Vec<String>, Vec<Vec<String>>)> {
    csv::read(content)
        .map_err(|e| ApiError::bad_request(format!("Couldn't read the CSV file: {e}")))
}

#[derive(Serialize)]
struct TemplateRef {
    id: i64,
    name: String,
}

/// A CSV file's headers and first rows, and which column holds each field: from a saved
/// template, the column names, or the model (straight away if it runs automatically and the
/// names don't say, or when asked with `ai`).
async fn preview_csv(State(state): State<AppState>, mut form: Form) -> ApiResult<Json<Value>> {
    let file = form.require_file()?;
    let ai = form.flag("ai")?;
    let (headers, data) = read_csv(&file.content)?;
    let rows: Vec<Vec<String>> = data.into_iter().take(templates::SAMPLE_ROWS).collect();
    let use_ai = ai
        || (csv_columns::names_leave_gaps(&headers)
            && config::runs_automatically(&state.db, Task::CsvColumns).await?);
    let (columns, template, ai_error) =
        csv_columns::suggest(&state.db, state.http.as_ref(), &headers, &rows, use_ai).await?;
    let columns: Map<String, Value> = columns
        .into_iter()
        .map(|(field, s): (String, Suggestion)| {
            let source = match s.source {
                Source::Template => "template",
                Source::Name => "name",
                Source::Ai => "ai",
            };
            (
                field,
                json!({"column": s.column, "source": source, "confidence": s.confidence}),
            )
        })
        .collect();
    let (provider, model) = config::task_model(&state.db, Task::CsvColumns).await?;
    Ok(Json(json!({
        "headers": headers,
        "rows": rows,
        "template": template.map(|t| TemplateRef { id: t.id, name: t.name }),
        "columns": columns,
        "ai_available": provider.is_some() && model.is_some(),
        "ai_error": ai_error,
    })))
}

async fn list_templates(State(state): State<AppState>) -> ApiResult<Json<Vec<Template>>> {
    Ok(Json(templates::list(&state.db).await?))
}

#[derive(Deserialize)]
struct TemplateBody {
    name: String,
    headers: Vec<String>,
    columns: Columns,
}

/// Saves a column mapping, for later files with the same headers.
async fn save_template(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<TemplateBody>,
) -> ApiResult<(StatusCode, Json<Template>)> {
    let template = templates::save(&state.db, &body.name, &body.headers, body.columns).await?;
    Ok((StatusCode::CREATED, Json(template)))
}

async fn delete_template(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    templates::delete(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Imports a statement into an account as pending transactions.
async fn upload(State(state): State<AppState>, mut form: Form) -> ApiResult<Json<ImportOutcome>> {
    let file = form.require_file()?;
    let account_id = form
        .fields
        .get("account_id")
        .ok_or_else(|| ApiError::invalid("The form must say which account the statement is for."))?
        .trim()
        .parse::<i64>()
        .map_err(|_| ApiError::invalid("account_id must be a number."))?;
    let importer_name = form
        .fields
        .get("importer_name")
        .cloned()
        .unwrap_or_else(|| csv::NAME.into());
    let csv_columns = match form.fields.get("csv_columns") {
        None => None,
        Some(text) => Some(serde_json::from_str::<Columns>(text).map_err(|_| {
            ApiError::bad_request("csv_columns must be a JSON object of column numbers")
        })?),
    };
    let upload = Upload {
        account_id,
        importer_name,
        filename: file.filename,
        content: file.content,
        csv_columns,
    };
    let outcome = batches::import(
        &state.db,
        &state.config.upload_dir,
        &state.importers(),
        &state.learned,
        state.http.as_ref(),
        upload,
    )
    .await?;
    // Scoring charges and labelling subscriptions waits for categories, and the upload
    // shouldn't wait for it.
    if config::runs_automatically(&state.db, Task::Insights).await? {
        let (db, http, batch) = (state.db.clone(), state.http.clone(), outcome.batch_id);
        state.background.spawn(async move {
            if let Err(e) = ai_insights::run_after_import(&db, http.as_ref(), batch).await {
                tracing::warn!("Insights after an import failed: {e}");
            }
        });
    }
    Ok(Json(outcome))
}

async fn list_batches(State(state): State<AppState>) -> ApiResult<Json<Vec<BatchSummary>>> {
    Ok(Json(batches::list(&state.db).await?))
}

async fn review_batch(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<BatchReview>> {
    Ok(Json(batches::review(&state.db, id).await?))
}

/// The statement file an import came from.
async fn download_file(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Response> {
    let (path, name) = batches::stored_file(&state.db, id).await?;
    let content = tokio::fs::read(&path)
        .await
        .map_err(|_| ImportError::FileGone)?;
    let mime = mime_guess::from_path(&name).first_or_octet_stream();
    let disposition = format!(
        "attachment; filename=\"{}\"",
        name.replace(['"', '\\', '\r', '\n'], "_")
    );
    Ok((
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(content),
    )
        .into_response())
}

#[derive(Serialize)]
struct Confirmed {
    confirmed: u64,
}

async fn confirm_batch(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Confirmed>> {
    Ok(Json(Confirmed {
        confirmed: batches::confirm_batch(&state.db, id).await?,
    }))
}

async fn confirm_account(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Confirmed>> {
    Ok(Json(Confirmed {
        confirmed: batches::confirm_account(&state.db, id).await?,
    }))
}

#[derive(Serialize)]
struct Updated {
    updated: u64,
}

/// Re-applies rules to an import, leaving what was categorised by hand.
async fn reapply_rules(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Updated>> {
    Ok(Json(Updated {
        updated: batches::reapply_rules(&state.db, id).await?,
    }))
}

#[derive(Deserialize)]
struct CategoriseQuery {
    target_account_id: i64,
}

/// Files a transaction under a category by hand.
async fn categorise(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    QueryParams(query): QueryParams<CategoriseQuery>,
) -> ApiResult<Json<Value>> {
    batches::categorise(&state.db, id, query.target_account_id).await?;
    Ok(Json(json!({"status": "ok", "transaction_id": id})))
}

/// Keeps a transaction that was flagged as a possible duplicate.
async fn not_duplicate(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<StatusCode> {
    batches::not_duplicate(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn account_review(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<AccountReview>> {
    match batches::account_review(&state.db, id).await {
        Ok(review) => Ok(Json(review)),
        Err(ImportError::AccountNotFound) => Err(ApiError::not_found("Account not found")),
        Err(err) => Err(err.into()),
    }
}

async fn delete_batch(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Value>> {
    batches::delete(&state.db, id).await?;
    Ok(Json(json!({"deleted": true})))
}

impl From<ImportError> for ApiError {
    fn from(err: ImportError) -> Self {
        let message = err.to_string();
        match err {
            ImportError::AlreadyImported(_) => ApiError::new(StatusCode::CONFLICT, message),
            ImportError::BatchNotFound
            | ImportError::TransactionNotFound
            | ImportError::FileNotFound
            | ImportError::FileGone => ApiError::not_found(message),
            ImportError::AccountNotFound
            | ImportError::ImporterMissing(_)
            | ImportError::Unreadable(_)
            | ImportError::Empty
            | ImportError::TargetNotFound
            | ImportError::NothingToCategorise => ApiError::bad_request(message),
            ImportError::Save(err) => ApiError::internal(err),
            ImportError::Database(err) => err.into(),
        }
    }
}

impl From<TemplateError> for ApiError {
    fn from(err: TemplateError) -> Self {
        let message = err.to_string();
        match err {
            TemplateError::NotFound => ApiError::not_found(message),
            TemplateError::Database(err) => err.into(),
            _ => ApiError::bad_request(message),
        }
    }
}

impl From<MatchError> for ApiError {
    fn from(err: MatchError) -> Self {
        let message = err.to_string();
        match err {
            MatchError::BatchNotFound | MatchError::NotFound | MatchError::PeerNotFound => {
                Self::not_found(message)
            }
            MatchError::PeerTaken => Self::new(StatusCode::CONFLICT, message),
            MatchError::NoCategoryPosting | MatchError::Model(_) => Self::bad_request(message),
            MatchError::NoPendingAccount => Self::new(StatusCode::INTERNAL_SERVER_ERROR, message),
            MatchError::Database(e) => Self::internal(e),
        }
    }
}

/// Asks the model about an import's duplicates and transfers, when matching runs on demand.
async fn match_batch(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Matched>> {
    Ok(Json(
        matching::match_batch(&state.db, state.http.as_ref(), id).await?,
    ))
}

async fn transfer_suggestions(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
) -> ApiResult<Json<Vec<TransferSuggestion>>> {
    Ok(Json(matching::transfer_suggestions(&state.db, id).await?))
}

async fn mark_as_transfer(
    State(state): State<AppState>,
    PathParam(id): PathParam<i64>,
    JsonBody(request): JsonBody<MarkAsTransfer>,
) -> ApiResult<Json<MarkedAsTransfer>> {
    Ok(Json(
        matching::mark_as_transfer(&state.db, id, request).await?,
    ))
}

async fn pending_balance(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(matching::pending_balance(&state.db).await?))
}
