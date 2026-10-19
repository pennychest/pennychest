//! Asking the model the "matching" task uses whether two records are the same thing, where
//! exact matching falls short: a card tap and a statement line, two imported transactions, or
//! the two sides of a transfer.
//!
//! A decision model answers one yes/no question per pair; language models return a
//! probability per pair. Callers act only on pairs at or above `MIN_PROBABILITY`.

use std::time::Instant;

use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::config::Kind;
use super::http::Transport;
use super::log_call;
use super::providers::{JsonRequest, ProviderError, Resolved};

pub const MIN_PROBABILITY: f64 = 0.8;
const BATCH_SIZE: usize = 40;

const SYSTEM: &str = "You compare financial records from a personal finance app and judge \
                      whether two of them are related in the way described.";

/// `question` with `{a}` and `{b}` standing for the two records.
fn worded(question: &str, a: &str, b: &str) -> String {
    question.replace("{a}", a).replace("{b}", b)
}

async fn with_decision_model(
    http: &dyn Transport,
    model: &Resolved,
    question: &str,
    pairs: &[(Value, Value)],
) -> Result<(Vec<f64>, Value, Value), ProviderError> {
    let (mut probabilities, mut requests, mut responses) = (Vec::new(), Vec::new(), Vec::new());
    for (n, batch) in pairs.chunks(BATCH_SIZE).enumerate() {
        let keys: Vec<String> = (0..batch.len())
            .map(|i| format!("p{}", n * BATCH_SIZE + i))
            .collect();
        let state: Map<String, Value> = keys
            .iter()
            .zip(batch)
            .map(|(key, (a, b))| (key.clone(), json!({"a": a, "b": b})))
            .collect();
        let questions: Map<String, Value> = keys
            .iter()
            .map(|key| {
                let instructions = worded(question, &format!("`{key}.a`"), &format!("`{key}.b`"));
                (
                    key.clone(),
                    json!({"type": "noul", "instructions": instructions}),
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
        for key in &keys {
            let value = call.data.get(key).and_then(|a| a["noul"].as_f64());
            probabilities.push(value.unwrap_or(0.0));
        }
        requests.push(call.request);
        responses.push(call.response);
    }
    Ok((
        probabilities,
        json!({"calls": requests}),
        json!({"calls": responses}),
    ))
}

async fn with_language_model(
    http: &dyn Transport,
    model: &Resolved,
    question: &str,
    pairs: &[(Value, Value)],
) -> Result<(Vec<f64>, Value, Value), ProviderError> {
    let listed: Vec<Value> = pairs
        .iter()
        .enumerate()
        .map(|(i, (a, b))| json!({"pair": i, "a": a, "b": b}))
        .collect();
    let listing = serde_json::to_string_pretty(&listed).expect("JSON serialises");
    let prompt = format!(
        "For each pair below, answer: {}\n\n{listing}\n\nGive each pair's probability, from 0 to \
         1, that the answer is yes.",
        worded(question, "`a`", "`b`")
    );
    let schema = json!({
        "type": "object",
        "properties": {
            "answers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "pair": {"type": "integer"},
                        "probability": {"type": "number"},
                    },
                    "required": ["pair", "probability"],
                    "additionalProperties": false,
                },
            }
        },
        "required": ["answers"],
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
                name: "match_pairs",
            },
        )
        .await?;
    let mut probabilities = vec![0.0; pairs.len()];
    for answer in call
        .data
        .get("answers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if let (Some(index), Some(value)) =
            (answer["pair"].as_u64(), answer["probability"].as_f64())
            && let Some(slot) = probabilities.get_mut(index as usize)
        {
            *slot = value.clamp(0.0, 1.0);
        }
    }
    Ok((probabilities, call.request, call.response))
}

/// The probability that the answer to `question` is yes for each `(a, b)` pair, in order.
/// `question` refers to the records as `{a}` and `{b}`. The call is logged under `operation`
/// either way.
pub async fn ask_pairs(
    db: &SqlitePool,
    http: &dyn Transport,
    model: &Resolved,
    operation: &str,
    question: &str,
    pairs: &[(Value, Value)],
) -> Result<Vec<f64>, ProviderError> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let started = Instant::now();
    let asked = match model.provider.kind() {
        Kind::Decision => with_decision_model(http, model, question, pairs).await,
        Kind::Language => with_language_model(http, model, question, pairs).await,
    };
    match asked {
        Ok((probabilities, request, response)) => {
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
            Ok(probabilities)
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

/// For each record, the candidate the model was most confident about, if confident enough.
/// `pairs` are `(record, candidate)` in the order they were asked.
pub fn best<K: Copy + Eq + std::hash::Hash, C: Clone>(
    pairs: &[(K, C)],
    probabilities: &[f64],
) -> std::collections::HashMap<K, C> {
    let mut best: std::collections::HashMap<K, (C, f64)> = std::collections::HashMap::new();
    for ((key, candidate), probability) in pairs.iter().zip(probabilities) {
        let beaten = best.get(key).is_none_or(|(_, p)| probability > p);
        if *probability >= MIN_PROBABILITY && beaten {
            best.insert(*key, (candidate.clone(), *probability));
        }
    }
    best.into_iter().map(|(k, (c, _))| (k, c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_confident_candidate_wins_if_confident_enough() {
        let pairs = [(1, "a"), (1, "b"), (2, "c")];
        let chosen = best(&pairs, &[0.85, 0.9, 0.5]);
        assert_eq!(chosen.get(&1), Some(&"b"));
        assert_eq!(chosen.get(&2), None);
    }
}
