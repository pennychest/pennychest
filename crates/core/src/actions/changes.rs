//! The log of changes made through actions (and by categorising imports), each with what's
//! needed to undo it. Settings > Changes made by AI lists them.

use chrono::NaiveDateTime;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;

use super::Source;
use super::undo::{self, UndoError};

pub struct NewChange<'a> {
    pub action: &'a str,
    pub arguments: Option<&'a Value>,
    pub summary: &'a str,
    pub undo: &'a Value,
    pub source: Source,
    pub conversation_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct ChangeRecord {
    pub id: i64,
    pub created_at: NaiveDateTime,
    pub action: String,
    pub summary: String,
    pub source: Source,
    pub conversation_id: Option<i64>,
    pub undone_at: Option<NaiveDateTime>,
}

const COLUMNS: &str = "id, created_at, action, summary, source, conversation_id, undone_at";

pub async fn record(
    db: impl sqlx::SqliteExecutor<'_>,
    change: &NewChange<'_>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO action_changes (action, arguments, summary, undo, source, conversation_id)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(change.action)
    .bind(change.arguments.map(Value::to_string))
    .bind(change.summary)
    .bind(change.undo.to_string())
    .bind(change.source)
    .bind(change.conversation_id)
    .fetch_one(db)
    .await
}

/// The most recent changes first, and how many there are in all.
pub async fn list(
    db: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<ChangeRecord>), sqlx::Error> {
    let total = sqlx::query_scalar("SELECT COUNT(*) FROM action_changes")
        .fetch_one(db)
        .await?;
    let changes = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM action_changes ORDER BY id DESC LIMIT ? OFFSET ?"
    ))
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;
    Ok((total, changes))
}

pub async fn get(db: &SqlitePool, id: i64) -> Result<Option<ChangeRecord>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM action_changes WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Reverses a change, if the data still looks the way the change left it. A refusal says why
/// and changes nothing.
pub async fn undo(db: &SqlitePool, id: i64) -> Result<ChangeRecord, UndoError> {
    let (action, data, undone): (String, String, Option<NaiveDateTime>) =
        sqlx::query_as("SELECT action, undo, undone_at FROM action_changes WHERE id = ?")
            .bind(id)
            .fetch_optional(db)
            .await?
            .ok_or(UndoError::NotFound)?;
    if undone.is_some() {
        return Err(UndoError::Refused(
            "This change has already been undone.".into(),
        ));
    }
    let data: Value = serde_json::from_str(&data)
        .map_err(|_| UndoError::Refused("This change's undo record is damaged.".into()))?;
    let mut tx = db.begin().await?;
    undo::reverse(&mut tx, &action, data).await?;
    sqlx::query("UPDATE action_changes SET undone_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(get(db, id).await?.expect("the change was just read"))
}
