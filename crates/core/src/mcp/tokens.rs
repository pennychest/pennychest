//! Access tokens for the MCP server: personal ones made in Settings, and ones issued to OAuth
//! connectors, which expire and are renewed (`client_id` set).

use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::actions::Scope;
use crate::auth::{now, random_token, token_hash};

pub const TOKEN_PREFIX: &str = "pc_";
/// The scopes a token can add on top of read, in the order they're shown.
pub const WRITE_SCOPES: [Scope; 2] = [Scope::Organise, Scope::Transactions];
const NAME_LENGTH: usize = 60;

/// A new access token's value.
pub(crate) fn new_access_token() -> String {
    format!("{TOKEN_PREFIX}{}", random_token(32))
}

/// The start of a token, so the user can tell their tokens apart.
pub(crate) fn prefix(token: &str) -> String {
    token.chars().take(10).collect()
}

/// The write scopes among `scopes`, in order, as stored.
pub(crate) fn write_scopes<S: AsRef<str>>(scopes: &[S]) -> Vec<&'static str> {
    WRITE_SCOPES
        .iter()
        .map(|s| s.name())
        .filter(|name| scopes.iter().any(|s| s.as_ref() == *name))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Token {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub created_at: NaiveDateTime,
    pub last_used_at: Option<NaiveDateTime>,
    /// "connector" for apps that signed in with OAuth (claude.ai, ChatGPT), else "token".
    pub kind: &'static str,
}

#[derive(sqlx::FromRow)]
struct TokenRow {
    id: i64,
    name: String,
    prefix: String,
    scopes: String,
    created_at: NaiveDateTime,
    last_used_at: Option<NaiveDateTime>,
    client_id: Option<String>,
}

impl From<TokenRow> for Token {
    fn from(row: TokenRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            prefix: row.prefix,
            scopes: serde_json::from_str(&row.scopes).unwrap_or_default(),
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            kind: if row.client_id.is_some() {
                "connector"
            } else {
                "token"
            },
        }
    }
}

const COLUMNS: &str = "id, name, prefix, scopes, created_at, last_used_at, client_id";

/// The newest first.
pub async fn list(db: &SqlitePool) -> Result<Vec<Token>, sqlx::Error> {
    let rows: Vec<TokenRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM access_tokens ORDER BY created_at DESC, id DESC"
    ))
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Token::from).collect())
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Give the token a name.")]
    NoName,
    #[error("Unknown permissions: {0}")]
    UnknownScopes(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A personal access token, and its value, which is shown only now.
pub async fn create(
    db: &SqlitePool,
    name: &str,
    scopes: &[String],
) -> Result<(Token, String), TokenError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TokenError::NoName);
    }
    let mut unknown: Vec<&str> = scopes
        .iter()
        .map(String::as_str)
        .filter(|s| !WRITE_SCOPES.iter().any(|w| w.name() == *s))
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        unknown.dedup();
        let listed: Vec<String> = unknown.iter().map(|s| format!("'{s}'")).collect();
        return Err(TokenError::UnknownScopes(format!(
            "[{}]",
            listed.join(", ")
        )));
    }
    let raw = new_access_token();
    let name: String = name.chars().take(NAME_LENGTH).collect();
    let row: TokenRow = sqlx::query_as(&format!(
        "INSERT INTO access_tokens (name, token_hash, prefix, scopes) VALUES (?, ?, ?, ?)
         RETURNING {COLUMNS}"
    ))
    .bind(name)
    .bind(token_hash(&raw))
    .bind(prefix(&raw))
    .bind(serde_json::to_string(&write_scopes(scopes)).expect("JSON serialises"))
    .fetch_one(db)
    .await?;
    Ok((row.into(), raw))
}

/// Whether there was one to delete. Deleting a connector's token disconnects it.
pub async fn delete(db: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query("DELETE FROM access_tokens WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(deleted.rows_affected() > 0)
}

/// What a valid bearer token allows.
#[derive(Debug, Clone, PartialEq)]
pub struct Bearer {
    pub id: i64,
    pub scopes: Vec<Scope>,
}

/// The token this bearer value belongs to, noting that it was used. An OAuth access token past
/// its hour is refused; the connector refreshes it.
pub async fn find(db: &SqlitePool, raw: &str) -> Result<Option<Bearer>, sqlx::Error> {
    if !raw.starts_with(TOKEN_PREFIX) {
        return Ok(None);
    }
    let found: Option<(i64, String, Option<i64>)> =
        sqlx::query_as("SELECT id, scopes, expires_at FROM access_tokens WHERE token_hash = ?")
            .bind(token_hash(raw))
            .fetch_optional(db)
            .await?;
    let Some((id, scopes, expires_at)) = found else {
        return Ok(None);
    };
    if expires_at.is_some_and(|at| at < now()) {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE access_tokens SET last_used_at = strftime('%Y-%m-%d %H:%M:%f', 'now')
         WHERE id = ?",
    )
    .bind(id)
    .execute(db)
    .await?;
    let names: Vec<String> = serde_json::from_str(&scopes).unwrap_or_default();
    Ok(Some(Bearer {
        id,
        scopes: names.iter().filter_map(|n| n.parse().ok()).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn tokens_are_stored_hashed_and_found_by_value() {
        let db = db::open_in_memory().await.unwrap();
        let scopes = vec!["transactions".to_owned(), "organise".to_owned()];
        let (token, raw) = create(&db, "  Claude Code ", &scopes).await.unwrap();
        assert!(raw.starts_with("pc_") && raw.len() == 46);
        assert_eq!(token.name, "Claude Code");
        assert_eq!(token.prefix, raw[..10]);
        assert_eq!(token.scopes, ["organise", "transactions"]);
        let stored: String = sqlx::query_scalar("SELECT token_hash FROM access_tokens")
            .fetch_one(&db)
            .await
            .unwrap();
        assert!(!stored.contains(&raw));

        let bearer = find(&db, &raw).await.unwrap().unwrap();
        assert_eq!(bearer.scopes, [Scope::Organise, Scope::Transactions]);
        assert!(list(&db).await.unwrap()[0].last_used_at.is_some());
        assert_eq!(find(&db, "pc_wrong").await.unwrap(), None);
        assert_eq!(find(&db, "nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn names_and_scopes_are_checked() {
        let db = db::open_in_memory().await.unwrap();
        assert!(matches!(
            create(&db, " ", &[]).await,
            Err(TokenError::NoName)
        ));
        let unknown = create(&db, "x", &["admin".into(), "read".into()]).await;
        assert_eq!(
            unknown.unwrap_err().to_string(),
            "Unknown permissions: ['admin', 'read']"
        );
    }
}
