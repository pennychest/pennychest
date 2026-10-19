//! The lookup tables' rows. Their ids are fixed by the first migration, which seeds them in a
//! set order, so code can name them rather than look them up.

use serde::{Deserialize, Serialize};

/// How a posting came to be in its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[repr(i64)]
#[serde(rename_all = "snake_case")]
pub enum CategorisationSource {
    Rule = 1,
    Manual = 2,
    ImportDefault = 3,
    Ai = 4,
    Transfer = 5,
    Learned = 6,
}

impl TryFrom<i64> for CategorisationSource {
    type Error = i64;

    /// The source with this id, or the id back if there's none.
    fn try_from(id: i64) -> Result<Self, i64> {
        Ok(match id {
            1 => Self::Rule,
            2 => Self::Manual,
            3 => Self::ImportDefault,
            4 => Self::Ai,
            5 => Self::Transfer,
            6 => Self::Learned,
            other => return Err(other),
        })
    }
}

/// How a rule's pattern is matched against a description.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[repr(i64)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    Substring = 1,
    Prefix = 2,
    Regex = 3,
}

impl TryFrom<i64> for MatchType {
    type Error = i64;

    /// The match type with this id, or the id back if there's none.
    fn try_from(id: i64) -> Result<Self, i64> {
        Ok(match id {
            1 => Self::Substring,
            2 => Self::Prefix,
            3 => Self::Regex,
            other => return Err(other),
        })
    }
}

impl MatchType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Substring => "substring",
            Self::Prefix => "prefix",
            Self::Regex => "regex",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "substring" => Some(Self::Substring),
            "prefix" => Some(Self::Prefix),
            "regex" => Some(Self::Regex),
            _ => None,
        }
    }
}

/// A lookup table's rows, as `/api/lookup/...` lists them.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct LookupItem {
    pub id: i64,
    pub name: String,
}

pub async fn match_types(db: &sqlx::SqlitePool) -> Result<Vec<LookupItem>, sqlx::Error> {
    sqlx::query_as("SELECT id, name FROM match_types ORDER BY id")
        .fetch_all(db)
        .await
}

pub async fn categorisation_sources(db: &sqlx::SqlitePool) -> Result<Vec<LookupItem>, sqlx::Error> {
    sqlx::query_as("SELECT id, name FROM categorisation_sources ORDER BY id")
        .fetch_all(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn ids_match_the_seeded_rows() {
        let pool = db::open_in_memory().await.unwrap();
        let sources = categorisation_sources(&pool).await.unwrap();
        for (source, name) in [
            (CategorisationSource::Rule, "rule"),
            (CategorisationSource::Manual, "manual"),
            (CategorisationSource::ImportDefault, "import_default"),
            (CategorisationSource::Ai, "ai"),
            (CategorisationSource::Transfer, "transfer"),
            (CategorisationSource::Learned, "learned"),
        ] {
            let row = sources.iter().find(|s| s.id == source as i64).unwrap();
            assert_eq!(row.name, name);
        }
        let types = match_types(&pool).await.unwrap();
        for (kind, name) in [
            (MatchType::Substring, "substring"),
            (MatchType::Prefix, "prefix"),
            (MatchType::Regex, "regex"),
        ] {
            assert_eq!(
                types.iter().find(|t| t.id == kind as i64).unwrap().name,
                name
            );
            assert_eq!(MatchType::from_name(name), Some(kind));
        }
    }
}
