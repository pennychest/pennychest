//! Suggesting rules that categorise transactions automatically, with the language model the
//! "rules" task uses.

use std::collections::HashSet;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::config::Task;
use super::http::Transport;
use super::providers::JsonRequest;
use super::{ResolveOrFail, log_call, resolve};
use crate::imports::batches::UNCATEGORISED_PATH;
use crate::lookups::{CategorisationSource, MatchType};

const MATCH_TYPES: [&str; 3] = ["substring", "prefix", "regex"];

const SYSTEM: &str = "You are a personal finance assistant. You suggest rules that \
                      automatically categorise bank transactions.";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub pattern: String,
    pub match_type: String,
    pub match_type_id: Option<i64>,
    pub target_account_full_path: String,
    pub target_account_id: Option<i64>,
    pub priority: i64,
    pub description: String,
}

fn schema(paths: &[String]) -> Value {
    json!({
        "type": "object",
        "properties": {
            "rules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "match_type": {"type": "string", "enum": MATCH_TYPES},
                        "target_account_full_path": {"type": "string", "enum": paths},
                        "priority": {
                            "type": "integer",
                            "description": "Between 1 and 100; higher is matched first.",
                        },
                        "description": {
                            "type": "string",
                            "description": "A brief factual note about what this pattern \
                                matches, e.g. the merchant name or type. Do not mention \
                                transaction counts.",
                        },
                    },
                    "required": [
                        "pattern",
                        "match_type",
                        "target_account_full_path",
                        "priority",
                        "description",
                    ],
                    "additionalProperties": false,
                },
            }
        },
        "required": ["rules"],
        "additionalProperties": false,
    })
}

fn prompt(
    paths: &[String],
    examples: &[(String, String)],
    existing: &[(String, String, String)],
) -> String {
    let accounts: Vec<String> = paths.iter().map(|p| format!("- {p}")).collect();
    let existing = if existing.is_empty() {
        "None".to_owned()
    } else {
        existing
            .iter()
            .map(|(pattern, kind, path)| format!("- \"{pattern}\" ({kind}) → {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (examples, guideline) = if examples.is_empty() {
        (
            "No categorised transactions are available yet.".to_owned(),
            "- Suggest useful starter rules based on common UK personal finance transaction \
             patterns (well-known supermarkets, transport, utilities, streaming services, etc.) \
             that match the available accounts above.",
        )
    } else {
        let listed: Vec<String> = examples
            .iter()
            .map(|(d, path)| format!("- \"{d}\" → {path}"))
            .collect();
        (
            format!("Recently categorised transactions:\n{}", listed.join("\n")),
            "- Only suggest rules for patterns that appear more than once, or are clearly \
             distinctive (e.g. a known brand name).",
        )
    };
    format!(
        "Suggest rules to auto-categorise bank transactions.\n\nExisting rules (do not duplicate \
         these):\n{existing}\n\nAvailable accounts:\n{}\n\n{examples}\n\nGuidelines:\n{guideline}\n\
         - Prefer \"substring\" match type. Use \"prefix\" only if the pattern is always at the \
         start. Avoid \"regex\" unless necessary.\n- Set priority between 1 and 100 (higher = \
         matched first). Use 10 as the default.\n- Do not suggest rules that duplicate or overlap \
         with existing rules.\n- Keep patterns short and general enough to match future \
         variations.\n- For description: write a brief factual note about what the pattern \
         matches (e.g. \"Major UK supermarket chain\"). Do not mention transaction counts.",
        accounts.join("\n")
    )
}

#[derive(Debug, thiserror::Error)]
pub enum SuggestError {
    /// What went wrong for the user, also in the AI logs.
    #[error("{0}")]
    Model(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Rules suggested from the transactions the user or the model categorised most recently, and
/// the rules there are already.
pub async fn suggest(
    db: &SqlitePool,
    http: &dyn Transport,
) -> Result<Vec<Suggestion>, SuggestError> {
    let examples: Vec<(String, String)> = sqlx::query_as(
        "SELECT t.description, a.full_path FROM postings p
         JOIN transactions t ON t.id = p.transaction_id
         JOIN accounts a ON a.id = p.account_id
         WHERE p.categorised_by_id IN (?, ?)
         ORDER BY p.id DESC LIMIT 200",
    )
    .bind(CategorisationSource::Manual as i64)
    .bind(CategorisationSource::Ai as i64)
    .fetch_all(db)
    .await?;
    let rules: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT r.pattern, r.match_type_id, a.full_path FROM rules r
         LEFT JOIN accounts a ON a.id = r.target_account_id ORDER BY r.id",
    )
    .fetch_all(db)
    .await?;
    let existing: Vec<(String, String, String)> = rules
        .into_iter()
        .map(|(pattern, kind, path)| {
            let kind = MatchType::try_from(kind).map_or("substring", MatchType::name);
            (pattern, kind.to_owned(), path.unwrap_or_default())
        })
        .collect();
    let accounts: Vec<(i64, String)> = sqlx::query_as("SELECT id, full_path FROM accounts")
        .fetch_all(db)
        .await?;
    let mut paths: Vec<String> = accounts
        .iter()
        .map(|(_, p)| p.clone())
        .filter(|p| p != UNCATEGORISED_PATH)
        .collect();
    paths.sort();

    let model = match resolve(db, Task::Rules, "suggest_rules").await {
        ResolveOrFail::Ready(model) => model,
        ResolveOrFail::Failed(message) => return Err(SuggestError::Model(message)),
    };
    let started = Instant::now();
    let schema = schema(&paths);
    let prompt = prompt(&paths, &examples, &existing);
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
                name: "suggest_rules",
            },
        )
        .await;
    let call = match call {
        Ok(call) => call,
        Err(e) => {
            log_call(
                db,
                &model.label(),
                "suggest_rules",
                started,
                None,
                None,
                Some(&e.message),
            )
            .await;
            return Err(SuggestError::Model(e.message));
        }
    };
    log_call(
        db,
        &model.label(),
        "suggest_rules",
        started,
        Some(&call.request),
        Some(&call.response),
        None,
    )
    .await;

    let known: HashSet<&String> = paths.iter().collect();
    let id_of = |path: &str| accounts.iter().find(|(_, p)| p == path).map(|(id, _)| *id);
    let suggestions = call
        .data
        .get("rules")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let pattern = r["pattern"].as_str().filter(|p| !p.is_empty())?;
            let match_type = r["match_type"]
                .as_str()
                .filter(|m| MATCH_TYPES.contains(m))?;
            let path = r["target_account_full_path"].as_str()?;
            if !known.contains(&path.to_owned()) {
                return None;
            }
            let priority = match &r["priority"] {
                Value::Number(n) => n.as_f64().map_or(10, |p| p as i64),
                Value::String(s) => s.trim().parse().unwrap_or(10),
                _ => 10,
            };
            let priority = if priority == 0 {
                10
            } else {
                priority.clamp(1, 100)
            };
            Some(Suggestion {
                pattern: pattern.to_owned(),
                match_type: match_type.to_owned(),
                match_type_id: MatchType::from_name(match_type).map(|m| m as i64),
                target_account_full_path: path.to_owned(),
                target_account_id: id_of(path),
                priority,
                description: match &r["description"] {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                },
            })
        })
        .collect();
    Ok(suggestions)
}
