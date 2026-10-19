//! Categorising transactions an import left uncategorised: from the user's own history first,
//! when that's confident, then with the model the "categorise" task uses.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::config::{Kind, Task};
use super::http::Transport;
use super::providers::{JsonRequest, ProviderError, Resolved};
use super::{ResolveOrFail, log_call, resolve};
use crate::actions::{Source, changes};
use crate::imports::batches::UNCATEGORISED_PATH;
use crate::learned::Learned;
use crate::lookups::CategorisationSource;
use crate::settings::SettingError;

/// Jev reports calibrated confidence; below this, leave the transaction uncategorised rather
/// than guess. Behind the learned model, 0.9 got 98.8% of a test set right while still
/// categorising 83% of it automatically, against 95.0% right on 90% at 0.5.
pub const JEV_MIN_CONFIDENCE: f64 = 0.9;
const JEV_BATCH_SIZE: usize = 40;
const JEV_MAX_OPTIONS: usize = 255;

const SYSTEM: &str = "You are a personal finance assistant. You assign bank transactions to the \
                      most appropriate account from a fixed list.";

#[derive(Debug, Clone, PartialEq)]
pub struct Categorisation {
    pub transaction_id: i64,
    pub account_full_path: String,
    /// Only decision models such as Jev report one.
    pub confidence: Option<f64>,
}

fn schema(paths: &[String]) -> Value {
    json!({
        "type": "object",
        "properties": {
            "categorisations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "transaction_id": {"type": "integer"},
                        "account_full_path": {"type": "string", "enum": paths},
                    },
                    "required": ["transaction_id", "account_full_path"],
                    "additionalProperties": false,
                },
            }
        },
        "required": ["categorisations"],
        "additionalProperties": false,
    })
}

/// What the model says each transaction is, with the request and response for the logs.
/// Only answers naming one of the given transactions and accounts are returned.
pub async fn categorise_transactions(
    http: &dyn Transport,
    model: &Resolved,
    transactions: &[(i64, String)],
    account_paths: &[String],
) -> Result<(Vec<Categorisation>, Value, Value), (ProviderError, Value, Value)> {
    let mut paths = account_paths.to_vec();
    paths.sort();
    let (results, request, response) = match model.provider.kind() {
        Kind::Decision => with_decision_model(http, model, transactions, &paths).await?,
        Kind::Language => with_language_model(http, model, transactions, &paths).await?,
    };
    let ids: HashSet<i64> = transactions.iter().map(|(id, _)| *id).collect();
    let known: HashSet<&String> = paths.iter().collect();
    let valid = results
        .into_iter()
        .filter(|c| ids.contains(&c.transaction_id) && known.contains(&c.account_full_path))
        .collect();
    Ok((valid, request, response))
}

type Answered = Result<(Vec<Categorisation>, Value, Value), (ProviderError, Value, Value)>;

async fn with_language_model(
    http: &dyn Transport,
    model: &Resolved,
    transactions: &[(i64, String)],
    paths: &[String],
) -> Answered {
    let accounts: Vec<String> = paths.iter().map(|p| format!("- {p}")).collect();
    let listed: Vec<String> = transactions
        .iter()
        .map(|(id, d)| format!("- id={id}: {d}"))
        .collect();
    let prompt = format!(
        "Assign each bank transaction to the most appropriate account.\n\nAvailable \
         accounts:\n{}\n\nTransactions to categorise:\n{}\n\nReturn one entry per transaction \
         id, using only accounts from the list above.",
        accounts.join("\n"),
        listed.join("\n")
    );
    let schema = schema(paths);
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
                name: "categorise_transactions",
            },
        )
        .await
        .map_err(|e| (e, Value::Null, Value::Null))?;
    let results = call
        .data
        .get("categorisations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let id = c["transaction_id"].as_i64().filter(|id| *id != 0)?;
            let path = c["account_full_path"].as_str().filter(|p| !p.is_empty())?;
            Some(Categorisation {
                transaction_id: id,
                account_full_path: path.to_owned(),
                confidence: None,
            })
        })
        .collect();
    Ok((results, call.request, call.response))
}

async fn with_decision_model(
    http: &dyn Transport,
    model: &Resolved,
    transactions: &[(i64, String)],
    paths: &[String],
) -> Answered {
    if paths.len() > JEV_MAX_OPTIONS {
        let message = format!(
            "{} can choose between at most {JEV_MAX_OPTIONS} accounts, but you have {}.",
            model.provider.label(),
            paths.len()
        );
        return Err((ProviderError::new(message), Value::Null, Value::Null));
    }
    let criteria: Map<String, Value> = paths.iter().map(|p| (p.clone(), Value::Null)).collect();
    let (mut results, mut requests, mut responses) = (Vec::new(), Vec::new(), Vec::new());
    for batch in transactions.chunks(JEV_BATCH_SIZE) {
        // One shared state and one choice question per transaction, answered in one call.
        let state: Map<String, Value> = batch
            .iter()
            .map(|(id, d)| (format!("t{id}"), json!(d)))
            .collect();
        let questions: Map<String, Value> = batch
            .iter()
            .map(|(id, _)| {
                let instructions = format!(
                    "Which account should the bank transaction `t{id}` be recorded against? \
                     Accounts are colon-separated paths from general to specific."
                );
                (
                    format!("t{id}"),
                    json!({"type": "choice", "instructions": instructions, "criteria": criteria}),
                )
            })
            .collect();
        let call = match model
            .provider
            .choose(
                http,
                &model.cfg,
                &model.model,
                &Value::Object(state),
                &questions,
            )
            .await
        {
            Ok(call) => call,
            Err(e) => {
                return Err((e, json!({"calls": requests}), json!({"calls": responses})));
            }
        };
        requests.push(call.request);
        responses.push(call.response);
        for (id, _) in batch {
            let answer = &call
                .data
                .get(&format!("t{id}"))
                .cloned()
                .unwrap_or(Value::Null);
            let Some(choice) = answer["choice"]
                .as_str()
                .filter(|c| criteria.contains_key(*c))
            else {
                continue;
            };
            let confidence = answer["confidence"].as_f64();
            if confidence.unwrap_or(0.0) >= JEV_MIN_CONFIDENCE {
                results.push(Categorisation {
                    transaction_id: *id,
                    account_full_path: choice.to_owned(),
                    confidence,
                });
            }
        }
    }
    Ok((
        results,
        json!({"calls": requests}),
        json!({"calls": responses}),
    ))
}

/// What categorising did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// Categorised from the user's own history.
    pub learned: usize,
    /// Categorised by the model.
    pub ai: usize,
    /// Why the model couldn't help, if it couldn't.
    pub error: Option<String>,
}

impl Outcome {
    pub fn total(&self) -> usize {
        self.learned + self.ai
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CategoriseError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Setting(#[from] SettingError),
}

/// Categorises the transactions an import left uncategorised (in one batch, or everywhere):
/// from the user's history first, when that's confident, then the rest with the model if
/// `use_ai`. Records what changed as one undoable change. The model's errors are reported in
/// the outcome (and kept in the AI logs) rather than raised, so what was learned still stands.
pub async fn uncategorised(
    db: &SqlitePool,
    http: &dyn Transport,
    learned: &Learned,
    batch_id: Option<i64>,
    source: Source,
    use_ai: bool,
) -> Result<Outcome, CategoriseError> {
    let mut outcome = Outcome::default();
    let uncategorised_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM accounts WHERE full_path = ?")
            .bind(UNCATEGORISED_PATH)
            .fetch_optional(db)
            .await?;
    let Some(uncategorised_id) = uncategorised_id else {
        return Ok(outcome);
    };
    let postings: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT p.id, t.id, t.description FROM postings p
         JOIN transactions t ON t.id = p.transaction_id
         WHERE p.account_id = ? AND p.categorised_by_id = ?
           AND (? IS NULL OR t.import_batch_id = ?)
         ORDER BY p.id",
    )
    .bind(uncategorised_id)
    .bind(CategorisationSource::ImportDefault as i64)
    .bind(batch_id)
    .bind(batch_id)
    .fetch_all(db)
    .await?;
    if postings.is_empty() {
        return Ok(outcome);
    }

    let descriptions: Vec<String> = postings.iter().map(|(_, _, d)| d.clone()).collect();
    let suggestions = learned.suggest(db, &descriptions).await?;
    let mut moves: Vec<(i64, i64, CategorisationSource)> = Vec::new();
    let mut remaining = Vec::new();
    for ((posting, txn, description), suggestion) in postings.into_iter().zip(suggestions) {
        match suggestion {
            Some((category, _)) => moves.push((posting, category, CategorisationSource::Learned)),
            None => remaining.push((posting, txn, description)),
        }
    }
    outcome.learned = moves.len();

    if !remaining.is_empty() && use_ai {
        match ai_categorise(db, http, &remaining).await {
            Ok(by_ai) => {
                outcome.ai = by_ai.len();
                moves.extend(by_ai);
            }
            Err(e) => outcome.error = Some(e),
        }
    }
    if moves.is_empty() {
        return Ok(outcome);
    }

    let mut tx = db.begin().await?;
    let mut before = Vec::new();
    for (posting, category, source) in &moves {
        sqlx::query(
            "UPDATE postings SET account_id = ?, categorised_by_id = ?, rule_id = NULL
             WHERE id = ?",
        )
        .bind(category)
        .bind(*source as i64)
        .bind(posting)
        .execute(&mut *tx)
        .await?;
        before.push(json!({
            "id": posting,
            "account_id": uncategorised_id,
            "categorised_by_id": CategorisationSource::ImportDefault as i64,
            "rule_id": null,
        }));
    }
    let file: Option<String> = match batch_id {
        Some(id) => sqlx::query_scalar("SELECT file_name FROM import_batches WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .flatten(),
        None => None,
    };
    let count = before.len();
    let what = if count == 1 {
        "1 transaction".to_owned()
    } else {
        format!("{count} transactions")
    };
    let from = file
        .filter(|f| !f.is_empty())
        .map(|f| format!(" from {f}"))
        .unwrap_or_default();
    let how = if outcome.learned > 0 {
        format!(" ({} from your history)", outcome.learned)
    } else {
        String::new()
    };
    let summary = format!("Categorised {what}{from}{how}");
    changes::record(
        &mut *tx,
        &changes::NewChange {
            action: "recategorise_transactions",
            arguments: Some(&json!({ "import_batch_id": batch_id })),
            summary: &summary,
            // The same shape as recategorise_transactions', so it undoes the same way.
            undo: &json!({ "postings": before }),
            source,
            conversation_id: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(outcome)
}

/// The model's categories for these postings, as moves; the model's problem if it failed,
/// which the AI logs keep too.
async fn ai_categorise(
    db: &SqlitePool,
    http: &dyn Transport,
    remaining: &[(i64, i64, String)],
) -> Result<Vec<(i64, i64, CategorisationSource)>, String> {
    let model = match resolve(db, Task::Categorise, "categorise").await {
        ResolveOrFail::Ready(model) => model,
        ResolveOrFail::Failed(message) => return Err(message),
    };
    let accounts: Vec<(i64, String)> = sqlx::query_as("SELECT id, full_path FROM accounts")
        .fetch_all(db)
        .await
        .map_err(|e| e.to_string())?;
    let ids: HashMap<&str, i64> = accounts.iter().map(|(id, p)| (p.as_str(), *id)).collect();
    let paths: Vec<String> = accounts
        .iter()
        .map(|(_, p)| p.clone())
        .filter(|p| p != UNCATEGORISED_PATH)
        .collect();
    let transactions: Vec<(i64, String)> = remaining
        .iter()
        .map(|(_, txn, d)| (*txn, d.clone()))
        .collect();
    let started = Instant::now();
    match categorise_transactions(http, &model, &transactions, &paths).await {
        Ok((found, request, response)) => {
            log_call(
                db,
                &model.label(),
                "categorise",
                started,
                Some(&request),
                Some(&response),
                None,
            )
            .await;
            let by_txn: HashMap<i64, i64> = remaining.iter().map(|(p, t, _)| (*t, *p)).collect();
            Ok(found
                .into_iter()
                .filter_map(|c| {
                    let posting = by_txn.get(&c.transaction_id)?;
                    let category = ids.get(c.account_full_path.as_str())?;
                    Some((*posting, *category, CategorisationSource::Ai))
                })
                .collect())
        }
        Err((e, _, _)) => {
            log_call(
                db,
                &model.label(),
                "categorise",
                started,
                None,
                None,
                Some(&e.message),
            )
            .await;
            Err(e.message)
        }
    }
}
