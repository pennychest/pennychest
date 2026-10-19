//! Saved CSV column mappings, used for later files with the same headers, and working out
//! which column holds what for a file the CSV importer doesn't recognise.

use serde::Serialize;
use sqlx::SqlitePool;

use super::csv::{self, Columns, FIELDS, missing_fields, normalise_header};

/// Rows of a file shown to the user (and the model).
pub const SAMPLE_ROWS: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Template {
    pub id: i64,
    pub name: String,
    /// The headers as they're matched: trimmed, lower case, underscores as spaces.
    pub headers: Vec<String>,
    pub columns: Columns,
}

/// Where a field's column came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Template,
    Name,
    Ai,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub column: Option<usize>,
    pub source: Source,
    /// Only decision models report one.
    pub confidence: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("Give the template a name.")]
    NoName,
    #[error("There's already a template called {0}.")]
    NameTaken(String),
    #[error("Unknown fields: {0}")]
    UnknownFields(String),
    #[error("A column is outside the file.")]
    OutsideFile,
    #[error("Choose the columns for the {0}.")]
    Incomplete(String),
    #[error("Template not found")]
    NotFound,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// The headers in one form, so a template matches files laid out the same way: a JSON list
/// with ", " between items and non-ASCII escaped.
pub fn header_key(headers: &[String]) -> String {
    let items: Vec<String> = headers
        .iter()
        .map(|h| json_string(&normalise_header(h)))
        .collect();
    format!("[{}]", items.join(", "))
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(sqlx::FromRow)]
struct TemplateRow {
    id: i64,
    name: String,
    header_key: String,
    columns: String,
}

impl TemplateRow {
    fn into_template(self) -> Template {
        Template {
            id: self.id,
            name: self.name,
            headers: serde_json::from_str(&self.header_key).unwrap_or_default(),
            columns: serde_json::from_str(&self.columns).unwrap_or_default(),
        }
    }
}

/// Every template, by name.
pub async fn list(db: &SqlitePool) -> Result<Vec<Template>, sqlx::Error> {
    let rows: Vec<TemplateRow> =
        sqlx::query_as("SELECT id, name, header_key, columns FROM csv_templates ORDER BY name")
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(TemplateRow::into_template).collect())
}

/// The most recently saved template for files with these headers.
pub async fn find(db: &SqlitePool, headers: &[String]) -> Result<Option<Template>, sqlx::Error> {
    let row: Option<TemplateRow> = sqlx::query_as(
        "SELECT id, name, header_key, columns FROM csv_templates
         WHERE header_key = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(header_key(headers))
    .fetch_optional(db)
    .await?;
    Ok(row.map(TemplateRow::into_template))
}

/// Whether a mapping can be used on a file `width` columns wide.
pub fn check_columns(columns: &Columns, width: usize) -> Result<(), TemplateError> {
    let known: Vec<&str> = FIELDS.iter().map(|(field, _)| *field).collect();
    let unknown: Vec<&str> = columns
        .keys()
        .map(String::as_str)
        .filter(|f| !known.contains(f))
        .collect();
    if !unknown.is_empty() {
        return Err(TemplateError::UnknownFields(unknown.join(", ")));
    }
    if columns.values().any(|c| *c >= width) {
        return Err(TemplateError::OutsideFile);
    }
    let missing = missing_fields(columns);
    if !missing.is_empty() {
        return Err(TemplateError::Incomplete(missing.join(" and ")));
    }
    Ok(())
}

/// Saves a column mapping for later files with the same headers.
pub async fn save(
    db: &SqlitePool,
    name: &str,
    headers: &[String],
    columns: Columns,
) -> Result<Template, TemplateError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TemplateError::NoName);
    }
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM csv_templates WHERE name = ?)")
            .bind(name)
            .fetch_one(db)
            .await?;
    if taken {
        return Err(TemplateError::NameTaken(name.to_owned()));
    }
    check_columns(&columns, headers.len())?;
    let key = header_key(headers);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO csv_templates (name, header_key, columns) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(&key)
    .bind(serde_json::to_string(&columns).expect("columns serialise"))
    .fetch_one(db)
    .await?;
    Ok(Template {
        id,
        name: name.to_owned(),
        headers: serde_json::from_str(&key).expect("a header key is a JSON list"),
        columns,
    })
}

pub async fn delete(db: &SqlitePool, id: i64) -> Result<(), TemplateError> {
    let result = sqlx::query("DELETE FROM csv_templates WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(TemplateError::NotFound);
    }
    Ok(())
}

/// Each field's column, from a saved template for these headers if there is one, else from
/// the column names. Returns the template too.
pub async fn suggest(
    db: &SqlitePool,
    headers: &[String],
) -> Result<(Vec<(String, Suggestion)>, Option<Template>), sqlx::Error> {
    if let Some(template) = find(db, headers).await? {
        let known: Vec<&str> = FIELDS.iter().map(|(f, _)| *f).collect();
        let columns = template
            .columns
            .iter()
            .filter(|(field, _)| known.contains(&field.as_str()))
            .map(|(field, column)| {
                let suggestion = Suggestion {
                    column: Some(*column),
                    source: Source::Template,
                    confidence: None,
                };
                (field.clone(), suggestion)
            })
            .collect();
        return Ok((columns, Some(template)));
    }
    Ok((by_name(headers), None))
}

/// The columns whose names say what they hold, in field order.
pub fn by_name(headers: &[String]) -> Vec<(String, Suggestion)> {
    let named = csv::columns_by_name(headers);
    FIELDS
        .iter()
        .filter_map(|(field, _)| {
            named.get(*field).map(|column| {
                let suggestion = Suggestion {
                    column: Some(*column),
                    source: Source::Name,
                    confidence: None,
                };
                ((*field).to_owned(), suggestion)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn columns(pairs: &[(&str, usize)]) -> Columns {
        pairs.iter().map(|(f, c)| ((*f).to_owned(), *c)).collect()
    }

    #[test]
    fn header_keys_are_normalised_json() {
        assert_eq!(
            header_key(&strings(&["Posted", " Ref ", "Money_In", "Café \"x\""])),
            "[\"posted\", \"ref\", \"money in\", \"caf\\u00e9 \\\"x\\\"\"]"
        );
    }

    #[test]
    fn checks_mappings() {
        let ok = columns(&[("date", 0), ("description", 2), ("money_out", 3)]);
        assert!(check_columns(&ok, 6).is_ok());
        let mut unknown = ok.clone();
        unknown.insert("balance".into(), 5);
        assert_eq!(
            check_columns(&unknown, 6).unwrap_err().to_string(),
            "Unknown fields: balance"
        );
        assert!(matches!(
            check_columns(&ok, 3),
            Err(TemplateError::OutsideFile)
        ));
        assert_eq!(
            check_columns(&columns(&[("date", 0)]), 6)
                .unwrap_err()
                .to_string(),
            "Choose the columns for the description and amount."
        );
    }

    #[tokio::test]
    async fn saved_templates_match_files_laid_out_the_same() {
        let pool = db::open_in_memory().await.unwrap();
        let headers = strings(&["Posted", "Ref", "Who", "Out", "In", "Running total"]);
        let mapping = columns(&[
            ("date", 0),
            ("description", 2),
            ("money_out", 3),
            ("money_in", 4),
        ]);
        let saved = save(&pool, " Monzo ", &headers, mapping.clone())
            .await
            .unwrap();
        assert_eq!(saved.name, "Monzo");
        assert_eq!(saved.headers[0], "posted");
        assert!(matches!(
            save(&pool, "Monzo", &headers, mapping.clone()).await,
            Err(TemplateError::NameTaken(_))
        ));
        assert!(matches!(
            save(&pool, " ", &headers, mapping).await,
            Err(TemplateError::NoName)
        ));

        let later = strings(&[" posted ", "Ref", "Who", "Out", "In", "Running total"]);
        let (columns, template) = suggest(&pool, &later).await.unwrap();
        assert_eq!(template.unwrap().id, saved.id);
        assert!(columns.iter().all(|(_, s)| s.source == Source::Template));
        assert_eq!(list(&pool).await.unwrap().len(), 1);

        delete(&pool, saved.id).await.unwrap();
        assert!(matches!(
            delete(&pool, saved.id).await,
            Err(TemplateError::NotFound)
        ));
        let (columns, template) = suggest(&pool, &later).await.unwrap();
        assert!(template.is_none() && columns.is_empty());
    }
}
