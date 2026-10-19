//! Opening the SQLite database and bringing its schema up to date.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use sqlx::migrate::MigrateError;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("couldn't open the database at {path}: {source}")]
    Open { path: String, source: sqlx::Error },
    #[error("couldn't bring the database up to date: {0}")]
    Migrate(#[from] MigrateError),
}

/// Opens (creating it if need be) the database at `path`, and migrates it.
pub async fn open(path: &Path) -> Result<SqlitePool, OpenError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        // Readers don't block the writer, and a crash can't corrupt the file.
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(configure(options))
        .await
        .map_err(|source| OpenError::Open {
            path: path.display().to_string(),
            source,
        })?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// A new, empty, migrated database in memory, for tests.
pub async fn open_in_memory() -> Result<SqlitePool, OpenError> {
    let options = SqliteConnectOptions::from_str("sqlite::memory:").expect("valid URL");
    // Each connection to :memory: is its own database, so keep to one.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(configure(options))
        .await
        .map_err(|source| OpenError::Open {
            path: ":memory:".into(),
            source,
        })?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

fn configure(options: SqliteConnectOptions) -> SqliteConnectOptions {
    options
        .foreign_keys(true)
        // Wait for another connection's write rather than failing straight away.
        .busy_timeout(Duration::from_secs(5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Money;

    async fn db() -> SqlitePool {
        open_in_memory().await.unwrap()
    }

    async fn names(pool: &SqlitePool, table: &str) -> Vec<String> {
        sqlx::query_scalar(&format!("SELECT name FROM {table} ORDER BY id"))
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn creates_every_table() {
        let pool = db().await;
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_schema WHERE type = 'table'
             AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx_%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            tables,
            [
                "access_tokens",
                "accounts",
                "action_changes",
                "ai_request_logs",
                "app_settings",
                "auth_sessions",
                "budget_periods",
                "budgets",
                "card_taps",
                "categorisation_sources",
                "chat_conversations",
                "chat_messages",
                "csv_templates",
                "import_batches",
                "import_source_types",
                "match_types",
                "merchant_labels",
                "oauth_clients",
                "oauth_codes",
                "postings",
                "raw_import_rows",
                "rules",
                "transactions",
                "wallet_cards",
            ]
        );
    }

    #[tokio::test]
    async fn seeds_lookups_with_fixed_ids() {
        let pool = db().await;
        assert_eq!(names(&pool, "budget_periods").await, ["monthly", "annual"]);
        assert_eq!(
            names(&pool, "match_types").await,
            ["substring", "prefix", "regex"]
        );
        assert_eq!(
            names(&pool, "categorisation_sources").await,
            [
                "rule",
                "manual",
                "import_default",
                "ai",
                "transfer",
                "learned"
            ]
        );
        assert_eq!(names(&pool, "import_source_types").await, ["file", "api"]);
    }

    /// Two accounts and a transaction between them, returning the transaction's id.
    async fn transaction(pool: &SqlitePool, amounts: &[Money]) -> i64 {
        sqlx::query(
            "INSERT OR IGNORE INTO accounts (id, name, full_path, type)
             VALUES (1, 'Current', 'Assets:Current', 'asset'),
                    (2, 'Food', 'Expenses:Food', 'expense')",
        )
        .execute(pool)
        .await
        .unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO transactions (date, description) VALUES ('2026-10-01', 'x') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        for (i, amount) in amounts.iter().enumerate() {
            sqlx::query(
                "INSERT INTO postings (transaction_id, account_id, amount, categorised_by_id)
                 VALUES (?, ?, ?, 2)",
            )
            .bind(id)
            .bind(1 + (i as i64 % 2))
            .bind(amount)
            .execute(pool)
            .await
            .unwrap();
        }
        id
    }

    #[tokio::test]
    async fn sums_money_exactly_in_sql() {
        let pool = db().await;
        let m = |s: &str| s.parse::<Money>().unwrap();
        transaction(&pool, &[m("0.1"), m("0.2"), m("-0.3")]).await;
        let positive: Money =
            sqlx::query_scalar("SELECT SUM(amount) FROM postings WHERE amount > 0")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(positive, m("0.3"));
        let total: Money = sqlx::query_scalar("SELECT SUM(amount) FROM postings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(total.is_zero());
    }

    #[tokio::test]
    async fn refuses_a_floating_point_amount() {
        let pool = db().await;
        let id = transaction(&pool, &[]).await;
        let result = sqlx::query(
            "INSERT INTO postings (transaction_id, account_id, amount, categorised_by_id)
             VALUES (?, 1, 0.1, 2)",
        )
        .bind(id)
        .execute(&pool)
        .await;
        assert!(result.is_err(), "a STRICT table must refuse a REAL amount");
    }

    #[tokio::test]
    async fn enforces_foreign_keys() {
        let pool = db().await;
        let result = sqlx::query(
            "INSERT INTO accounts (name, full_path, parent_id, type)
             VALUES ('Orphan', 'Orphan', 999, 'asset')",
        )
        .execute(&pool)
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn deleting_a_transaction_deletes_its_postings_and_unlinks_it() {
        let pool = db().await;
        let m = |s: &str| s.parse::<Money>().unwrap();
        let first = transaction(&pool, &[m("5"), m("-5")]).await;
        let second = transaction(&pool, &[m("5"), m("-5")]).await;
        sqlx::query(
            "UPDATE transactions SET transfer_peer_id = ?, duplicate_of_id = ? WHERE id = ?",
        )
        .bind(first)
        .bind(first)
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("DELETE FROM transactions WHERE id = ?")
            .bind(first)
            .execute(&pool)
            .await
            .unwrap();

        let postings: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM postings WHERE transaction_id = ?")
                .bind(first)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(postings, 0);
        let links: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT transfer_peer_id, duplicate_of_id FROM transactions WHERE id = ?",
        )
        .bind(second)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(links, (None, None));
    }

    #[tokio::test]
    async fn checks_enumerations_and_json() {
        let pool = db().await;
        let bad = [
            "INSERT INTO accounts (name, full_path, type) VALUES ('A', 'A', 'ASSET')",
            "INSERT INTO transactions (date, description, status) VALUES ('2026-10-01', 'x', 'done')",
            "INSERT INTO transactions (date, description, import_metadata) VALUES ('2026-10-01', 'x', '{oops')",
            "INSERT INTO merchant_labels (merchant_key, kind) VALUES ('NETFLIX', 'treat')",
        ];
        for sql in bad {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
    }

    #[tokio::test]
    async fn opens_a_file_and_reopens_it() {
        let dir = std::env::temp_dir().join(format!("pennychest-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pennychest.db");
        let _ = std::fs::remove_file(&path);

        let pool = open(&path).await.unwrap();
        sqlx::query("INSERT INTO app_settings (key, value) VALUES ('k', 'v')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let pool = open(&path).await.unwrap();
        let value: String = sqlx::query_scalar("SELECT value FROM app_settings WHERE key = 'k'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(value, "v");
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        pool.close().await;
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
