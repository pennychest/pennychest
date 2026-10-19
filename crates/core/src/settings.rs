//! App-wide settings, kept in the `app_settings` table as text by key.
//!
//! Each setting is a `Setting<T>` constant, so its key and type are declared once and code
//! can't read or write it as the wrong type. Plain strings are stored as they are (a password
//! hash, a token), structured values as JSON.

use std::borrow::Cow;
use std::marker::PhantomData;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::SqliteExecutor;

/// How a setting's value is written as text.
pub trait Stored: Sized {
    fn encode(&self) -> Result<String, SettingError>;
    fn decode(text: &str) -> Result<Self, SettingError>;
}

/// Plain text, stored as it is.
impl Stored for String {
    fn encode(&self) -> Result<String, SettingError> {
        Ok(self.clone())
    }

    fn decode(text: &str) -> Result<Self, SettingError> {
        Ok(text.to_owned())
    }
}

/// A value stored as JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Json<T>(pub T);

impl<T: Serialize + DeserializeOwned> Stored for Json<T> {
    fn encode(&self) -> Result<String, SettingError> {
        serde_json::to_string(&self.0).map_err(|e| SettingError::Encode(e.to_string()))
    }

    fn decode(text: &str) -> Result<Self, SettingError> {
        serde_json::from_str(text)
            .map(Json)
            .map_err(|e| SettingError::Decode(e.to_string()))
    }
}

pub struct Setting<T> {
    pub key: Cow<'static, str>,
    kind: PhantomData<fn() -> T>,
}

impl<T> Setting<T> {
    pub const fn new(key: &'static str) -> Self {
        Self {
            key: Cow::Borrowed(key),
            kind: PhantomData,
        }
    }

    /// A setting whose key is only known at run time, such as one per AI provider.
    pub fn named(key: String) -> Self {
        Self {
            key: Cow::Owned(key),
            kind: PhantomData,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("a saved setting couldn't be read: {0}")]
    Decode(String),
    #[error("a setting couldn't be saved: {0}")]
    Encode(String),
}

impl<T: Stored> Setting<T> {
    /// The saved value, or `None` if it has never been set.
    pub async fn get(&self, db: impl SqliteExecutor<'_>) -> Result<Option<T>, SettingError> {
        let text: Option<Option<String>> =
            sqlx::query_scalar("SELECT value FROM app_settings WHERE key = ?")
                .bind(self.key.as_ref())
                .fetch_optional(db)
                .await?;
        text.flatten().map(|text| T::decode(&text)).transpose()
    }

    pub async fn set(&self, db: impl SqliteExecutor<'_>, value: &T) -> Result<(), SettingError> {
        sqlx::query(
            "INSERT INTO app_settings (key, value) VALUES (?, ?)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(self.key.as_ref())
        .bind(value.encode()?)
        .execute(db)
        .await?;
        Ok(())
    }

    /// Forgets the setting, so `get` returns `None` again.
    pub async fn clear(&self, db: impl SqliteExecutor<'_>) -> Result<(), SettingError> {
        sqlx::query("DELETE FROM app_settings WHERE key = ?")
            .bind(self.key.as_ref())
            .execute(db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use serde::Deserialize;

    const TOKEN: Setting<String> = Setting::new("test.token");

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Prefs {
        colour: String,
        size: u8,
    }
    const PREFS: Setting<Json<Prefs>> = Setting::new("test.prefs");

    async fn stored(pool: &sqlx::SqlitePool, key: &str) -> Option<String> {
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn strings_are_stored_as_they_are() {
        let pool = db::open_in_memory().await.unwrap();
        assert_eq!(TOKEN.get(&pool).await.unwrap(), None);
        TOKEN.set(&pool, &"abc".into()).await.unwrap();
        assert_eq!(TOKEN.get(&pool).await.unwrap().as_deref(), Some("abc"));
        assert_eq!(stored(&pool, "test.token").await.as_deref(), Some("abc"));
    }

    #[tokio::test]
    async fn json_values_round_trip_and_overwrite() {
        let pool = db::open_in_memory().await.unwrap();
        let prefs = Prefs {
            colour: "green".into(),
            size: 2,
        };
        PREFS.set(&pool, &Json(prefs)).await.unwrap();
        PREFS
            .set(
                &pool,
                &Json(Prefs {
                    colour: "blue".into(),
                    size: 3,
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            stored(&pool, "test.prefs").await.as_deref(),
            Some(r#"{"colour":"blue","size":3}"#)
        );
        assert_eq!(PREFS.get(&pool).await.unwrap().unwrap().0.size, 3);
    }

    #[tokio::test]
    async fn clearing_forgets_the_value() {
        let pool = db::open_in_memory().await.unwrap();
        TOKEN.set(&pool, &"abc".into()).await.unwrap();
        TOKEN.clear(&pool).await.unwrap();
        assert_eq!(TOKEN.get(&pool).await.unwrap(), None);
        // Clearing what was never set is fine too.
        TOKEN.clear(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn a_corrupt_value_is_an_error_not_a_default() {
        let pool = db::open_in_memory().await.unwrap();
        sqlx::query("INSERT INTO app_settings (key, value) VALUES ('test.prefs', '{oops')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            PREFS.get(&pool).await,
            Err(SettingError::Decode(_))
        ));
    }
}
