//! Labelling recurring merchants (subscription, bill or other) and scoring how unusual each
//! new charge is, with the model the "insights" task uses. Runs after each import, or when
//! asked from the import's review page; the recurring_payments and unusual_charges actions
//! read the results.
//!
//! A decision model answers a choice question per merchant and a score question per charge;
//! language models return the same answers as JSON. A failing model never breaks an import:
//! the error is in the AI request logs and the work is retried on the next import.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::{Days, NaiveDate};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::config::{self, Kind, Task};
use super::http::Transport;
use super::log_call;
use super::providers::{self, JsonRequest, ProviderError, ResolveError, Resolved};
use crate::imports::batches::UNCATEGORISED_PATH;
use crate::insights::{Charge, charges, find_recurring, median, merchant_key, money};

const BATCH_SIZE: usize = 40;
const HISTORY_DAYS: u64 = 400;
const MERCHANT_HISTORY: usize = 12;

const KINDS: [(&str, &str); 3] = [
    (
        "subscription",
        "A subscription to a service the user signed up for, such as streaming, software, apps, \
         a gym, a magazine or a membership",
    ),
    (
        "bill",
        "A bill for a household essential or contract, such as rent, mortgage, council tax, \
         energy, water, insurance, phone, broadband or a loan repayment",
    ),
    (
        "other",
        "Not a commitment: ordinary spending that just happens regularly, such as groceries, \
         coffee, lunches, fuel or transport fares",
    ),
];

/// Score levels, low to high. Stored as level / (count - 1), so 0 is normal and 1 very
/// unusual.
const UNUSUAL_LEVELS: [&str; 4] = [
    "Normal: in line with what this merchant usually charges, or an everyday amount for the \
     category",
    "A little different: somewhat more than usual, or a new merchant charging an ordinary amount",
    "Unusual: clearly more than this merchant usually charges, or a large one-off for the \
     category",
    "Very unusual: far beyond anything seen before; could be a mistake, a duplicate or fraud",
];

const SYSTEM: &str = "You review a person's bank transactions in a personal finance app.";

fn is_kind(kind: &str) -> bool {
    KINDS.iter().any(|(k, _)| *k == kind)
}

fn kinds_criteria() -> Map<String, Value> {
    KINDS
        .iter()
        .map(|(k, text)| ((*k).to_owned(), json!(text)))
        .collect()
}

/// Asks the model, logging the call. `decision` and `language` each return what was found,
/// with the request and response.
async fn call<T>(
    db: &SqlitePool,
    model: &Resolved,
    operation: &str,
    asked: Result<(T, Value, Value), ProviderError>,
    started: Instant,
) -> Result<T, ProviderError> {
    match asked {
        Ok((found, request, response)) => {
            log_call(
                db,
                &model.label(),
                operation,
                started,
                Some(&request),
                Some(&response),
                None,
            )
            .await;
            Ok(found)
        }
        Err(e) => {
            log_call(
                db,
                &model.label(),
                operation,
                started,
                None,
                None,
                Some(&e.message),
            )
            .await;
            Err(e)
        }
    }
}

// Merchants

type Kinds = HashMap<String, (String, Option<f64>)>;

async fn kinds_with_decision_model(
    http: &dyn Transport,
    model: &Resolved,
    merchants: &[Value],
) -> Result<(Kinds, Value, Value), ProviderError> {
    let criteria = kinds_criteria();
    let (mut found, mut requests, mut responses) = (HashMap::new(), Vec::new(), Vec::new());
    for (n, batch) in merchants.chunks(BATCH_SIZE).enumerate() {
        let start = n * BATCH_SIZE;
        let keys: Vec<String> = (0..batch.len())
            .map(|i| format!("m{}", start + i))
            .collect();
        let state: Map<String, Value> = keys.iter().cloned().zip(batch.iter().cloned()).collect();
        let questions: Map<String, Value> = keys
            .iter()
            .map(|key| {
                let instructions = format!("What kind of regular payment is `{key}`?");
                (
                    key.clone(),
                    json!({"type": "choice", "instructions": instructions, "criteria": criteria}),
                )
            })
            .collect();
        let call = model
            .provider
            .choose(
                http,
                &model.cfg,
                &model.model,
                &Value::Object(state),
                &questions,
            )
            .await?;
        for (key, merchant) in keys.iter().zip(batch) {
            let answer = call.data.get(key).cloned().unwrap_or(Value::Null);
            if let Some(kind) = answer["choice"].as_str().filter(|k| is_kind(k)) {
                let name = merchant["merchant"].as_str().unwrap_or_default().to_owned();
                found.insert(name, (kind.to_owned(), answer["confidence"].as_f64()));
            }
        }
        requests.push(call.request);
        responses.push(call.response);
    }
    Ok((
        found,
        json!({"calls": requests}),
        json!({"calls": responses}),
    ))
}

async fn kinds_with_language_model(
    http: &dyn Transport,
    model: &Resolved,
    merchants: &[Value],
) -> Result<(Kinds, Value, Value), ProviderError> {
    let kinds: Vec<String> = KINDS.iter().map(|(k, t)| format!("- {k}: {t}")).collect();
    let indexed: Vec<Value> = merchants
        .iter()
        .enumerate()
        .map(|(i, m)| json!([i, m]))
        .collect();
    let listing = serde_json::to_string_pretty(&indexed).expect("JSON serialises");
    let prompt = format!(
        "These payments repeat regularly. Say what kind each one is.\n\nKinds:\n{}\n\nPayments, \
         as [index, details]:\n{listing}",
        kinds.join("\n")
    );
    let names: Vec<&str> = KINDS.iter().map(|(k, _)| *k).collect();
    let schema = json!({
        "type": "object",
        "properties": {
            "labels": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "index": {"type": "integer"},
                        "kind": {"type": "string", "enum": names},
                    },
                    "required": ["index", "kind"],
                    "additionalProperties": false,
                },
            }
        },
        "required": ["labels"],
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
                name: "label_merchants",
            },
        )
        .await?;
    let mut found = HashMap::new();
    for label in call
        .data
        .get("labels")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(index) = label["index"].as_u64().map(|i| i as usize) else {
            continue;
        };
        if let (Some(merchant), Some(kind)) = (merchants.get(index), label["kind"].as_str())
            && is_kind(kind)
        {
            let name = merchant["merchant"].as_str().unwrap_or_default().to_owned();
            found.insert(name, (kind.to_owned(), None));
        }
    }
    Ok((found, call.request, call.response))
}

/// Labels recurring merchants that haven't been labelled yet. Returns how many were.
pub async fn label_merchants(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    today: NaiveDate,
) -> Result<usize, InsightsError> {
    let labelled: HashSet<String> = sqlx::query_scalar("SELECT merchant_key FROM merchant_labels")
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();
    let unlabelled: Vec<_> = find_recurring(db, today)
        .await?
        .into_iter()
        .filter(|r| !labelled.contains(&r.merchant_key))
        .collect();
    if unlabelled.is_empty() {
        return Ok(0);
    }
    let merchants: Vec<Value> = unlabelled
        .iter()
        .map(|r| {
            json!({
                "merchant": r.merchant,
                "category": r.category,
                "cadence": r.cadence,
                "amount": r.amount,
            })
        })
        .collect();
    let started = Instant::now();
    let asked = match model.provider.kind() {
        Kind::Decision => kinds_with_decision_model(http, model, &merchants).await,
        Kind::Language => kinds_with_language_model(http, model, &merchants).await,
    };
    let found = call(db, model, "label_merchants", asked, started).await?;
    for r in &unlabelled {
        if let Some((kind, confidence)) = found.get(&r.merchant) {
            sqlx::query(
                "INSERT INTO merchant_labels (merchant_key, kind, confidence) VALUES (?, ?, ?)
                 ON CONFLICT (merchant_key) DO NOTHING",
            )
            .bind(&r.merchant_key)
            .bind(kind)
            .bind(confidence)
            .execute(db)
            .await?;
        }
    }
    Ok(found.len())
}

// Unusual charges

/// What the model sees for one charge: it, the merchant's earlier charges and what's typical
/// for the category over the past year.
fn context(charge: &Charge, history: &[&Charge]) -> Value {
    let before: Vec<&&Charge> = history
        .iter()
        .filter(|h| h.date < charge.date || (h.date == charge.date && h.id < charge.id))
        .collect();
    let key = merchant_key(&charge.description);
    let merchant: Vec<&&&Charge> = before
        .iter()
        .filter(|h| merchant_key(&h.description) == key)
        .collect();
    let merchant = &merchant[merchant.len().saturating_sub(MERCHANT_HISTORY)..];
    let mut context = json!({
        "charge": {
            "date": charge.date,
            "description": charge.description,
            "amount": money(charge.amount),
            "category": charge.category,
        },
        "merchant_history": merchant
            .iter()
            .map(|h| json!({"date": h.date, "amount": money(h.amount)}))
            .collect::<Vec<_>>(),
    });
    if charge.category != UNCATEGORISED_PATH {
        let mut amounts: Vec<Decimal> = before
            .iter()
            .filter(|h| h.category == charge.category)
            .map(|h| h.amount)
            .collect();
        amounts.sort();
        if let Some(largest) = amounts.last() {
            context["category_last_year"] = json!({
                "charges": amounts.len(),
                "median": money(median(&amounts)),
                "largest": money(*largest),
            });
        }
    }
    context
}

type Scores = HashMap<i64, f64>;

async fn scores_with_decision_model(
    http: &dyn Transport,
    model: &Resolved,
    contexts: &[(i64, Value)],
) -> Result<(Scores, Value, Value), ProviderError> {
    let (mut found, mut requests, mut responses) = (HashMap::new(), Vec::new(), Vec::new());
    for batch in contexts.chunks(BATCH_SIZE) {
        let state: Map<String, Value> = batch
            .iter()
            .map(|(id, c)| (format!("c{id}"), c.clone()))
            .collect();
        let questions: Map<String, Value> = batch
            .iter()
            .map(|(id, _)| {
                let instructions = format!(
                    "How unusual is `c{id}.charge` compared with `c{id}.merchant_history` and \
                     what is typical for its category?"
                );
                (
                    format!("c{id}"),
                    json!({"type": "score", "instructions": instructions, "criteria": UNUSUAL_LEVELS}),
                )
            })
            .collect();
        let call = model
            .provider
            .choose(
                http,
                &model.cfg,
                &model.model,
                &Value::Object(state),
                &questions,
            )
            .await?;
        for (id, _) in batch {
            if let Some(score) = call
                .data
                .get(&format!("c{id}"))
                .and_then(|a| a["score"].as_f64())
            {
                found.insert(*id, score);
            }
        }
        requests.push(call.request);
        responses.push(call.response);
    }
    Ok((
        found,
        json!({"calls": requests}),
        json!({"calls": responses}),
    ))
}

async fn scores_with_language_model(
    http: &dyn Transport,
    model: &Resolved,
    contexts: &[(i64, Value)],
) -> Result<(Scores, Value, Value), ProviderError> {
    let levels: Vec<String> = UNUSUAL_LEVELS
        .iter()
        .enumerate()
        .map(|(n, text)| format!("{n}: {text}"))
        .collect();
    let keyed: Map<String, Value> = contexts
        .iter()
        .map(|(id, c)| (id.to_string(), c.clone()))
        .collect();
    let listing = serde_json::to_string_pretty(&keyed).expect("JSON serialises");
    let prompt = format!(
        "Rate how unusual each new charge is, compared with the merchant's earlier charges and \
         what is typical for its category.\n\nLevels:\n{}\n\nCharges, keyed by id:\n{listing}",
        levels.join("\n")
    );
    let level_values: Vec<usize> = (0..UNUSUAL_LEVELS.len()).collect();
    let schema = json!({
        "type": "object",
        "properties": {
            "scores": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "integer"},
                        "level": {"type": "integer", "enum": level_values},
                    },
                    "required": ["id", "level"],
                    "additionalProperties": false,
                },
            }
        },
        "required": ["scores"],
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
                name: "score_charges",
            },
        )
        .await?;
    let ids: HashSet<i64> = contexts.iter().map(|(id, _)| *id).collect();
    let mut found = HashMap::new();
    for item in call
        .data
        .get("scores")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if let (Some(id), Some(level)) = (item["id"].as_i64(), item["level"].as_f64())
            && ids.contains(&id)
        {
            found.insert(id, level);
        }
    }
    Ok((found, call.request, call.response))
}

/// Scores how unusual each of these transactions' spending is. Returns how many were.
pub async fn score_charges(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    transaction_ids: &[i64],
) -> Result<usize, InsightsError> {
    let wanted: HashSet<i64> = transaction_ids.iter().copied().collect();
    let dates: Vec<NaiveDate> = {
        let mut query = sqlx::QueryBuilder::new("SELECT date FROM transactions WHERE id IN (");
        let mut list = query.separated(", ");
        for id in transaction_ids {
            list.push_bind(*id);
        }
        query.push(")");
        query.build_query_scalar().fetch_all(db).await?
    };
    let (Some(first), Some(last)) = (dates.iter().min(), dates.iter().max()) else {
        return Ok(0);
    };
    let mut history = charges(db, *first - Days::new(HISTORY_DAYS), *last).await?;
    history.sort_by_key(|c| (c.date, c.id));
    let contexts: Vec<(i64, Value)> = history
        .iter()
        .filter(|c| wanted.contains(&c.id))
        .map(|c| {
            let since = c.date - Days::new(HISTORY_DAYS);
            let window: Vec<&Charge> = history.iter().filter(|h| h.date >= since).collect();
            (c.id, context(c, &window))
        })
        .collect();
    if contexts.is_empty() {
        return Ok(0);
    }
    let started = Instant::now();
    let asked = match model.provider.kind() {
        Kind::Decision => scores_with_decision_model(http, model, &contexts).await,
        Kind::Language => scores_with_language_model(http, model, &contexts).await,
    };
    let scores = call(db, model, "score_charges", asked, started).await?;
    let top = (UNUSUAL_LEVELS.len() - 1) as f64;
    for (id, score) in &scores {
        let stored = ((score / top).clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
        sqlx::query("UPDATE transactions SET unusual_score = ? WHERE id = ?")
            .bind(stored)
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(scores.len())
}

#[derive(Debug, thiserror::Error)]
pub enum InsightsError {
    #[error("{}", .0.message)]
    Model(#[from] ProviderError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Scores the import's charges and labels recurring merchants that are new as of its latest
/// charge, if insights are on and have a model. Returns the model's problem, if it had one.
pub async fn run_after_import(
    db: &SqlitePool,
    http: &dyn Transport,
    batch_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    let (provider, model) = config::task_model(db, Task::Insights)
        .await
        .unwrap_or((None, None));
    if provider.is_none() || model.is_none() {
        return Ok(None);
    }
    let model = match providers::resolve_task(db, Task::Insights).await {
        Ok(model) => model,
        Err(ResolveError::Provider(e)) => return Ok(Some(e.message)),
        Err(ResolveError::Setting(e)) => return Ok(Some(e.to_string())),
    };
    let rows: Vec<(i64, NaiveDate)> =
        sqlx::query_as("SELECT id, date FROM transactions WHERE import_batch_id = ?")
            .bind(batch_id)
            .fetch_all(db)
            .await?;
    let Some(latest) = rows.iter().map(|(_, d)| *d).max() else {
        return Ok(None);
    };
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let result = async {
        score_charges(db, http, &model, &ids).await?;
        label_merchants(db, http, &model, latest).await
    }
    .await;
    match result {
        Ok(_) => Ok(None),
        Err(InsightsError::Model(e)) => Ok(Some(e.message)),
        Err(InsightsError::Database(e)) => Err(e),
    }
}
