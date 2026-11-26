//! Tags: labels on transactions, such as who paid or which trip it was for. A transaction can
//! have any number, and a tag exists while a transaction has it. Names ignore case: "Becca" and
//! "becca" are the same tag, spelt as it was first used.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, SqliteConnection, SqlitePool};

pub const MAX_LENGTH: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    #[error("A tag can't be empty.")]
    Empty,
    #[error("A tag can't be longer than {MAX_LENGTH} characters.")]
    TooLong,
    #[error("A tag can't have a comma in it.")]
    Comma,
}

/// The name as it's kept: trimmed, with runs of spaces made one. Commas aren't allowed, since
/// they separate tags in a filter.
pub fn normalise(name: &str) -> Result<String, TagError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        Err(TagError::Empty)
    } else if name.chars().count() > MAX_LENGTH {
        Err(TagError::TooLong)
    } else if name.contains(',') {
        Err(TagError::Comma)
    } else {
        Ok(name)
    }
}

/// Each name normalised, once each ignoring case, in the order given.
pub fn normalise_all<S: AsRef<str>>(names: &[S]) -> Result<Vec<String>, TagError> {
    let mut seen = BTreeSet::new();
    let mut tags = Vec::new();
    for name in names {
        let name = normalise(name.as_ref())?;
        if seen.insert(name.to_lowercase()) {
            tags.push(name);
        }
    }
    Ok(tags)
}

/// Forgets tags no transaction has.
async fn forget_unused(db: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM transaction_tags)")
        .execute(db)
        .await?;
    Ok(())
}

async fn tag_id(db: &mut SqliteConnection, name: &str) -> Result<i64, sqlx::Error> {
    sqlx::query("INSERT INTO tags (name) VALUES (?) ON CONFLICT (name) DO NOTHING")
        .bind(name)
        .execute(&mut *db)
        .await?;
    sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(db)
        .await
}

/// Gives the transaction these tags (already normalised), keeping any it has. Returns the ones
/// it didn't have, as the tags are spelt.
pub async fn add(
    db: &mut SqliteConnection,
    transaction_id: i64,
    names: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    forget_unused(db).await?;
    let mut added = Vec::new();
    for name in names {
        let id = tag_id(db, name).await?;
        let inserted = sqlx::query(
            "INSERT INTO transaction_tags (transaction_id, tag_id) VALUES (?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(transaction_id)
        .bind(id)
        .execute(&mut *db)
        .await?
        .rows_affected();
        if inserted > 0 {
            let spelt: String = sqlx::query_scalar("SELECT name FROM tags WHERE id = ?")
                .bind(id)
                .fetch_one(&mut *db)
                .await?;
            added.push(spelt);
        }
    }
    Ok(added)
}

/// Takes these tags off the transaction. Returns the ones it had, as they're spelt.
pub async fn remove(
    db: &mut SqliteConnection,
    transaction_id: i64,
    names: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    let mut removed = Vec::new();
    for name in names {
        let tag: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM tags WHERE name = ?")
            .bind(name)
            .fetch_optional(&mut *db)
            .await?;
        let Some((id, spelt)) = tag else { continue };
        let deleted =
            sqlx::query("DELETE FROM transaction_tags WHERE transaction_id = ? AND tag_id = ?")
                .bind(transaction_id)
                .bind(id)
                .execute(&mut *db)
                .await?
                .rows_affected();
        if deleted > 0 {
            removed.push(spelt);
        }
    }
    forget_unused(db).await?;
    Ok(removed)
}

/// Gives the transaction exactly these tags (already normalised).
pub async fn set(
    db: &mut SqliteConnection,
    transaction_id: i64,
    names: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM transaction_tags WHERE transaction_id = ?")
        .bind(transaction_id)
        .execute(&mut *db)
        .await?;
    add(db, transaction_id, names).await?;
    forget_unused(db).await
}

/// The tag with this name, ignoring case: its ID and how it's spelt.
pub async fn find(
    db: &mut SqliteConnection,
    name: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    forget_unused(db).await?;
    sqlx::query_as("SELECT id, name FROM tags WHERE name = ?")
        .bind(name)
        .fetch_optional(db)
        .await
}

async fn transactions_with(db: &mut SqliteConnection, id: i64) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT transaction_id FROM transaction_tags WHERE tag_id = ? ORDER BY transaction_id",
    )
    .bind(id)
    .fetch_all(db)
    .await
}

/// What renaming a tag did, which is also what's needed to undo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Renamed {
    /// The tag as it was spelt.
    pub from: String,
    /// The tag its transactions have now, as it's spelt.
    pub to: String,
    /// Whether `to` was already another tag, so the two became one.
    pub merged: bool,
    /// The transactions that had `from`.
    pub transaction_ids: Vec<i64>,
    /// Of those, the ones that had `to` as well.
    pub had_both: Vec<i64>,
}

/// Renames a tag on every transaction that has it. If `to` (already normalised) is another
/// tag, the two are merged into that one. `None` if there's no tag `from`.
pub async fn rename(
    db: &mut SqliteConnection,
    from: &str,
    to: &str,
) -> Result<Option<Renamed>, sqlx::Error> {
    let Some((id, spelt)) = find(db, from).await? else {
        return Ok(None);
    };
    let transaction_ids = transactions_with(db, id).await?;
    let other = find(db, to).await?.filter(|(other, _)| *other != id);
    let Some((other, other_spelt)) = other else {
        sqlx::query("UPDATE tags SET name = ? WHERE id = ?")
            .bind(to)
            .bind(id)
            .execute(db)
            .await?;
        return Ok(Some(Renamed {
            from: spelt,
            to: to.to_owned(),
            merged: false,
            transaction_ids,
            had_both: Vec::new(),
        }));
    };
    let had_both = sqlx::query_scalar(
        "SELECT transaction_id FROM transaction_tags WHERE tag_id = ? AND transaction_id IN
         (SELECT transaction_id FROM transaction_tags WHERE tag_id = ?) ORDER BY transaction_id",
    )
    .bind(id)
    .bind(other)
    .fetch_all(&mut *db)
    .await?;
    sqlx::query("UPDATE OR IGNORE transaction_tags SET tag_id = ? WHERE tag_id = ?")
        .bind(other)
        .bind(id)
        .execute(&mut *db)
        .await?;
    sqlx::query("DELETE FROM transaction_tags WHERE tag_id = ?")
        .bind(id)
        .execute(&mut *db)
        .await?;
    forget_unused(db).await?;
    Ok(Some(Renamed {
        from: spelt,
        to: other_spelt,
        merged: true,
        transaction_ids,
        had_both,
    }))
}

/// Takes a tag off every transaction that has it. Returns the tag as it was spelt and those
/// transactions, or `None` if there's no such tag.
pub async fn delete(
    db: &mut SqliteConnection,
    name: &str,
) -> Result<Option<(String, Vec<i64>)>, sqlx::Error> {
    let Some((id, spelt)) = find(db, name).await? else {
        return Ok(None);
    };
    let transaction_ids = transactions_with(db, id).await?;
    sqlx::query("DELETE FROM transaction_tags WHERE tag_id = ?")
        .bind(id)
        .execute(&mut *db)
        .await?;
    forget_unused(db).await?;
    Ok(Some((spelt, transaction_ids)))
}

/// Each transaction's tags, in alphabetical order. Transactions without any aren't in the map.
pub async fn for_transactions<'e>(
    db: impl sqlx::SqliteExecutor<'e>,
    ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, sqlx::Error> {
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    if ids.is_empty() {
        return Ok(tags);
    }
    let mut query = QueryBuilder::new(
        "SELECT tt.transaction_id, t.name FROM transaction_tags tt
         JOIN tags t ON t.id = tt.tag_id WHERE tt.transaction_id IN (",
    );
    let mut list = query.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    query.push(") ORDER BY t.name COLLATE NOCASE");
    let rows: Vec<(i64, String)> = query.build_query_as().fetch_all(db).await?;
    for (id, name) in rows {
        tags.entry(id).or_default().push(name);
    }
    Ok(tags)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct TagCount {
    pub name: String,
    pub transaction_count: i64,
}

/// Every tag in use, alphabetically, with how many transactions have it.
pub async fn list(db: &SqlitePool) -> Result<Vec<TagCount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT t.name, COUNT(*) AS transaction_count FROM tags t
         JOIN transaction_tags tt ON tt.tag_id = t.id
         GROUP BY t.id ORDER BY t.name COLLATE NOCASE",
    )
    .fetch_all(db)
    .await
}

/// SQL for "the transaction `t` has every one of these tags", to add to a query's conditions.
pub fn push_filter(query: &mut QueryBuilder<'_, sqlx::Sqlite>, names: &[String]) {
    for name in names {
        query
            .push(
                " AND t.id IN (SELECT tt.transaction_id FROM transaction_tags tt
                  JOIN tags g ON g.id = tt.tag_id WHERE g.name = ",
            )
            .push_bind(name.clone())
            .push(")");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn normalises_names() {
        assert_eq!(normalise("  Becca   card "), Ok("Becca card".into()));
        assert_eq!(normalise(" "), Err(TagError::Empty));
        assert_eq!(normalise("a,b"), Err(TagError::Comma));
        assert_eq!(normalise(&"x".repeat(51)), Err(TagError::TooLong));
        assert_eq!(
            normalise_all(&["Becca", "becca", "Holiday"]),
            Ok(vec!["Becca".into(), "Holiday".into()])
        );
    }

    async fn transaction(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO transactions (date, description) VALUES ('2026-01-01', 'x') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn adds_removes_and_lists_ignoring_case() {
        let pool = db::open_in_memory().await.unwrap();
        let (one, two) = (transaction(&pool).await, transaction(&pool).await);
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert_eq!(
            add(
                &mut pool.acquire().await.unwrap(),
                one,
                &names(&["Becca", "Holiday"])
            )
            .await
            .unwrap(),
            ["Becca", "Holiday"]
        );
        assert_eq!(
            add(&mut pool.acquire().await.unwrap(), one, &names(&["BECCA"]))
                .await
                .unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            add(&mut pool.acquire().await.unwrap(), two, &names(&["becca"]))
                .await
                .unwrap(),
            ["Becca"],
            "spelt as first used"
        );
        assert_eq!(
            list(&pool).await.unwrap(),
            [
                TagCount {
                    name: "Becca".into(),
                    transaction_count: 2
                },
                TagCount {
                    name: "Holiday".into(),
                    transaction_count: 1
                },
            ]
        );

        let mut query = QueryBuilder::new("SELECT t.id FROM transactions t WHERE 1 = 1");
        push_filter(&mut query, &names(&["becca", "Holiday"]));
        let both: Vec<i64> = query.build_query_scalar().fetch_all(&pool).await.unwrap();
        assert_eq!(both, [one]);

        assert_eq!(
            remove(
                &mut pool.acquire().await.unwrap(),
                one,
                &names(&["holiday", "Nope"])
            )
            .await
            .unwrap(),
            ["Holiday"]
        );
        set(&mut pool.acquire().await.unwrap(), two, &[])
            .await
            .unwrap();
        let tags = for_transactions(&pool, &[one, two]).await.unwrap();
        assert_eq!(tags.get(&one).unwrap(), &["Becca"]);
        assert!(!tags.contains_key(&two));
        let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kept, 1, "unused tags are forgotten");
    }

    #[tokio::test]
    async fn renames_merges_and_deletes() {
        let pool = db::open_in_memory().await.unwrap();
        let (one, two) = (transaction(&pool).await, transaction(&pool).await);
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut db = pool.acquire().await.unwrap();
        add(&mut db, one, &names(&["Rebecca Stievenard", "Becca"]))
            .await
            .unwrap();
        add(&mut db, two, &names(&["Rebecca Stievenard"]))
            .await
            .unwrap();

        assert_eq!(rename(&mut db, "Nope", "Becca").await.unwrap(), None);
        let renamed = rename(&mut db, "rebecca stievenard", "Rebecca Bailey")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(renamed.from, "Rebecca Stievenard");
        assert!(!renamed.merged);
        assert_eq!(renamed.transaction_ids, [one, two]);
        let recased = rename(&mut db, "Rebecca Bailey", "rebecca bailey")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (recased.to.as_str(), recased.merged),
            ("rebecca bailey", false)
        );

        let merged = rename(&mut db, "rebecca bailey", "becca")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            merged,
            Renamed {
                from: "rebecca bailey".into(),
                to: "Becca".into(),
                merged: true,
                transaction_ids: vec![one, two],
                had_both: vec![one],
            }
        );
        drop(db);
        assert_eq!(
            list(&pool).await.unwrap(),
            [TagCount {
                name: "Becca".into(),
                transaction_count: 2
            }]
        );

        let mut db = pool.acquire().await.unwrap();
        assert_eq!(
            delete(&mut db, "BECCA").await.unwrap(),
            Some(("Becca".into(), vec![one, two]))
        );
        assert_eq!(delete(&mut db, "Becca").await.unwrap(), None);
    }
}
