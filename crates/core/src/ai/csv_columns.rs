//! Working out which column of a CSV file holds each field, with the model the "csv_columns"
//! task uses, when neither a saved template nor the column names say. A decision model answers
//! one choice question per field; a language model returns them all as JSON.

use std::time::Instant;

use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::config::{Kind, Task};
use super::http::Transport;
use super::log_call;
use super::providers::{self, JsonRequest, ProviderError, ResolveError, Resolved};
use crate::accounts::quoted;
use crate::imports::csv::{FIELDS, missing_fields};
use crate::imports::templates::{self, SAMPLE_ROWS, Source, Suggestion, Template};

const NONE: &str = "none";

const QUESTIONS: [(&str, &str); 5] = [
    (
        "date",
        "Which column of `csv` holds the date of each transaction?",
    ),
    (
        "description",
        "Which column of `csv` holds the description of each transaction: who was paid or who \
         paid, the merchant or the reference?",
    ),
    (
        "amount",
        "Which column of `csv` holds each transaction's amount as one signed number, with money \
         in and money out in the same column? Not a running balance.",
    ),
    (
        "money_out",
        "Which column of `csv` holds only money paid out (debits), with money paid in in another \
         column? Not a running balance.",
    ),
    (
        "money_in",
        "Which column of `csv` holds only money paid in (credits), with money paid out in \
         another column? Not a running balance.",
    ),
];

const SYSTEM: &str = "You read bank statement CSV files exported by banks and card providers.";

type Found = Vec<(String, Suggestion)>;

async fn with_decision_model(
    http: &dyn Transport,
    model: &Resolved,
    headers: &[String],
    rows: &[Vec<String>],
) -> Result<(Found, Value, Value), ProviderError> {
    let mut options: Map<String, Value> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            (
                i.to_string(),
                json!(format!("Column {}, headed {}", i + 1, quoted(h))),
            )
        })
        .collect();
    options.insert(NONE.into(), json!("No column holds this"));
    let state = json!({"csv": {"headers": headers, "rows": rows}});
    let questions: Map<String, Value> = QUESTIONS
        .iter()
        .map(|(field, question)| {
            (
                (*field).to_owned(),
                json!({"type": "choice", "instructions": question, "criteria": options}),
            )
        })
        .collect();
    let call = model
        .provider
        .choose(http, &model.cfg, &model.model, &state, &questions)
        .await?;
    let mut found = Vec::new();
    for (field, _) in QUESTIONS {
        let answer = call.data.get(field).cloned().unwrap_or(Value::Null);
        let Some(choice) = answer["choice"]
            .as_str()
            .filter(|c| options.contains_key(*c))
        else {
            continue;
        };
        let column = if choice == NONE {
            None
        } else {
            choice.parse().ok()
        };
        found.push((
            field.to_owned(),
            Suggestion {
                column,
                source: Source::Ai,
                confidence: answer["confidence"].as_f64(),
            },
        ));
    }
    Ok((found, call.request, call.response))
}

async fn with_language_model(
    http: &dyn Transport,
    model: &Resolved,
    headers: &[String],
    rows: &[Vec<String>],
) -> Result<(Found, Value, Value), ProviderError> {
    let listing = serde_json::to_string_pretty(&json!({"headers": headers, "rows": rows}))
        .expect("JSON serialises");
    let questions: Vec<String> = QUESTIONS
        .iter()
        .map(|(field, question)| format!("- {field}: {}", question.replace("`csv`", "the file")))
        .collect();
    let prompt = format!(
        "Here are the headers and first rows of a bank statement CSV file:\n\n{listing}\n\nFor \
         each field, give the index (counting from 0) of the column that holds it, or null if \
         none\ndoes.\n{}",
        questions.join("\n")
    );
    let fields: Vec<&str> = QUESTIONS.iter().map(|(f, _)| *f).collect();
    let properties: Map<String, Value> = fields
        .iter()
        .map(|f| ((*f).to_owned(), json!({"type": ["integer", "null"]})))
        .collect();
    let schema = json!({
        "type": "object",
        "properties": properties,
        "required": fields,
        "additionalProperties": false,
    });
    let call = model
        .provider
        .complete_json(
            http,
            &model.cfg,
            JsonRequest {
                model: &model.model,
                system: SYSTEM,
                prompt: &prompt,
                schema: &schema,
                name: "csv_columns",
            },
        )
        .await?;
    let mut found = Vec::new();
    for field in fields {
        let column = match call.data.get(field) {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_u64() {
                Some(i) if (i as usize) < headers.len() => Some(i as usize),
                _ => continue,
            },
        };
        found.push((
            field.to_owned(),
            Suggestion {
                column,
                source: Source::Ai,
                confidence: None,
            },
        ));
    }
    Ok((found, call.request, call.response))
}

#[derive(Debug, thiserror::Error)]
pub enum ColumnsError {
    /// The task isn't set up, or the model failed (and the AI logs say so).
    #[error("{0}")]
    Model(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// The model's column for each field it answered.
pub async fn suggest_with_ai(
    db: &SqlitePool,
    http: &dyn Transport,
    headers: &[String],
    rows: &[Vec<String>],
) -> Result<Found, ColumnsError> {
    let model = match providers::resolve_task(db, Task::CsvColumns).await {
        Ok(model) => model,
        Err(ResolveError::Provider(e)) => return Err(ColumnsError::Model(e.message)),
        Err(ResolveError::Setting(e)) => return Err(ColumnsError::Model(e.to_string())),
    };
    let rows = &rows[..rows.len().min(SAMPLE_ROWS)];
    let started = Instant::now();
    let answered = match model.provider.kind() {
        Kind::Decision => with_decision_model(http, &model, headers, rows).await,
        Kind::Language => with_language_model(http, &model, headers, rows).await,
    };
    let mut found = match answered {
        Ok((found, request, response)) => {
            log_call(
                db,
                &model.label(),
                "csv_columns",
                started,
                Some(&request),
                Some(&response),
                None,
            )
            .await;
            found
        }
        Err(e) => {
            log_call(
                db,
                &model.label(),
                "csv_columns",
                started,
                None,
                None,
                Some(&e.message),
            )
            .await;
            return Err(ColumnsError::Model(e.message));
        }
    };
    found.retain(|(_, s)| s.column.is_none_or(|c| c < headers.len()));
    // One signed amount column makes separate money in and out columns redundant.
    if found
        .iter()
        .any(|(f, s)| f == "amount" && s.column.is_some())
    {
        found.retain(|(f, _)| f != "money_out" && f != "money_in");
    }
    Ok(found)
}

/// Whether the column names leave fields the importer needs unaccounted for.
pub fn names_leave_gaps(headers: &[String]) -> bool {
    let named = crate::imports::csv::columns_by_name(headers);
    !missing_fields(&named).is_empty()
}

/// Each field's column, the template it came from if any, and the model's problem if it was
/// asked and couldn't help. Column names win over the model, which only fills the gaps.
pub async fn suggest(
    db: &SqlitePool,
    http: &dyn Transport,
    headers: &[String],
    rows: &[Vec<String>],
    use_ai: bool,
) -> Result<(Found, Option<Template>, Option<String>), sqlx::Error> {
    let (mut columns, template) = templates::suggest(db, headers).await?;
    if template.is_some() || !use_ai {
        return Ok((columns, template, None));
    }
    let from_ai = match suggest_with_ai(db, http, headers, rows).await {
        Ok(found) => found,
        Err(ColumnsError::Model(message)) => return Ok((columns, None, Some(message))),
        Err(ColumnsError::Database(e)) => return Err(e),
    };
    let amounts = ["amount", "money_out", "money_in"];
    let named_amount = columns.iter().any(|(f, _)| amounts.contains(&f.as_str()));
    for (field, suggestion) in from_ai {
        if columns.iter().any(|(f, _)| *f == field) || suggestion.column.is_none() {
            continue;
        }
        // The names already say how amounts are laid out.
        if amounts.contains(&field.as_str()) && named_amount {
            continue;
        }
        columns.push((field, suggestion));
    }
    // In field order, as the names give them.
    columns.sort_by_key(|(f, _)| FIELDS.iter().position(|(name, _)| name == f));
    Ok((columns, None, None))
}
