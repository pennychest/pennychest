//! AI: the providers, the language and decision models the tasks share, the tasks that use
//! them (categorising, suggesting rules, insights, reading CSV columns, matching duplicates and
//! transfers, and checking chat actions), and the log of every request made to a model.

pub mod categorise;
pub mod check_actions;
pub mod config;
pub mod csv_columns;
pub mod http;
pub mod insights;
pub mod matching;
pub mod providers;
pub mod rules;

use std::time::Instant;

use chrono::NaiveDateTime;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;

/// One request made to a model, as the AI logs page lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogEntry {
    pub id: i64,
    pub created_at: NaiveDateTime,
    /// e.g. "openai:gpt-5-mini", or "not configured".
    pub provider: String,
    pub operation: String,
    pub request_data: Option<Value>,
    pub response_data: Option<Value>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
}

/// What `log` records.
pub struct NewLog<'a> {
    pub provider: &'a str,
    pub operation: &'a str,
    pub request: Option<&'a Value>,
    pub response: Option<&'a Value>,
    pub duration_ms: i64,
    pub error: Option<&'a str>,
}

pub async fn log(db: &SqlitePool, entry: NewLog<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ai_request_logs
            (provider, operation, request_data, response_data, duration_ms, error)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(entry.provider)
    .bind(entry.operation)
    .bind(entry.request.map(Value::to_string))
    .bind(entry.response.map(Value::to_string))
    .bind(entry.duration_ms)
    .bind(entry.error)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct LogRow {
    id: i64,
    created_at: NaiveDateTime,
    provider: String,
    operation: String,
    request_data: Option<String>,
    response_data: Option<String>,
    duration_ms: Option<i64>,
    error: Option<String>,
}

/// The most recent requests first, and how many there are in all.
pub async fn logs(
    db: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<LogEntry>), sqlx::Error> {
    let total = sqlx::query_scalar("SELECT COUNT(*) FROM ai_request_logs")
        .fetch_one(db)
        .await?;
    let rows: Vec<LogRow> = sqlx::query_as(
        "SELECT id, created_at, provider, operation, request_data, response_data, duration_ms,
                error
         FROM ai_request_logs ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;
    let json = |text: Option<String>| text.and_then(|t| serde_json::from_str(&t).ok());
    Ok((
        total,
        rows.into_iter()
            .map(|r| LogEntry {
                id: r.id,
                created_at: r.created_at,
                provider: r.provider,
                operation: r.operation,
                request_data: json(r.request_data),
                response_data: json(r.response_data),
                duration_ms: r.duration_ms,
                error: r.error,
            })
            .collect(),
    ))
}

/// Logs a model call, from `started` until now. A log that can't be written is only warned
/// about: the work it records shouldn't fail for it.
pub(crate) async fn log_call(
    db: &SqlitePool,
    provider: &str,
    operation: &str,
    started: Instant,
    request: Option<&Value>,
    response: Option<&Value>,
    error: Option<&str>,
) {
    let entry = NewLog {
        provider,
        operation,
        request,
        response,
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        error,
    };
    if let Err(e) = log(db, entry).await {
        tracing::warn!("Couldn't log an AI request: {e}");
    }
}

/// How the logs name the model a task would use, e.g. "openai:gpt-5-mini".
pub async fn task_label(db: &SqlitePool, task: config::Task) -> String {
    match config::task_model(db, task).await {
        Ok((Some(provider), Some(model))) => format!("{provider}:{model}"),
        _ => "not configured".into(),
    }
}

pub(crate) enum ResolveOrFail {
    Ready(providers::Resolved),
    /// What to set up, which has been logged under `operation`.
    Failed(String),
}

/// The model `task` uses, logging why not if it can't run.
pub(crate) async fn resolve(db: &SqlitePool, task: config::Task, operation: &str) -> ResolveOrFail {
    let started = Instant::now();
    let message = match providers::resolve_task(db, task).await {
        Ok(model) => return ResolveOrFail::Ready(model),
        Err(providers::ResolveError::Provider(e)) => e.message,
        Err(providers::ResolveError::Setting(e)) => e.to_string(),
    };
    let label = task_label(db, task).await;
    log_call(db, &label, operation, started, None, None, Some(&message)).await;
    ResolveOrFail::Failed(message)
}
